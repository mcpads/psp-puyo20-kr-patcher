//! Read-only comparison population and current build adoption audit.
use crate::{
    sha256,
    source::{self, SourceProfile},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
mod export;

use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::Path,
};

fn read(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn array(v: &Value) -> Result<&Vec<Value>> {
    v.as_array().context("expected array")
}
fn number(v: &Value) -> Result<u64> {
    v.as_u64().context("expected number")
}
fn string(v: &Value) -> Result<&str> {
    v.as_str().context("expected string")
}
fn increment(counts: &mut BTreeMap<String, usize>, label: &str) {
    *counts.entry(label.into()).or_default() += 1;
}

fn member_kind(path: &str, graphic: Option<&Value>, whitespace_equal: bool) -> &'static str {
    if path.ends_with(".pss") {
        return if whitespace_equal {
            "script_whitespace_only"
        } else {
            "script_tokens_review"
        };
    }
    if path.ends_with(".snc") {
        return "layout_review";
    }
    if (path.ends_with(".gim") || path.ends_with(".snt"))
        && let Some(g) = graphic
    {
        if let Some(changes) = g["pixel_changes"].as_array().filter(|a| !a.is_empty()) {
            if !changes.is_empty() && changes.iter().all(|c| c["visible_equal"] == true) {
                return "graphics_visible_equal";
            }
            return "graphics_visible_changed";
        }
        return "graphics_slot_mapping_review";
    }
    "unclassified"
}

pub fn audit(
    root: &Path,
    source_path: &Path,
    product_path: &Path,
    inventory_path: &Path,
    graphics_path: &Path,
    build_path: &Path,
    output: &Path,
) -> Result<Value> {
    ensure!(!output.exists(), "output already exists");
    let inventory = read(inventory_path)?;
    let graphics = read(graphics_path)?;
    let build = read(build_path)?;
    let texts = read(&root.join("config/text-resources.json"))?;
    let inventory_hash = sha256(&fs::read(inventory_path)?);
    ensure!(
        texts["inventory_sha256"] == inventory_hash
            && graphics["inventory_sha256"] == inventory_hash,
        "inventory identity mismatch"
    );
    let profile: SourceProfile =
        serde_json::from_slice(&fs::read(root.join("config/source.json"))?)?;
    let en = read(&root.join("config/english-reference.json"))?;
    ensure!(
        inventory["source_sha256"][0] == profile.sha256
            && inventory["source_sha256"][1] == en["sha256"],
        "comparison source mismatch"
    );
    ensure!(
        build["source_sha256"] == profile.sha256,
        "build source mismatch"
    );
    let mut source = source::verify(source_path, &profile)?;
    let mut product = source::verify(
        product_path,
        &SourceProfile {
            size_bytes: number(&build["size_bytes"])?,
            sha256: string(&build["output_sha256"])?.into(),
        },
    )?;
    let menu_path = root.join("config/menu-text.json");
    let menu = read(&menu_path)?;
    ensure!(
        build["descriptions"]["config_sha256"] == sha256(&fs::read(&menu_path)?),
        "menu config differs from build"
    );
    let story_path = root.join("config/story-build.json");
    let story = read(&story_path)?;
    ensure!(
        build["story_manifest_sha256"] == sha256(&fs::read(&story_path)?),
        "story manifest differs from build"
    );
    let mut adopted = BTreeMap::new();
    for pair in array(&menu["pairs"])? {
        for r in array(&pair["resources"])? {
            adopted.insert(
                number(&r["offset"])?,
                json!({"translation":pair["translation"], "source_sha256":r["sha256"]}),
            );
        }
    }
    for (i, pair) in array(&story["pairs"])?.iter().enumerate() {
        let font_path = root.join(string(&pair["font"])?);
        let font = read(&font_path)?;
        ensure!(
            build["story_pairs"][i]["config_sha256"] == sha256(&fs::read(font_path)?),
            "story font config differs from build"
        );
        let original = array(&texts["pairs"])?
            .iter()
            .find(|p| p["name"] == font["pair"])
            .context("story text pair missing")?;
        for k in ["font", "text"] {
            adopted.insert(
                number(&original[k]["offset"])?,
                json!({"translation_config":pair["text"], "source_sha256":original[k]["sha256"]}),
            );
        }
    }
    let graphic_map: BTreeMap<_, _> = array(&graphics["members"])?
        .iter()
        .map(|g| {
            (
                (
                    g["container"].as_str().unwrap_or(""),
                    g["member"].as_str().unwrap_or(""),
                ),
                g,
            )
        })
        .collect();
    let mut resources = Vec::new();
    let mut members = Vec::new();
    let mut counts = BTreeMap::new();
    for r in array(&inventory["resources"])? {
        let path = string(&r["candidate_path"])?;
        let offset = number(&texts["game_offset"])? + number(&r["jp"]["offset"])?;
        let bytes = source::read_extent(
            &mut source,
            offset,
            number(&r["jp"]["size_bytes"])? as usize,
        )?;
        ensure!(
            sha256(&bytes) == r["jp"]["sha256"],
            "resource hash mismatch: {path}"
        );
        let adoption = adopted.get(&offset);
        if let Some(a) = adoption {
            ensure!(
                a["source_sha256"] == r["jp"]["sha256"],
                "adopted original mismatch"
            );
        }
        let changed = r["equal"] == false;
        let kind = if path.ends_with(".fnt") {
            "font"
        } else if path.ends_with(".mtx") {
            "text"
        } else if path.ends_with(".zip") || r["jp"]["signature"] == "ZIP" {
            "archive"
        } else if path.ends_with(".pss") {
            "script"
        } else {
            "other"
        };
        let mut modified_members = 0;
        if let Some(zmembers) = r["zip_members"].as_array() {
            let after = source::read_extent(&mut product, offset, bytes.len())?;
            let mut zip = zip::ZipArchive::new(Cursor::new(after))?;
            let mut original_zip = zip::ZipArchive::new(Cursor::new(&bytes))?;
            for m in zmembers.iter().filter(|m| m["equal"] == false) {
                let name = string(&m["path"])?;
                let mut original = Vec::new();
                original_zip.by_name(name)?.read_to_end(&mut original)?;
                ensure!(
                    sha256(&original) == m["jp"]["sha256"],
                    "member hash mismatch"
                );
                let mut current = Vec::new();
                zip.by_name(name)?.read_to_end(&mut current)?;
                let modified = sha256(&current) != m["jp"]["sha256"];
                modified_members += usize::from(modified);
                let g = graphic_map.get(&(path, name)).copied();
                if let Some(g) = g {
                    ensure!(
                        g["versions"]["jp"]["sha256"] == m["jp"]["sha256"]
                            && g["versions"]["en"]["sha256"] == m["en"]["sha256"],
                        "graphics identity mismatch"
                    );
                }
                let kind = member_kind(name, g, m["whitespace_normalized_equal"] == true);
                increment(&mut counts, kind);
                members.push(json!({"container":path,"member":name,"kind":kind,"jp_sha256":m["jp"]["sha256"],"en_sha256":m["en"]["sha256"],"current_sha256":sha256(&current),"current_modified":modified,"pixel_changes":g.map(|g| &g["pixel_changes"]),"scope_status":"review_required; byte change is not translation completion"}));
            }
        }
        resources.push(json!({"path":path,"hash_path":r["hash_path"],"occurrence":r["occurrence"],"kind":kind,"en_changed":changed,"jp_sha256":r["jp"]["sha256"],"en_sha256":r["en"]["sha256"],"adopted":adoption,"modified_changed_members":modified_members}));
    }
    let mut text_pairs = Vec::new();
    for p in array(&texts["pairs"])? {
        let mut row = json!({"path":p["name"]});
        for k in ["font", "text"] {
            let offset = number(&p[k]["offset"])?;
            let r = array(&inventory["resources"])?
                .iter()
                .find(|r| {
                    r["jp"]["offset"]
                        .as_u64()
                        .map(|o| o + texts["game_offset"].as_u64().unwrap_or(0))
                        == Some(offset)
                })
                .context("missing text resource")?;
            row[k] =
                json!({"en_changed":r["equal"]==false,"adopted":adopted.contains_key(&offset)});
        }
        text_pairs.push(row);
    }
    let summary = json!({"game_resources":resources.len(),"game_changed":resources.iter().filter(|r|r["en_changed"]==true).count(),"zip_changed_members":members.len(),"member_kinds":counts,"current_modified_changed_members":members.iter().filter(|m|m["current_modified"]==true).count(),"text_pairs":text_pairs.len(),"adopted_text_pairs":text_pairs.iter().filter(|p|p["font"]["adopted"]==true&&p["text"]["adopted"]==true).count()});
    let report = json!({"scope":"read-only static population; adoption and byte changes do not prove full translation, reachability or review","inventory_sha256":inventory_hash,"graphics_report_sha256":sha256(&fs::read(graphics_path)?),"build_report_sha256":sha256(&fs::read(build_path)?),"source_sha256":profile.sha256,"product_sha256":build["output_sha256"],"summary":summary,"resources":resources,"members":members,"text_pairs":text_pairs,"iso_files":inventory["iso_files"]});
    fs::create_dir_all(output)?;
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    export::write(output, &report)?;
    Ok(summary)
}

#[cfg(test)]
mod tests;
