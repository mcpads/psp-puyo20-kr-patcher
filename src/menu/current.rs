//! Rebuild all adopted inputs without relying on an earlier work directory.
use anyhow::{Result, ensure};
use serde_json::Value;
use std::{fs, path::Path};

pub fn build_current(
    root: &Path,
    source: &Path,
    ppsspp_root: &Path,
    output: &Path,
) -> Result<Value> {
    ensure!(!output.exists(), "output already exists");
    crate::pins::run(root, &[])?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".full-build-")
        .tempdir_in(parent)?;
    let preparation = staging.path().join("preparation");
    fs::create_dir(&preparation)?;
    super::extract(root, source, &preparation.join("source"))?;
    crate::authoring::compose(
        root,
        &root.join("config/submenu-graphics-inputs.json"),
        &preparation.join("preview"),
    )?;
    crate::authoring::reinsert(
        root,
        &preparation.join("source/mainmenu.snt"),
        &preparation.join("preview/report.json"),
        &preparation.join("snt"),
    )?;
    let product = staging.path().join("product");
    let report = super::build(
        root,
        source,
        super::BuildOptions {
            snt_report: &preparation.join("snt/report.json"),
            output: &product,
            descriptions: true,
            expand_story_font: false,
            opening: false,
            story: true,
            fix_font_copy: true,
            ppsspp_root: Some(ppsspp_root),
            remove_install_data: true,
        },
    )?;
    // Preparation is reproducible scratch data; the staging guard removes it.
    ensure!(!output.exists(), "output appeared during build");
    fs::rename(product, output)?;
    Ok(report)
}
