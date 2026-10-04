//! Translation inputs stored as a small index plus batch files of one MTX group each.
//!
//! A config names either one translation file (`"translation": path`) or a batched set:
//! `"translation": {"index": {path, sha256}, "batches": [{path, sha256}, ...]}`. A batch
//! holds `{"group": n, "part": k, "entries": [...]}` with at most [`BATCH_ENTRIES`] entries;
//! batches run in ascending (group, part) order and live beside the index, and every JSON
//! file in that directory must be listed. Loading rebuilds the single
//! document the pipeline has always consumed, so batching never changes build output.
use crate::{sha256, translation_edit::dump_like};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, fs, path::Path};

/// Largest batch, small enough to review one file at a time.
pub const BATCH_ENTRIES: usize = 32;

pub struct Loaded {
    pub doc: Value,
    /// Hash identity for build receipts.
    pub identity: Value,
}

fn pinned(root: &Path, item: &Value, what: &str) -> Result<(String, Vec<u8>)> {
    let path = item["path"]
        .as_str()
        .with_context(|| format!("{what} path"))?;
    let raw = fs::read(root.join(path)).with_context(|| path.to_string())?;
    ensure!(
        item["sha256"].as_str() == Some(sha256(&raw).as_str()),
        "{path} differs from its pinned hash"
    );
    Ok((path.to_string(), raw))
}

/// Load the translation named by `holder["translation"]`.
pub fn load(root: &Path, holder: &Value) -> Result<Loaded> {
    let spec = &holder["translation"];
    if let Some(path) = spec.as_str() {
        let raw = fs::read(root.join(path))?;
        ensure!(
            holder["translation_sha256"].as_str() == Some(sha256(&raw).as_str()),
            "{path} differs from its pinned hash or has no translation_sha256"
        );
        return Ok(Loaded {
            identity: json!(sha256(&raw)),
            doc: serde_json::from_slice(&raw)?,
        });
    }
    let (index_path, index_raw) = pinned(root, &spec["index"], "index")?;
    let mut doc: Map<String, Value> = serde_json::from_slice(&index_raw)?;
    ensure!(
        !doc.contains_key("entries"),
        "{index_path} must not hold entries"
    );
    let dir = Path::new(&index_path).parent().context("index directory")?;
    let batches = spec["batches"].as_array().context("batches")?;
    ensure!(!batches.is_empty(), "no translation batches");
    let mut listed = BTreeSet::from([index_path.clone()]);
    let mut entries = Vec::new();
    let mut identities = Vec::new();
    let mut previous = None;
    for item in batches {
        let (path, raw) = pinned(root, item, "batch")?;
        ensure!(
            Path::new(&path).parent() == Some(dir),
            "{path} outside {}",
            dir.display()
        );
        let batch: Value = serde_json::from_slice(&raw)?;
        let group = batch["group"]
            .as_u64()
            .with_context(|| format!("{path} group"))?;
        let part = batch["part"]
            .as_u64()
            .with_context(|| format!("{path} part"))?;
        let expected = match previous {
            Some((g, p)) if g == group => p + 1,
            Some((g, _)) => {
                ensure!(g < group, "{path} breaks ascending group order");
                0
            }
            None => 0,
        };
        ensure!(part == expected, "{path} should be part {expected}");
        previous = Some((group, part));
        let items = batch["entries"]
            .as_array()
            .with_context(|| format!("{path} entries"))?;
        ensure!(
            !items.is_empty() && items.len() <= BATCH_ENTRIES,
            "{path} must hold 1..={BATCH_ENTRIES} entries"
        );
        for e in items {
            ensure!(
                e["group"].as_u64() == Some(group),
                "{path} holds another group"
            );
        }
        entries.extend(items.iter().cloned());
        identities.push(json!({"path": path, "sha256": sha256(&raw)}));
        listed.insert(path);
    }
    for entry in fs::read_dir(root.join(dir))? {
        let name = entry?.file_name();
        let path = dir.join(&name).to_string_lossy().into_owned();
        ensure!(
            !path.ends_with(".json") || listed.contains(&path),
            "unlisted translation file {path}"
        );
    }
    doc.insert("entries".into(), Value::Array(entries));
    Ok(Loaded {
        doc: Value::Object(doc),
        identity: json!({"index_sha256": sha256(&index_raw), "batches": identities}),
    })
}

/// Split the single translation named at `pointer` in `config` into an index and group batches.
pub fn split(root: &Path, config_path: &Path, pointer: &str) -> Result<Value> {
    let config_text = fs::read_to_string(root.join(config_path))?;
    let mut config: Value = serde_json::from_str(&config_text)?;
    let holder = config
        .pointer_mut(pointer)
        .with_context(|| format!("no object at {pointer}"))?;
    let path = holder["translation"]
        .as_str()
        .context("translation is already batched")?
        .to_string();
    let file_text = fs::read_to_string(root.join(&path))?;
    let before = load(root, holder)?.doc;
    let Value::Object(mut index) = before.clone() else {
        bail!("{path} is not an object")
    };
    let entries = index.remove("entries").context("entries")?;
    let entries = entries.as_array().context("entries")?;
    let stem = Path::new(&path)
        .file_stem()
        .context("stem")?
        .to_string_lossy()
        .trim_end_matches("-aligned")
        .to_string();
    let dir = Path::new(&path).parent().context("parent")?.join(&stem);
    ensure!(
        !root.join(&dir).exists(),
        "{} already exists",
        dir.display()
    );
    let mut groups: Vec<(u64, Vec<Value>)> = Vec::new();
    for e in entries {
        let g = e["group"].as_u64().context("entry without group")?;
        match groups.last_mut() {
            Some((last, items)) if *last == g => items.push(e.clone()),
            Some((last, _)) if *last > g => bail!("{path} entries are not in group order"),
            _ => groups.push((g, vec![e.clone()])),
        }
    }
    // Render every new file with the source file's indent and final newline.
    let indent = file_text
        .lines()
        .nth(1)
        .map_or(2, |l| l.len() - l.trim_start().len());
    let render = |v: &Value| -> Result<String> {
        let pad = vec![b' '; indent];
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(&pad);
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        serde::Serialize::serialize(v, &mut ser)?;
        let mut out = String::from_utf8(buf)?;
        if file_text.ends_with('\n') {
            out.push('\n');
        }
        Ok(out)
    };
    fs::create_dir_all(root.join(&dir))?;
    let index_file = dir.join("index.json");
    let index_text = render(&Value::Object(index))?;
    fs::write(root.join(&index_file), &index_text)?;
    let mut batches = Vec::new();
    for (g, items) in &groups {
        for (part, chunk) in items.chunks(BATCH_ENTRIES).enumerate() {
            let file = dir.join(format!("group-{g:02}-{part}.json"));
            let text = render(&json!({"group": g, "part": part, "entries": chunk}))?;
            fs::write(root.join(&file), &text)?;
            batches.push(json!({"path": file, "sha256": sha256(text.as_bytes())}));
        }
    }
    let holder = config.pointer_mut(pointer).context("holder")?;
    let map = holder.as_object_mut().context("holder object")?;
    map.insert(
        "translation".into(),
        json!({"index": {"path": index_file, "sha256": sha256(index_text.as_bytes())}, "batches": batches}),
    );
    map.remove("translation_sha256");
    let after = load(root, config.pointer(pointer).context("holder")?)?.doc;
    ensure!(after == before, "split changed {path}");
    fs::write(root.join(config_path), dump_like(&config_text, &config)?)?;
    fs::remove_file(root.join(&path))?;
    Ok(
        json!({"source": path, "index": index_file, "groups": groups.len(), "batches": batches.len(), "entries": entries.len()}),
    )
}

#[cfg(test)]
mod tests;
