use crate::{
    graphics, sha256,
    write_plan::{self, ExpectedWrite},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::HashSet, fs, io::Cursor, path::Path};

pub fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().with_context(|| format!("missing string {k}"))
}
fn number(v: &Value, k: &str) -> Result<usize> {
    Ok(v[k]
        .as_u64()
        .with_context(|| format!("missing number {k}"))?
        .try_into()?)
}
fn array<'a>(v: &'a Value, k: &str) -> Result<&'a Vec<Value>> {
    v[k].as_array().with_context(|| format!("missing list {k}"))
}
pub fn png_read(path: &Path) -> Result<graphics::Image> {
    let raw = fs::read(path)?;
    let mut decoder = png::Decoder::new(Cursor::new(raw));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let n = reader.output_buffer_size().context("PNG overflow")?;
    ensure!(n <= 64 * 1024 * 1024, "PNG too large");
    let mut buffer = vec![0; n];
    let info = reader.next_frame(&mut buffer)?;
    let data = &buffer[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|v| [*v, *v, *v, 255]).collect(),
        png::ColorType::GrayscaleAlpha => data
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        _ => anyhow::bail!("unsupported PNG"),
    };
    ensure!(
        info.bit_depth == png::BitDepth::Eight,
        "unsupported PNG depth"
    );
    Ok(graphics::Image {
        width: info.width as usize,
        height: info.height as usize,
        format: 3,
        order: 0,
        rgba,
    })
}
pub fn png_write(path: &Path, im: &graphics::Image) -> Result<()> {
    let mut enc = png::Encoder::new(
        fs::File::create(path)?,
        im.width.try_into()?,
        im.height.try_into()?,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&im.rgba)?;
    Ok(())
}
fn rect(r: &Value, w: usize, h: usize) -> Result<[usize; 4]> {
    let v = array(r, "rect_xywh")?;
    ensure!(v.len() == 4, "invalid rectangle");
    let mut a = [0usize; 4];
    for (i, x) in v.iter().enumerate() {
        a[i] = x.as_u64().context("bad coordinate")?.try_into()?;
    }
    ensure!(
        a[2] > 0
            && a[3] > 0
            && a[0].checked_add(a[2]).context("x overflow")? <= w
            && a[1].checked_add(a[3]).context("y overflow")? <= h,
        "region outside image"
    );
    Ok(a)
}
fn mask(s: &Value, w: usize, h: usize, chosen: &HashSet<String>) -> Result<Vec<bool>> {
    let mut mask = vec![false; w * h];
    let mut seen = HashSet::new();
    for r in array(s, "regions")? {
        let id = text(r, "id")?;
        if !chosen.contains(id) {
            continue;
        }
        ensure!(seen.insert(id.to_string()), "duplicate region");
        let [x, y, rw, rh] = rect(r, w, h)?;
        for yy in y..y + rh {
            for xx in x..x + rw {
                ensure!(!mask[yy * w + xx], "overlapping regions");
                mask[yy * w + xx] = true;
            }
        }
    }
    ensure!(seen == *chosen, "unknown region");
    Ok(mask)
}
fn check_pixels(before: &[u8], after: &[u8], allowed: &[bool]) -> Result<usize> {
    ensure!(
        before.len() == after.len() && before.len() == allowed.len() * 4,
        "pixel length mismatch"
    );
    let mut changed = 0;
    for (i, (a, b)) in before
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.as_chunks::<4>().0.iter())
        .enumerate()
    {
        if a != b {
            ensure!(allowed[i], "protected pixel changed at index {i}");
            changed += 1;
        }
    }
    Ok(changed)
}
fn context(root: &Path) -> Result<(Vec<u8>, Value, String)> {
    let raw = fs::read(root.join("config/graphics/main-menu.json"))?;
    let m: Value = serde_json::from_slice(&raw)?;
    let thash = sha256(&fs::read(root.join(text(&m, "translation_file")?))?);
    Ok((raw, m, thash))
}

pub fn compose(root: &Path, input: &Path, output: &Path) -> Result<Value> {
    ensure!(!output.exists(), "output exists");
    let (raw, m, thash) = context(root)?;
    let packet: Value = serde_json::from_slice(&fs::read(input)?)?;
    ensure!(
        text(&packet, "manifest_sha256")? == sha256(&raw)
            && text(&packet, "translation_sha256")? == thash,
        "stale authoring packet"
    );
    // Optional explicit paths let `pins` track these hashes; they must name the real inputs.
    ensure!(
        packet
            .get("manifest")
            .is_none_or(|v| v == "config/graphics/main-menu.json")
            && packet
                .get("translation")
                .is_none_or(|v| *v == m["translation_file"]),
        "authoring packet names other inputs"
    );
    let base = input.parent().context("input parent")?;
    let mut grouped = std::collections::BTreeMap::<String, Vec<&Value>>::new();
    for e in array(&packet, "inputs")? {
        grouped
            .entry(text(e, "surface")?.into())
            .or_default()
            .push(e);
    }
    let mut results = Vec::new();
    let mut images = Vec::new();
    for (sid, entries) in grouped {
        let s = array(&m, "surfaces")?
            .iter()
            .find(|s| s["id"] == sid)
            .context("unknown surface")?;
        let source = png_read(&root.join(text(s, "source_png")?))?;
        ensure!(
            s["size"] == json!([source.width, source.height]),
            "source dimensions mismatch"
        );
        ensure!(
            sha256(&source.rgba) == text(s, "rgba_sha256")?,
            "source RGBA mismatch"
        );
        let sheet = entries.iter().any(|e| e["mode"] == "sheet");
        ensure!(!sheet || entries.len() == 1, "conflicting input modes");
        let mut out = graphics::Image {
            rgba: source.rgba.clone(),
            ..source
        };
        let mut chosen = HashSet::new();
        let mut hashes = Vec::new();
        for e in entries {
            let path = base.join(text(e, "path")?);
            let hash = sha256(&fs::read(&path)?);
            ensure!(
                e.get("sha256").is_none_or(|expected| expected == &hash),
                "authoring input hash mismatch: {}",
                path.display()
            );
            hashes.push(hash);
            let target = png_read(&path)?;
            match text(e, "mode")? {
                "sheet" => {
                    ensure!(
                        target.width == out.width && target.height == out.height,
                        "sheet dimensions mismatch"
                    );
                    out.rgba = target.rgba;
                    for rid in array(e, "regions")? {
                        ensure!(
                            chosen.insert(rid.as_str().context("bad region id")?.to_string()),
                            "duplicate writer"
                        );
                    }
                }
                "region" => {
                    let rid = text(e, "region")?;
                    ensure!(chosen.insert(rid.to_string()), "duplicate writer");
                    let r = array(s, "regions")?
                        .iter()
                        .find(|r| r["id"] == rid)
                        .context("unknown region")?;
                    let [x, y, w, h] = rect(r, out.width, out.height)?;
                    ensure!(
                        target.width == w && target.height == h,
                        "crop dimensions mismatch"
                    );
                    for yy in 0..h {
                        let dst = ((y + yy) * out.width + x) * 4;
                        out.rgba[dst..dst + w * 4]
                            .copy_from_slice(&target.rgba[yy * w * 4..(yy + 1) * w * 4]);
                    }
                }
                _ => anyhow::bail!("unknown authoring mode"),
            }
        }
        ensure!(!chosen.is_empty(), "empty edit set");
        let allowed = mask(s, out.width, out.height, &chosen)?;
        let original = png_read(&root.join(text(s, "source_png")?))?;
        let changed = check_pixels(&original.rgba, &out.rgba, &allowed)?;
        results.push(json!({"surface":sid,"before_rgba_sha256":sha256(&original.rgba),"after_rgba_sha256":sha256(&out.rgba),"changed_pixels":changed,"input_sha256":hashes}));
        images.push((sid, out));
    }
    fs::create_dir_all(output)?;
    for (sid, im) in images {
        png_write(&output.join(format!("{sid}.png")), &im)?;
    }
    let report = json!({"scope":"RGBA authoring preview only","manifest_sha256":sha256(&raw),"translation_sha256":thash,"results":results});
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}
pub fn reinsert(
    root: &Path,
    source_path: &Path,
    preview_path: &Path,
    output: &Path,
) -> Result<Value> {
    ensure!(!output.exists(), "output exists");
    let (raw, m, thash) = context(root)?;
    let source = fs::read(source_path)?;
    ensure!(
        sha256(&source) == text(&m["member"], "sha256")?,
        "SNT source mismatch"
    );
    let report_raw = fs::read(preview_path)?;
    let report: Value = serde_json::from_slice(&report_raw)?;
    ensure!(
        text(&report, "manifest_sha256")? == sha256(&raw)
            && text(&report, "translation_sha256")? == thash,
        "stale preview"
    );
    let parts = graphics::snt_table(&source)?;
    let mut writes = Vec::new();
    let mut ids = HashSet::new();
    for r in array(&report, "results")? {
        let sid = text(r, "surface")?;
        ensure!(ids.insert(sid), "duplicate surface");
        let s = array(&m, "surfaces")?
            .iter()
            .find(|s| s["id"] == sid)
            .context("unknown surface")?;
        let &(off, size) = parts.get(number(s, "slot")?).context("bad texture slot")?;
        let before = graphics::bytes(&source, off, size)?;
        ensure!(
            off == usize::from_str_radix(text(s, "snt_offset")?.trim_start_matches("0x"), 16)?
                && size == number(s, "gim_size")?
                && sha256(before) == text(s, "gim_sha256")?,
            "GIM binding mismatch"
        );
        let old = graphics::decode(before)?;
        ensure!(
            sha256(&old.rgba) == text(s, "rgba_sha256")?
                && sha256(&old.rgba) == text(r, "before_rgba_sha256")?,
            "original RGBA mismatch"
        );
        let image = png_read(
            &preview_path
                .parent()
                .context("preview parent")?
                .join(format!("{sid}.png")),
        )?;
        ensure!(
            image.width == old.width
                && image.height == old.height
                && sha256(&image.rgba) == text(r, "after_rgba_sha256")?,
            "preview mismatch"
        );
        let chosen = array(s, "regions")?
            .iter()
            .map(|r| text(r, "id").map(String::from))
            .collect::<Result<HashSet<_>>>()?;
        let allowed = mask(s, old.width, old.height, &chosen)?;
        check_pixels(&old.rgba, &image.rgba, &allowed)?;
        writes.push(ExpectedWrite {
            offset: off as u64,
            before: before.to_vec(),
            after: graphics::encode(before, image.width, image.height, &image.rgba)?,
        });
    }
    writes.sort_by_key(|w| w.offset);
    let plans = write_plan::validate(
        &writes,
        source.len() as u64,
        std::slice::from_ref(&(0..source.len() as u64)),
    )?;
    let mut result = source.clone();
    for w in &writes {
        result[w.offset as usize..w.offset as usize + w.after.len()].copy_from_slice(&w.after);
    }
    ensure!(
        graphics::snt_table(&result)? == parts,
        "texture extents changed"
    );
    let changed = source.iter().zip(&result).filter(|(a, b)| a != b).count();
    ensure!(
        changed == plans.iter().map(|r| r.changed_bytes).sum::<usize>(),
        "unexplained diff"
    );
    let receipt = json!({"scope":"development SNT; runtime unverified","translation_sha256":thash,"manifest_sha256":sha256(&raw),"preview_report_sha256":sha256(&report_raw),"source_sha256":sha256(&source),"output_sha256":sha256(&result),"changed_bytes":changed,"writes":plans});
    fs::create_dir_all(output)?;
    fs::write(output.join("mainmenu.snt"), result)?;
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}
#[cfg(test)]
mod tests;
