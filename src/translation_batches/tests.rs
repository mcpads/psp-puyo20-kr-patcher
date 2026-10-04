use super::{load, split};
use crate::sha256;
use serde_json::{Value, json};
use std::{fs, path::Path};

fn project(entries: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let doc = json!({"scope": "test", "entries": entries});
    let text = serde_json::to_string_pretty(&doc).unwrap() + "\n";
    fs::create_dir_all(dir.path().join("assets")).unwrap();
    fs::write(dir.path().join("assets/story-aligned.json"), &text).unwrap();
    let config = json!({"translation": "assets/story-aligned.json",
        "translation_sha256": sha256(text.as_bytes())});
    fs::write(
        dir.path().join("c.json"),
        serde_json::to_string_pretty(&config).unwrap() + "\n",
    )
    .unwrap();
    dir
}

fn entries() -> Value {
    json!([{"group": 0, "record": 0, "t": "a"}, {"group": 0, "record": 1, "t": "b"},
           {"group": 2, "record": 0, "t": "c"}])
}

fn config(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("c.json")).unwrap()).unwrap()
}

#[test]
fn split_round_trips_and_pins_every_batch() {
    let dir = project(entries());
    let root = dir.path();
    let before = load(root, &config(root)).unwrap().doc;
    split(root, Path::new("c.json"), "").unwrap();
    assert!(!root.join("assets/story-aligned.json").exists());
    let after = config(root);
    assert_eq!(after["translation"]["batches"].as_array().unwrap().len(), 2);
    assert!(after.get("translation_sha256").is_none());
    assert_eq!(load(root, &after).unwrap().doc, before);
}

#[test]
fn rejects_drift_and_unlisted_files() {
    let dir = project(entries());
    let root = dir.path();
    split(root, Path::new("c.json"), "").unwrap();
    fs::write(root.join("assets/story/stray.json"), "{}").unwrap();
    assert!(load(root, &config(root)).is_err());
    fs::remove_file(root.join("assets/story/stray.json")).unwrap();
    assert!(load(root, &config(root)).is_ok());
    fs::write(root.join("assets/story/group-02-0.json"), "{}").unwrap();
    assert!(load(root, &config(root)).is_err());
}

#[test]
fn refuses_entries_out_of_group_order() {
    let dir = project(json!([{"group": 1, "record": 0}, {"group": 0, "record": 0}]));
    assert!(split(dir.path(), Path::new("c.json"), "").is_err());
    assert!(dir.path().join("assets/story-aligned.json").exists());
}

#[test]
fn caps_batch_size_with_numbered_parts() {
    let many: Vec<Value> = (0..70).map(|r| json!({"group": 3, "record": r})).collect();
    let dir = project(Value::Array(many));
    let root = dir.path();
    let before = load(root, &config(root)).unwrap().doc;
    let receipt = split(root, Path::new("c.json"), "").unwrap();
    assert_eq!(receipt["batches"], 3);
    assert!(root.join("assets/story/group-03-2.json").exists());
    assert_eq!(load(root, &config(root)).unwrap().doc, before);
}
