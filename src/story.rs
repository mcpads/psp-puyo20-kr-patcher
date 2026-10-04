//! Pinned source-script context for translation preparation, not a PSS interpreter.
use crate::{sha256, source, text};
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
struct Member {
    path: String,
    size: usize,
    sha256: String,
}
#[derive(Deserialize)]
struct Archive {
    name: String,
    #[serde(flatten)]
    extent: text::Extent,
    members: Vec<Member>,
}
#[derive(Deserialize)]
struct Catalog {
    source_sha256: String,
    archives: Vec<Archive>,
    general_scripts: Vec<GeneralScript>,
    unmapped_general_scripts: Vec<Value>,
}

#[derive(Deserialize)]
struct GeneralScript {
    name: String,
    #[serde(flatten)]
    extent: text::Extent,
    text_pair: String,
    group: usize,
}

use commands::{candidate_record, scan_calls, scan_commands};
mod commands;

fn read_verified(
    file: &mut fs::File,
    extent: &text::Extent,
    catalog: &text::Catalog,
) -> Result<Vec<u8>> {
    ensure!(
        extent.size <= 16 * 1024 * 1024
            && extent.offset >= catalog.game_offset
            && extent
                .offset
                .checked_add(extent.size as u64)
                .context("extent overflow")?
                <= catalog
                    .game_offset
                    .checked_add(catalog.game_size)
                    .context("GAME overflow")?,
        "story input outside GAME"
    );
    let bytes = source::read_extent(file, extent.offset, extent.size)?;
    ensure!(sha256(&bytes) == extent.sha256, "story input hash mismatch");
    Ok(bytes)
}

pub fn extract(root: &Path, iso: &Path, output: &Path) -> Result<Value> {
    ensure!(!output.exists(), "output already exists");
    let profile: source::SourceProfile =
        serde_json::from_slice(&fs::read(root.join("config/source.json"))?)?;
    let catalog_bytes = fs::read(root.join("config/story-scripts.json"))?;
    let catalog: Catalog = serde_json::from_slice(&catalog_bytes)?;
    let text_catalog_bytes = fs::read(root.join("config/text-resources.json"))?;
    let text_catalog: text::Catalog = serde_json::from_slice(&text_catalog_bytes)?;
    ensure!(
        catalog.source_sha256 == profile.sha256
            && text_catalog.source_sha256 == profile.sha256
            && catalog.archives.len() == 25,
        "story catalog identity/population mismatch"
    );
    let mut source_file = source::verify(iso, &profile)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = tempfile::tempdir_in(parent)?;
    let mut routes = Vec::new();
    let mut seen_routes = BTreeSet::new();
    let (mut script_count, mut call_count, mut mapped, mut symbolic) = (0, 0, 0, 0);
    for archive in &catalog.archives {
        let route = archive
            .name
            .strip_prefix("script/story_demo/")
            .and_then(|s| s.strip_suffix(".zip"))
            .context("invalid story archive name")?;
        ensure!(
            !route.is_empty()
                && route
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                && seen_routes.insert(route),
            "unsafe/duplicate story route"
        );
        let pair_name = format!("text/story_demo/{route}");
        let pairs: Vec<_> = text_catalog
            .pairs
            .iter()
            .filter(|p| p.name == pair_name)
            .collect();
        ensure!(pairs.len() == 1, "missing/ambiguous story text pair");
        let pair = pairs[0];
        let font = read_verified(&mut source_file, &pair.font, &text_catalog)?;
        let raw_text = read_verified(&mut source_file, &pair.text, &text_catalog)?;
        let text_packet = text::inspect_pair(&font, &raw_text)?;
        let mtx = text::Mtx::parse(&raw_text)?;
        let raw_zip = read_verified(&mut source_file, &archive.extent, &text_catalog)?;
        let mut zip = zip::ZipArchive::new(Cursor::new(raw_zip))?;
        ensure!(
            zip.len() == archive.members.len() && zip.len() == 16,
            "story ZIP member population mismatch"
        );
        let dir = temp.path().join(route);
        fs::create_dir(&dir)?;
        fs::write(dir.join("text.json"), serde_json::to_vec(&text_packet)?)?;
        let mut scripts = Vec::new();
        let mut referenced = BTreeSet::new();
        let mut seen_members = BTreeSet::new();
        for member in &archive.members {
            ensure!(
                member.path.ends_with(".pss")
                    && member
                        .path
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
                    && seen_members.insert(&member.path),
                "unsafe/duplicate PSS member name"
            );
            let mut raw = Vec::new();
            zip.by_name(&member.path)?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut raw)?;
            ensure!(
                raw.len() == member.size
                    && raw.len() <= 1024 * 1024
                    && sha256(&raw) == member.sha256,
                "PSS member identity mismatch"
            );
            let mut calls = scan_calls(&raw)?;
            for call in &mut calls {
                call_count += 1;
                match candidate_record(call, &mtx)? {
                    Some((group, record, index)) => {
                        mapped += 1;
                        referenced.insert(index);
                        call["mapping"] = json!({"status":"literal_candidate",
                            "id":format!("{pair_name}/g{group:02}/r{record:04}"),
                            "group":group,"record":record,"flat_record":index,
                            "record_offset":mtx.records[index].offset,
                            "source":text_packet["strings"][index],
                            "speaker":null,"translation_eligible":false,
                            "remaining":"Confirm consumer argument meanings, speaker and context before translation."});
                    }
                    None => {
                        symbolic += 1;
                        call["mapping"] = json!({"status":"symbolic_unresolved"});
                    }
                }
            }
            script_count += 1;
            fs::write(dir.join(&member.path), &raw)?;
            let packet = json!({"script":member.path,"sha256":member.sha256,
                "scope":"Source order and candidate text references only; not execution order or speaker proof.",
                "calls":calls,"context_events":scan_commands(&raw, true)?});
            let bytes = serde_json::to_vec(&packet)?;
            fs::write(dir.join(format!("{}.json", member.path)), &bytes)?;
            scripts.push(json!({"script":member.path,"source_sha256":member.sha256,
                "packet_sha256":sha256(&bytes),"calls":calls.len()}));
        }
        let unreferenced: Vec<_> = (0..mtx.records.len())
            .filter(|i| !referenced.contains(i))
            .collect();
        routes.push(json!({"route":route,"archive_sha256":archive.extent.sha256,
            "font_sha256":pair.font.sha256,"text_sha256":pair.text.sha256,
            "records":mtx.records.len(),"literal_referenced_records":referenced.len(),
            "records_without_literal_reference":unreferenced,"scripts":scripts}));
    }
    let general_dir = temp.path().join("general");
    fs::create_dir(&general_dir)?;
    let mut general_receipts = Vec::new();
    let mut general_calls = 0;
    let mut general_referenced = BTreeSet::new();
    let mut general_records = None;
    for script in &catalog.general_scripts {
        let filename = script
            .name
            .strip_prefix("script/story_demo/general/")
            .context("general script path")?;
        ensure!(
            ["opening.pss", "ending.pss"].contains(&filename)
                && script.text_pair == "text/story_demo/general",
            "unadopted general script association"
        );
        let pair = text_catalog
            .pairs
            .iter()
            .find(|p| p.name == script.text_pair)
            .context("general text pair missing")?;
        let font = read_verified(&mut source_file, &pair.font, &text_catalog)?;
        let raw_text = read_verified(&mut source_file, &pair.text, &text_catalog)?;
        let mtx = text::Mtx::parse(&raw_text)?;
        let packet = text::inspect_pair(&font, &raw_text)?;
        general_records = Some(mtx.records.len());
        let raw = read_verified(&mut source_file, &script.extent, &text_catalog)?;
        let mut calls = scan_calls(&raw)?;
        for call in &mut calls {
            let (group, record, index) = candidate_record(call, &mtx)?
                .context("general script requires literal reference")?;
            ensure!(group == script.group, "general script group drift");
            ensure!(
                general_referenced.insert(index),
                "duplicate general reference"
            );
            call["mapping"] = json!({"status":"source_consumer_index",
                "id":format!("{}/g{group:02}/r{record:04}",script.text_pair),
                "group":group,"record":record,"flat_record":index,
                "record_offset":mtx.records[index].offset,"source":packet["strings"][index],
                "speaker":null,"translation_eligible":false,
                "remaining":"Resolve scene speaker and control policies per translation unit."});
        }
        general_calls += calls.len();
        fs::write(general_dir.join(filename), &raw)?;
        let bytes = serde_json::to_vec(&json!({"script":filename,"sha256":script.extent.sha256,
            "scope":"General script indexed references; source order only.","calls":calls,"context_events":scan_commands(&raw, true)?}))?;
        fs::write(general_dir.join(format!("{filename}.json")), &bytes)?;
        fs::write(general_dir.join("text.json"), serde_json::to_vec(&packet)?)?;
        general_receipts.push(
            json!({"script":script.name,"source_sha256":script.extent.sha256,
            "packet_sha256":sha256(&bytes),"text_pair":script.text_pair,
            "text_sha256":pair.text.sha256,"font_sha256":pair.font.sha256,
            "group":script.group,"calls":calls.len()}),
        );
    }
    ensure!(general_receipts.len() == 2, "general script population");
    let general_unreferenced: Vec<_> = (0..general_records.context("general records missing")?)
        .filter(|i| !general_referenced.contains(i))
        .collect();
    source::verify(iso, &profile)?;
    let report = json!({"scope":"25 story route archives plus opening and ending. Route candidates, three independent scripts and full control meanings remain unresolved.",
        "source_sha256":profile.sha256,"catalog_sha256":sha256(&catalog_bytes),
        "text_catalog_sha256":sha256(&text_catalog_bytes),
        "totals":{"routes":routes.len(),"scripts":script_count,"calls":call_count,
            "literal_candidates":mapped,"symbolic_unresolved":symbolic},
        "routes":routes,"general":{"scripts":general_receipts,"calls":general_calls,"records":general_records,"records_without_reference":general_unreferenced},"unmapped_general_scripts":catalog.unmapped_general_scripts});
    fs::write(
        temp.path().join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    ensure!(!output.exists(), "output appeared during extraction");
    fs::rename(temp.path(), output)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
