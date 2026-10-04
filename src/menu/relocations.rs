//! Pinned main-menu cell relocation into formerly transparent, unreferenced atlas space.
use crate::{authoring, graphics, sha256, snc, zip_patch};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::{Cursor, Read},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    archive_sha256: String,
    cell_table_sha256: String,
    members: Vec<Member>,
    relocations: Vec<Edit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    path: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    cell: snc::CellRelocation,
    png: String,
    png_sha256: String,
}

fn images(snt: &[u8]) -> Result<Vec<graphics::Image>> {
    graphics::snt_table(snt)?
        .into_iter()
        .map(|(at, n)| graphics::decode(graphics::bytes(snt, at, n)?))
        .collect()
}

pub(super) fn apply(
    root: &Path,
    original: &[u8],
    current: Vec<u8>,
    snt_name: &str,
    snt: &[u8],
) -> Result<(Vec<u8>, Value)> {
    let manifest_bytes = fs::read(root.join("config/menu-cell-relocations.json"))?;
    let plan: Manifest = serde_json::from_slice(&manifest_bytes)?;
    ensure!(
        sha256(original) == plan.archive_sha256,
        "relocation archive identity mismatch"
    );
    let mut zip = zip::ZipArchive::new(Cursor::new(original))?;
    let actual: BTreeSet<_> = zip
        .file_names()
        .filter(|s| s.ends_with(".snc"))
        .map(str::to_owned)
        .collect();
    let specified: BTreeSet<_> = plan.members.iter().map(|m| m.path.clone()).collect();
    ensure!(
        specified.len() == plan.members.len() && actual == specified,
        "relocation must cover exactly every SNC member"
    );
    let mut old_snt = Vec::new();
    zip.by_name(snt_name)?.read_to_end(&mut old_snt)?;
    let before = images(&old_snt)?;
    let after = images(snt)?;
    let dimensions: Vec<_> = before.iter().map(|im| (im.width, im.height)).collect();
    ensure!(
        dimensions
            == after
                .iter()
                .map(|im| (im.width, im.height))
                .collect::<Vec<_>>(),
        "relocation texture dimensions changed"
    );
    let edits: Vec<_> = plan.relocations.iter().map(|e| e.cell.clone()).collect();
    ensure!(!edits.is_empty(), "empty relocation manifest");
    let mut result = current;
    let mut receipts = Vec::new();
    // Validate every member and destination before mutating the ZIP.
    let mut replacements = Vec::new();
    for member in &plan.members {
        let mut bytes = Vec::new();
        zip.by_name(&member.path)?.read_to_end(&mut bytes)?;
        ensure!(
            sha256(&bytes) == member.sha256,
            "relocation SNC identity mismatch"
        );
        let table = snc::CellTable::parse(&bytes, &dimensions)?;
        ensure!(
            table.cell_sha256 == plan.cell_table_sha256,
            "relocation cell table mismatch"
        );
        let changed = snc::relocate_cells(&bytes, &dimensions, &edits)?;
        replacements.push((&member.path, changed));
    }
    for edit in &plan.relocations {
        let e = &edit.cell;
        let source = before
            .get(e.source_slot)
            .context("relocation source texture")?;
        let source_after = after
            .get(e.source_slot)
            .context("relocation source texture after")?;
        let dest = before
            .get(e.target_slot)
            .context("relocation destination texture")?;
        let dest_after = after
            .get(e.target_slot)
            .context("relocation destination texture after")?;
        let path = root.join(&edit.png);
        ensure!(
            sha256(&fs::read(&path)?) == edit.png_sha256,
            "relocation PNG hash mismatch"
        );
        let png = authoring::png_read(&path)?;
        let [sx, sy, w, h] = e.source_rect;
        ensure!(
            (png.width, png.height) == (w as usize, h as usize),
            "relocation PNG dimensions mismatch"
        );
        let [tx, ty] = e.target_origin;
        for y in 0..png.height {
            for x in 0..png.width {
                let old = ((sy as usize + y) * source.width + sx as usize + x) * 4;
                ensure!(
                    source.rgba[old..old + 4] == source_after.rgba[old..old + 4],
                    "shared source pixels changed during relocation"
                );
                let new = ((ty + y) * dest.width + tx + x) * 4;
                ensure!(
                    dest.rgba[new + 3] == 0,
                    "relocation destination was not transparent"
                );
                let pixel = (y * png.width + x) * 4;
                ensure!(
                    dest_after.rgba[new..new + 4] == png.rgba[pixel..pixel + 4],
                    "relocation destination does not contain prepared PNG"
                );
            }
        }
    }
    for (name, bytes) in replacements {
        let (next, receipt) = zip_patch::replace(&result, name, &bytes)?;
        result = next;
        receipts.push(receipt);
    }
    Ok((
        result,
        json!({"manifest_sha256":sha256(&manifest_bytes),"relocations":edits,
        "snc_members":receipts,"shared_source_pixels_preserved":true,
        "original_destinations_transparent":true,"prepared_pngs_verified":true}),
    ))
}
