//! Fixed-source menu archive inspection, extraction and ISO build entry points.
mod build;
mod current;
mod relocations;
use crate::{
    sha256,
    source::{self, SourceProfile},
};
use anyhow::{Context, Result, ensure};
pub use build::{BuildOptions, build};
pub use current::build_current;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Read},
    path::Path,
};

#[derive(Deserialize)]
struct Layout {
    game_iso_offset: u64,
    game_size: u64,
    zip_size: usize,
    identical_archive_offsets: Vec<u64>,
}
fn load<T: serde::de::DeserializeOwned>(p: &Path) -> Result<T> {
    Ok(serde_json::from_slice(
        &fs::read(p).with_context(|| format!("read {}", p.display()))?,
    )?)
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().with_context(|| format!("missing {key}"))
}

struct MenuSource {
    source: fs::File,
    profile: SourceProfile,
    manifest_bytes: Vec<u8>,
    manifest: Value,
    layout: Layout,
    offset: u64,
    before: Vec<u8>,
    members: Vec<Value>,
}
fn read(root: &Path, source_path: &Path) -> Result<MenuSource> {
    let profile: SourceProfile = load(&root.join("config/source.json"))?;
    let mut source = source::verify(source_path, &profile)?;
    let manifest_bytes = fs::read(root.join("config/graphics/main-menu.json"))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    let layout: Layout = load(&root.join("config/menu-build.json"))?;
    let relative = u64::from_str_radix(
        field(&manifest["container"], "jp_game_offset")?.trim_start_matches("0x"),
        16,
    )?;
    ensure!(
        relative
            .checked_add(layout.zip_size as u64)
            .context("extent overflow")?
            <= layout.game_size,
        "archive outside GAME"
    );
    let offset = layout
        .game_iso_offset
        .checked_add(relative)
        .context("ISO offset overflow")?;
    let before = source::read_extent(&mut source, offset, layout.zip_size)?;
    ensure!(
        sha256(&before) == field(&manifest["container"], "sha256")?,
        "archive identity mismatch"
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(&before))?;
    let mut members = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        ensure!(f.size() <= 128 * 1024 * 1024, "member too large");
        let mut b = Vec::new();
        f.read_to_end(&mut b)?;
        members.push(json!({"name":f.name(),"size":b.len(),"compressed_size":f.compressed_size(),"sha256":sha256(&b)}));
    }
    drop(archive);
    Ok(MenuSource {
        source,
        profile,
        manifest_bytes,
        manifest,
        layout,
        offset,
        before,
        members,
    })
}

pub fn inspect(root: &Path, source_path: &Path) -> Result<Value> {
    let menu = read(root, source_path)?;
    Ok(json!({"zip_sha256":sha256(&menu.before),"members":menu.members}))
}

pub fn extract(root: &Path, source_path: &Path, output: &Path) -> Result<Value> {
    let MenuSource {
        profile,
        manifest_bytes,
        manifest,
        before,
        ..
    } = read(root, source_path)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(&before))?;
    ensure!(!output.exists(), "output already exists");
    let mut snt = Vec::new();
    archive
        .by_name(field(&manifest["member"], "path")?)?
        .read_to_end(&mut snt)?;
    ensure!(
        sha256(&snt) == field(&manifest["member"], "sha256")?,
        "SNT identity mismatch"
    );
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = tempfile::Builder::new()
        .prefix(".menu-extract-")
        .tempdir_in(parent)?;
    let mut textures = Vec::new();
    for (slot, (offset, size)) in crate::graphics::snt_table(&snt)?.into_iter().enumerate() {
        let gim = crate::graphics::bytes(&snt, offset, size)?;
        let image = crate::graphics::decode(gim)?;
        fs::write(temp.path().join(format!("{slot:03}.gim")), gim)?;
        crate::authoring::png_write(&temp.path().join(format!("{slot:03}.png")), &image)?;
        textures.push(json!({"slot":slot,"offset":offset,"size":size,"gim_sha256":sha256(gim),"rgba_sha256":sha256(&image.rgba)}));
    }
    fs::write(temp.path().join("mainmenu.snt"), &snt)?;
    let report = json!({"source_sha256":profile.sha256,"manifest_sha256":sha256(&manifest_bytes),"snt_sha256":sha256(&snt),"textures":textures});
    fs::write(
        temp.path().join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    ensure!(!output.exists(), "output appeared during extraction");
    fs::rename(temp.path(), output)?;
    Ok(report)
}
