//! Deterministic small white labels with original outline and transparency colors.
use crate::{authoring, graphics, sha256, snc};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    font: Input,
    snt: Input,
    snc: Input,
    translation: Input,
    entry_ids: Vec<String>,
}
fn read(root: &Path, input: &Input) -> Result<Vec<u8>> {
    let bytes = fs::read(root.join(&input.path))?;
    ensure!(
        sha256(&bytes) == input.sha256,
        "label input identity mismatch: {}",
        input.path
    );
    Ok(bytes)
}
fn render(
    font: &fontdue::Font,
    text: &str,
    w: usize,
    h: usize,
    clear: [u8; 4],
    outline: [u8; 4],
) -> Result<graphics::Image> {
    ensure!(
        !text.is_empty() && w >= 5 && h >= 16,
        "invalid label geometry"
    );
    let mut glyphs = Vec::new();
    let mut advance = 0i32;
    for c in text.chars() {
        ensure!(font.lookup_glyph_index(c) != 0, "missing label glyph {c}");
        let (m, pixels) = font.rasterize(c, 12.0);
        let x = advance + m.xmin;
        let y = 12 - m.ymin - m.height as i32;
        ensure!(
            x >= 0 && y >= 0 && y + m.height as i32 <= 14,
            "label glyph bounds"
        );
        advance += m.advance_width.round() as i32;
        glyphs.push((x, y, m, pixels));
    }
    ensure!(
        advance > 0 && advance as usize + 4 <= w,
        "label exceeds cell width: {text}"
    );
    let left = (w - advance as usize) / 2;
    let top = (h - 14) / 2;
    let mut ink = vec![false; w * h];
    for (x, y, m, pixels) in glyphs {
        for yy in 0..m.height {
            for xx in 0..m.width {
                if pixels[yy * m.width + xx] >= 128 {
                    let px = left + x as usize + xx;
                    let py = top + y as usize + yy;
                    ensure!(
                        px > 0 && py > 0 && px + 1 < w && py + 1 < h,
                        "label outline clips"
                    );
                    ink[py * w + px] = true;
                }
            }
        }
    }
    ensure!(ink.iter().any(|v| *v), "empty label raster");
    let mut rgba = clear.repeat(w * h);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            if ink[y * w + x] {
                for yy in y - 1..=y + 1 {
                    for xx in x - 1..=x + 1 {
                        rgba[(yy * w + xx) * 4..][..4].copy_from_slice(&outline);
                    }
                }
            }
        }
    }
    for (i, on) in ink.into_iter().enumerate() {
        if on {
            rgba[i * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
    Ok(graphics::Image {
        width: w,
        height: h,
        format: 3,
        order: 0,
        rgba,
    })
}

pub fn prepare(root: &Path, manifest: &Path, output: &Path) -> Result<Value> {
    ensure!(!output.exists(), "label output exists");
    let raw = fs::read(manifest)?;
    let m: Manifest = serde_json::from_slice(&raw)?;
    let font = fontdue::Font::from_bytes(read(root, &m.font)?, fontdue::FontSettings::default())
        .map_err(anyhow::Error::msg)?;
    let snt = read(root, &m.snt)?;
    let snc = read(root, &m.snc)?;
    let translation: Value = serde_json::from_slice(&read(root, &m.translation)?)?;
    let entries = translation["entries"].as_array().context("label entries")?;
    let mut ids = Vec::new();
    let mut jobs = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for id in &m.entry_ids {
        ensure!(names.insert(id), "duplicate label entry");
        let entry = entries
            .iter()
            .find(|e| e["id"] == *id)
            .context("unknown label entry")?;
        let text = entry["korean"].as_str().context("label text")?;
        for region in entry["regions"].as_array().context("label regions")? {
            let cell = usize::try_from(region["cell_id"].as_u64().context("cell id")?)?;
            ids.push(cell);
            jobs.push((id, text, cell, region));
        }
    }
    let table = snc::inspect(&snt, &snc, Some(&ids))?;
    let textures = graphics::snt_table(&snt)?;
    let mut results = Vec::new();
    let mut images = Vec::new();
    for (id, text, cell_id, region) in jobs {
        let cell = &table.cells[cell_id];
        ensure!(
            region["slot"] == cell.slot && region["rect_xywh"] == json!(cell.rect_xywh),
            "label cell mapping drift"
        );
        let (offset, size) = textures[cell.slot];
        let gim = &snt[offset..offset + size];
        let source = graphics::decode(gim)?;
        let [x, y, w, h] = cell.rect_xywh.map(|n| n as usize);
        let mut colors = BTreeMap::<[u8; 4], usize>::new();
        for yy in y..y + h {
            for xx in x..x + w {
                *colors
                    .entry(source.rgba[(yy * source.width + xx) * 4..][..4].try_into()?)
                    .or_default() += 1;
            }
        }
        let clear = *colors
            .iter()
            .filter(|(p, _)| p[3] == 0)
            .max_by_key(|(_, n)| *n)
            .context("label lacks transparent background")?
            .0;
        let outline = *colors
            .iter()
            .filter(|(p, _)| p[3] == 255 && **p != [255, 255, 255, 255])
            .max_by_key(|(_, n)| *n)
            .context("label lacks opaque outline")?
            .0;
        let image = render(&font, text, w, h, clear, outline)?;
        let palette = graphics::palette(gim)?;
        ensure!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| palette.contains(p)),
            "label color outside source palette"
        );
        results.push(json!({"entry":id,"text":text,"cell_id":cell_id,"slot":cell.slot,"size":[w,h],"clear":clear,"outline":outline,"rgba_sha256":sha256(&image.rgba)}));
        images.push((cell_id, image));
    }
    fs::create_dir_all(output)?;
    for ((id, image), receipt) in images.into_iter().zip(results.iter_mut()) {
        let name = format!("cell-{id:03}.png");
        let path = output.join(&name);
        authoring::png_write(&path, &image)?;
        receipt["png"] = json!(name);
        receipt["png_sha256"] = json!(sha256(&fs::read(path)?));
    }
    let receipt = json!({"manifest_sha256":sha256(&raw),"font_sha256":m.font.sha256,"translation_sha256":m.translation.sha256,"font_pixels":12,"method":"fontdue fixed advance rounding; alpha threshold 128; one pixel original-color outline","labels":results});
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}
