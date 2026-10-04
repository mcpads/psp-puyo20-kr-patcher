use super::*;

fn table(output: &Path, name: &str, header: &str, rows: Vec<Vec<String>>) -> Result<Value> {
    let mut text = format!("{header}\n");
    for row in rows {
        ensure!(
            row.iter().all(|x| !x.contains(['\t', '\n', '\r'])),
            "TSV delimiter in field"
        );
        text.push_str(&row.join("\t"));
        text.push('\n');
    }
    fs::write(output.join(name), &text)?;
    Ok(json!({"file":name,"sha256":sha256(text.as_bytes())}))
}
fn field(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}
pub(super) fn write(output: &Path, r: &Value) -> Result<()> {
    let mut files = Vec::new();
    files.push(table(
        output,
        "members.tsv",
        "container\tmember\tclassification\tcurrent_modified",
        array(&r["members"])?
            .iter()
            .map(|m| {
                ["container", "member", "kind", "current_modified"]
                    .iter()
                    .map(|k| field(&m[k]))
                    .collect()
            })
            .collect(),
    )?);
    files.push(table(output,"resources.tsv","path\thash_path\toccurrence\tkind\ten_changed\tadopted_text_input\tmodified_changed_members",array(&r["resources"])?.iter().map(|m|vec![field(&m["path"]),field(&m["hash_path"]),field(&m["occurrence"]),field(&m["kind"]),field(&m["en_changed"]),field(&m["adopted"]),field(&m["modified_changed_members"])]).collect())?);
    files.push(table(
        output,
        "text-pairs.tsv",
        "path\ten_font_changed\ten_text_changed\tfont_adopted\ttext_adopted",
        array(&r["text_pairs"])?
            .iter()
            .map(|m| {
                vec![
                    field(&m["path"]),
                    field(&m["font"]["en_changed"]),
                    field(&m["text"]["en_changed"]),
                    field(&m["font"]["adopted"]),
                    field(&m["text"]["adopted"]),
                ]
            })
            .collect(),
    )?);
    files.push(table(
        output,
        "iso-files.tsv",
        "path\ten_changed\tjp_sha256\ten_sha256",
        array(&r["iso_files"])?
            .iter()
            .map(|m| {
                vec![
                    field(&m["path"]),
                    field(&json!(m["equal"] == false)),
                    field(&m["jp"]["sha256"]),
                    field(&m["en"]["sha256"]),
                ]
            })
            .collect(),
    )?);
    let receipt = json!({"scope":r["scope"],"inventory_sha256":r["inventory_sha256"],"graphics_report_sha256":r["graphics_report_sha256"],"build_report_sha256":r["build_report_sha256"],"source_sha256":r["source_sha256"],"product_sha256":r["product_sha256"],"summary":r["summary"],"tables":files});
    fs::write(
        output.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(())
}
