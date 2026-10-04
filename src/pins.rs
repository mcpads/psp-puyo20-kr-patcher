//! Hash pins in `config/` that name repository files.
//!
//! A pin is either a `"<key>": path` string with a sibling `"<key>_sha256"`, or an object
//! with `"path"` and `"sha256"`. Values that do not resolve to a repository file (ZIP member
//! names, ISO paths) are not pins. Paths resolve from the repository root first, then from
//! the config file's directory. Updating is explicit: only pins whose file was named change,
//! each at its own JSON location, and the config keeps its layout.
use crate::{sha256, translation_edit::dump_like};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct Pin {
    pub config: PathBuf,
    /// JSON Pointer of the pinned hash value.
    pub pointer: String,
    pub key: String,
    pub file: PathBuf,
    pub pinned: String,
    pub actual: String,
}

fn resolve(root: &Path, config_dir: &Path, p: &str) -> Option<PathBuf> {
    [root.join(p), config_dir.join(p)]
        .into_iter()
        .find(|c| c.is_file())
        .and_then(|c| c.canonicalize().ok())
}

fn escape(k: &str) -> String {
    k.replace('~', "~0").replace('/', "~1")
}

fn visit(v: &Value, at: &str, root: &Path, config: &Path, out: &mut Vec<Pin>) -> Result<()> {
    let dir = config.parent().context("config dir")?;
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                let Some(p) = val.as_str() else { continue };
                let sha_key = if k == "path" {
                    "sha256".to_string()
                } else {
                    format!("{k}_sha256")
                };
                if let (Some(pinned), Some(file)) = (
                    map.get(&sha_key).and_then(Value::as_str),
                    resolve(root, dir, p),
                ) {
                    out.push(Pin {
                        config: config.to_path_buf(),
                        pointer: format!("{at}/{}", escape(&sha_key)),
                        key: k.clone(),
                        actual: sha256(&fs::read(&file)?),
                        file,
                        pinned: pinned.to_string(),
                    });
                }
            }
            for (k, val) in map {
                visit(val, &format!("{at}/{}", escape(k)), root, config, out)?;
            }
        }
        Value::Array(items) => {
            for (i, val) in items.iter().enumerate() {
                visit(val, &format!("{at}/{i}"), root, config, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn scan(root: &Path) -> Result<Vec<Pin>> {
    let mut configs = Vec::new();
    let mut stack = vec![root.join("config")];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                configs.push(path);
            }
        }
    }
    configs.sort();
    let mut pins = Vec::new();
    for config in configs {
        let value: Value = serde_json::from_slice(&fs::read(&config)?)
            .with_context(|| config.display().to_string())?;
        visit(&value, "", root, &config, &mut pins)?;
    }
    Ok(pins)
}

/// Report all pins; rewrite drifted pins whose file is in `update`. Fails if drift remains.
pub fn run(root: &Path, update: &[PathBuf]) -> Result<Value> {
    let targets: Vec<PathBuf> = update
        .iter()
        .map(|p| {
            root.join(p)
                .canonicalize()
                .with_context(|| format!("update target {}", p.display()))
        })
        .collect::<Result<_>>()?;
    let base = root.canonicalize()?;
    let rel = |p: &Path| p.strip_prefix(&base).unwrap_or(p).display().to_string();
    let pins = scan(root)?;
    let mut edits: BTreeMap<PathBuf, Vec<&Pin>> = BTreeMap::new();
    let mut updated = Vec::new();
    let mut drifted = Vec::new();
    for pin in pins.iter().filter(|p| p.pinned != p.actual) {
        let record = json!({"config":rel(&pin.config.canonicalize()?),"pointer":pin.pointer,"key":pin.key,
            "file":rel(&pin.file),"pinned":pin.pinned,"actual":pin.actual});
        if targets.contains(&pin.file) {
            edits.entry(pin.config.clone()).or_default().push(pin);
            updated.push(record);
        } else {
            drifted.push(record);
        }
    }
    for (config, list) in &edits {
        let text = fs::read_to_string(config)?;
        let mut doc: Value = serde_json::from_str(&text)?;
        for pin in list {
            let slot = doc.pointer_mut(&pin.pointer).context("pin location")?;
            ensure!(
                slot.as_str() == Some(pin.pinned.as_str()),
                "pin changed during update"
            );
            *slot = json!(pin.actual);
        }
        fs::write(config, dump_like(&text, &doc)?)?;
    }
    let report = json!({"pins":pins.len(),"updated":updated,"drifted":drifted});
    ensure!(
        drifted.is_empty(),
        "{} drifted pins: {}",
        drifted.len(),
        serde_json::to_string_pretty(&report)?
    );
    Ok(report)
}

#[cfg(test)]
mod tests;

/// Add missing pins: every config string naming one of `files` gets a sibling
/// `<key>_sha256` (or `sha256` beside `path`) holding the file's current hash.
pub fn adopt(root: &Path, files: &[PathBuf]) -> Result<Value> {
    let targets: Vec<PathBuf> = files
        .iter()
        .map(|p| {
            root.join(p)
                .canonicalize()
                .with_context(|| format!("adopt target {}", p.display()))
        })
        .collect::<Result<_>>()?;
    fn add(
        v: &mut Value,
        root: &Path,
        dir: &Path,
        targets: &[PathBuf],
        added: &mut Vec<String>,
    ) -> Result<()> {
        match v {
            Value::Object(map) => {
                let mut inserts = Vec::new();
                for (i, (k, val)) in map.iter().enumerate() {
                    let Some(p) = val.as_str() else { continue };
                    let sha_key = if k == "path" {
                        "sha256".to_string()
                    } else {
                        format!("{k}_sha256")
                    };
                    if map.contains_key(&sha_key) {
                        continue;
                    }
                    if let Some(file) = resolve(root, dir, p).filter(|f| targets.contains(f)) {
                        inserts.push((i + 1, sha_key, sha256(&fs::read(&file)?)));
                        added.push(p.to_string());
                    }
                }
                for (i, key, hash) in inserts.into_iter().rev() {
                    map.shift_insert(i, key, json!(hash));
                }
                for val in map.values_mut() {
                    add(val, root, dir, targets, added)?;
                }
            }
            Value::Array(items) => {
                for val in items {
                    add(val, root, dir, targets, added)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut added = Vec::new();
    let mut stack = vec![root.join("config")];
    let mut configs = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                configs.push(path);
            }
        }
    }
    configs.sort();
    for config in configs {
        let text = fs::read_to_string(&config)?;
        let mut doc: Value = serde_json::from_str(&text)?;
        let before = added.len();
        add(
            &mut doc,
            root,
            config.parent().context("config dir")?,
            &targets,
            &mut added,
        )?;
        if added.len() > before {
            fs::write(&config, dump_like(&text, &doc)?)?;
        }
    }
    ensure!(
        !added.is_empty(),
        "no unpinned reference to the named files"
    );
    Ok(json!({"adopted": added}))
}
