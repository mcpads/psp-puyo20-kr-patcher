use super::*;

#[test]
fn pins_are_found_reported_and_updated_only_for_named_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(root.join("assets/a.json"), "old").unwrap();
    fs::write(root.join("assets/b.png"), "b").unwrap();
    let a = sha256(b"old");
    let b = sha256(b"b");
    let config = json!({
        "translation": "assets/a.json", "translation_sha256": a,
        "bg": {"path": "assets/b.png", "sha256": b},
        "member": {"path": "inside.zip/member", "sha256": b}
    });
    fs::write(
        root.join("config/x.json"),
        serde_json::to_string_pretty(&config).unwrap() + "\n",
    )
    .unwrap();
    assert_eq!(scan(root).unwrap().len(), 2);
    assert!(run(root, &[]).is_ok());
    fs::write(root.join("assets/a.json"), "new").unwrap();
    assert!(run(root, &[]).is_err());
    let report = run(root, &[PathBuf::from("assets/a.json")]).unwrap();
    assert_eq!(report["updated"].as_array().unwrap().len(), 1);
    let text = fs::read_to_string(root.join("config/x.json")).unwrap();
    assert!(text.contains(&sha256(b"new")) && !text.contains(&a));
}

#[test]
fn identical_files_keep_separate_pins() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("a.png"), "same").unwrap();
    fs::write(root.join("b.png"), "same").unwrap();
    let h = sha256(b"same");
    fs::write(
        root.join("config/g.json"),
        format!("{{\n  \"p\": [\n    {{\n      \"png\": \"a.png\",\n      \"png_sha256\": \"{h}\"\n    }},\n    {{\n      \"png\": \"b.png\",\n      \"png_sha256\": \"{h}\"\n    }}\n  ]\n}}\n"),
    )
    .unwrap();
    fs::write(root.join("a.png"), "changed").unwrap();
    run(root, &[PathBuf::from("a.png")]).unwrap();
    let doc: Value =
        serde_json::from_slice(&fs::read(root.join("config/g.json")).unwrap()).unwrap();
    assert_eq!(doc["p"][0]["png_sha256"], sha256(b"changed"));
    assert_eq!(doc["p"][1]["png_sha256"], h);
}

#[test]
fn adopt_adds_missing_pin_beside_reference() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("assets/a.json"), "{}").unwrap();
    std::fs::write(
        root.join("config/x.json"),
        "{\n  \"pairs\": [\n    {\n      \"translation\": \"assets/a.json\",\n      \"n\": 1\n    }\n  ]\n}\n",
    )
    .unwrap();
    let report = super::adopt(root, &["assets/a.json".into()]).unwrap();
    assert_eq!(report["adopted"].as_array().unwrap().len(), 1);
    let text = std::fs::read_to_string(root.join("config/x.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let keys: Vec<_> = doc["pairs"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, ["translation", "translation_sha256", "n"]);
    assert!(super::run(root, &[]).is_ok());
    assert!(super::adopt(root, &["assets/a.json".into()]).is_err());
}
