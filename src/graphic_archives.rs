//! Fixed-size graphic ZIP edits bounded by SNC writers and declared shared views, or GIM rectangles.
use crate::{authoring, graphics, sha256, snc, source, write_plan::ExpectedWrite, zip_patch};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Read},
    path::Path,
};

mod gim;
mod manifest;
mod preparation;
mod snt;
use gim::*;
pub use manifest::BUILD_DIR;
use manifest::*;
pub(crate) use preparation::merge_rectangles;
pub use preparation::prepare_gim;
use snt::*;

fn member(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>> {
    let mut f = zip.by_name(name)?;
    ensure!(f.size() <= 128 * 1024 * 1024, "oversized graphics member");
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn plan(root: &Path, iso: &mut fs::File) -> Result<(Vec<ExpectedWrite>, Value)> {
    let Some((manifest, manifest_files)) = read_manifest(root)? else {
        return Ok((Vec::new(), Value::Null));
    };
    let mut writes = Vec::new();
    let mut receipts = Vec::new();
    for archive in manifest.archives {
        ensure!(
            (!archive.patches.is_empty() || !archive.prepared_dxt.is_empty())
                && !archive.snc_members.is_empty(),
            "empty graphic edit"
        );
        let before = source::read_extent(iso, archive.offset, archive.size)?;
        ensure!(
            sha256(&before) == archive.sha256,
            "graphic ZIP identity mismatch"
        );
        let mut zip = zip::ZipArchive::new(Cursor::new(before.as_slice()))?;
        let mut edits = vec![SntEdit {
            member: archive.member,
            member_sha256: archive.member_sha256,
            snc_members: archive.snc_members,
            cell_table_sha256: archive.cell_table_sha256,
            patches: archive.patches,
            unmapped_regions: archive.unmapped_regions,
            prepared_dxt: archive.prepared_dxt,
            shared_views: archive.shared_views,
            snc_geometry: archive.snc_geometry,
            snc_cell_remaps: archive.snc_cell_remaps,
        }];
        edits.extend(archive.additional_snt_members);
        validate_snt_members(&edits, &archive.gim_members)?;
        let mut all_checked = BTreeSet::new();
        for edit in &edits {
            for record in &edit.snc_members {
                ensure!(
                    all_checked.insert(record.path.clone()),
                    "duplicate SNC declaration"
                );
            }
        }
        for record in &archive.other_snt_snc_members {
            ensure!(
                edits.iter().all(|edit| record.snt != edit.member)
                    && record.snt != record.path
                    && all_checked.insert(record.path.clone()),
                "invalid other-SNT SNC declaration"
            );
            let other = member(&mut zip, &record.snt)?;
            ensure!(
                sha256(&other) == record.snt_sha256,
                "other SNT identity mismatch"
            );
            let bytes = member(&mut zip, &record.path)?;
            ensure!(sha256(&bytes) == record.sha256, "SNC identity mismatch");
            snc::inspect(&other, &bytes, None)?;
        }
        let actual: BTreeSet<_> = zip
            .file_names()
            .filter(|s| s.ends_with(".snc"))
            .map(str::to_owned)
            .collect();
        ensure!(
            actual == all_checked,
            "all archive SNC members must be checked"
        );
        let mut after = before.clone();
        let mut snt_receipts = Vec::new();
        for edit in edits {
            let (next, receipt) = apply_snt_member(root, &mut zip, after, edit)?;
            after = next;
            snt_receipts.push(receipt);
        }
        let (after, gim_receipts) = apply_gim_members(root, &before, after, archive.gim_members)?;
        // Keep existing primary-member receipt fields stable.
        let mut receipt = snt_receipts.remove(0);
        receipt["offset"] = json!(archive.offset);
        receipt["gim_members"] = json!(gim_receipts);
        receipt["additional_snt_members"] = json!(snt_receipts);
        receipts.push(receipt);
        writes.push(ExpectedWrite {
            offset: archive.offset,
            before,
            after,
        });
    }
    for archive in manifest.gim_archives {
        let (write, receipt) = plan_gim_archive(root, iso, archive)?;
        writes.push(write);
        receipts.push(receipt);
    }
    Ok((
        writes,
        json!({"manifest_files":manifest_files,"archives":receipts}),
    ))
}

#[cfg(test)]
mod tests;
