//! Assemble one expected-write plan and audit the complete development ISO.
use super::{MenuSource, field, read};
use crate::{
    sha256,
    write_plan::{self, ExpectedWrite},
    zip_patch,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub struct BuildOptions<'a> {
    pub snt_report: &'a Path,
    pub output: &'a Path,
    pub descriptions: bool,
    pub expand_story_font: bool,
    pub opening: bool,
    pub story: bool,
    pub fix_font_copy: bool,
    pub ppsspp_root: Option<&'a Path>,
    pub remove_install_data: bool,
}

pub fn build(root: &Path, source_path: &Path, options: BuildOptions<'_>) -> Result<Value> {
    let BuildOptions {
        snt_report,
        output,
        descriptions,
        expand_story_font,
        opening,
        story,
        fix_font_copy,
        ppsspp_root,
        remove_install_data,
    } = options;
    let MenuSource {
        mut source,
        profile,
        manifest_bytes,
        manifest,
        layout,
        offset,
        before,
        members,
    } = read(root, source_path)?;
    ensure!(!output.exists(), "output already exists");
    let receipt_bytes = fs::read(snt_report)?;
    let receipt: Value = serde_json::from_slice(&receipt_bytes)?;
    let snt = fs::read(
        snt_report
            .parent()
            .context("receipt parent")?
            .join("mainmenu.snt"),
    )?;
    ensure!(
        field(&receipt, "translation_sha256")?
            == sha256(&fs::read(
                root.join("assets/translation/ui/main-menu.json")
            )?)
            && field(&receipt, "manifest_sha256")? == sha256(&manifest_bytes)
            && field(&receipt, "source_sha256")? == field(&manifest["member"], "sha256")?
            && field(&receipt, "output_sha256")? == sha256(&snt),
        "SNT receipt mismatch"
    );
    let member = field(&manifest["member"], "path")?;
    let original_member = members
        .iter()
        .find(|r| r["name"] == member)
        .context("missing SNT")?;
    ensure!(
        field(original_member, "sha256")? == field(&manifest["member"], "sha256")?,
        "original SNT mismatch"
    );
    let (after, zreceipt) = zip_patch::replace(&before, member, &snt)?;
    let (after, relocation_receipt) =
        super::relocations::apply(root, &before, after, member, &snt)?;
    let mut planned = vec![ExpectedWrite {
        offset,
        before,
        after,
    }];
    // Index occurrences are separate extents even when their original bytes match.
    // Reuse the complete validated result, including the SNC cell relocations.
    for &copy_offset in &layout.identical_archive_offsets {
        let original = crate::source::read_extent(&mut source, copy_offset, layout.zip_size)?;
        ensure!(
            original == planned[0].before,
            "duplicate menu archive differs from the verified original"
        );
        planned.push(ExpectedWrite {
            offset: copy_offset,
            before: original,
            after: planned[0].after.clone(),
        });
    }
    let game_range = layout.game_iso_offset..layout.game_iso_offset + layout.game_size;
    let mut allowed = vec![game_range];
    let mut executable_receipt = Value::Null;
    let mut verified_font_copy = None;
    let mut story_fonts = Vec::new();
    let mut story_manifest_hash = Value::Null;
    if story {
        let raw = fs::read(root.join("config/story-build.json"))?;
        let manifest: Value = serde_json::from_slice(&raw)?;
        let pairs = manifest["pairs"].as_array().context("story pairs")?;
        ensure!(!pairs.is_empty(), "empty story manifest");
        for pair in pairs {
            story_fonts.push(crate::story_font::plan(
                root,
                &mut source,
                Path::new(pair["font"].as_str().context("story font config")?),
                Some(Path::new(
                    pair["text"].as_str().context("story text config")?,
                )),
            )?);
        }
        story_manifest_hash = json!(sha256(&raw));
    } else if expand_story_font {
        story_fonts.push(crate::story_font::plan(
            root,
            &mut source,
            Path::new("config/story-font.json"),
            opening.then_some(Path::new("config/story-text.json")),
        )?);
    }
    let menu_changes = if descriptions {
        crate::menu_text::index_changes(root)?
    } else {
        Vec::new()
    };
    if fix_font_copy {
        let (write, receipt, verified) = crate::executable::plan_font_copy(
            root,
            &mut source,
            ppsspp_root.context("PPSSPP root required")?,
            &story_fonts,
            &menu_changes,
        )?;
        allowed.push(write.offset..write.offset + write.before.len() as u64);
        planned.push(write);
        executable_receipt = receipt;
        verified_font_copy = Some(verified);
    }
    let mut story_font_receipt = Value::Null;
    let mut story_receipts = Vec::new();
    for plan in story_fonts {
        ensure!(
            verified_font_copy.is_some(),
            "story expansion requires executable plan"
        );
        planned.push(plan.write);
        if expand_story_font {
            story_font_receipt = plan.receipt.clone();
        }
        story_receipts.push(plan.receipt);
    }
    let mut text_receipt = Value::Null;
    if descriptions {
        let (text_writes, receipt) =
            crate::menu_text::plan(root, &mut source, verified_font_copy.as_ref())?;
        planned.extend(text_writes);
        text_receipt = receipt;
    }
    let (graphic_writes, graphic_receipt) = crate::graphic_archives::plan(root, &mut source)?;
    planned.extend(graphic_writes);
    let mut output_len = profile.size_bytes;
    let mut install_receipt = Value::Null;
    if remove_install_data {
        let plan = crate::install_data::plan(&mut source, profile.size_bytes)?;
        for w in &plan.writes {
            allowed.push(w.offset..w.offset + w.before.len() as u64);
        }
        planned.extend(plan.writes);
        output_len = plan.output_len;
        install_receipt = plan.receipt;
    }
    let mut disc_metadata_receipt = Value::Null;
    if remove_install_data {
        let plan = crate::disc_metadata::plan(root, &mut source)?;
        allowed.push(plan.allowed);
        planned.extend(plan.writes);
        disc_metadata_receipt = plan.receipt;
    }
    planned.sort_by_key(|w| w.offset);

    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = tempfile::Builder::new()
        .prefix(".menu-build-")
        .tempdir_in(parent)?;
    let mut dest = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(temp.path().join("menu.iso"))?;
    let (hash, writes) = write_plan::apply_and_audit(
        &mut source,
        &mut dest,
        &profile,
        &planned,
        &allowed,
        output_len,
    )?;
    dest.sync_all()?;
    let report = json!({"scope":"development ISO; changed-artwork runtime and install path unverified","source_sha256":profile.sha256,"output_sha256":hash,"size_bytes":output_len,"install_data":install_receipt,"disc_metadata":disc_metadata_receipt,"snt_report_sha256":sha256(&receipt_bytes),"manifest_sha256":sha256(&manifest_bytes),"writes":writes,"zip":zreceipt,"cell_relocations":relocation_receipt,"graphic_archives":graphic_receipt,"descriptions":text_receipt,"executable":executable_receipt,"story_font":story_font_receipt,"story_pairs":story_receipts,"story_manifest_sha256":story_manifest_hash});
    fs::write(
        temp.path().join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    ensure!(!output.exists(), "output appeared during build");
    fs::rename(temp.path(), output)?;
    Ok(report)
}
