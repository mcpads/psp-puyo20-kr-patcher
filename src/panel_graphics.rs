//! Fixed-font multiline text in bounded portions of original graphic cells.
use crate::{authoring, graphics, sha256, snc};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
#[cfg(test)]
mod tests;

fn default_outlined() -> bool {
    true
}

fn default_font_pixels() -> usize {
    12
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    text: String,
    color: [u8; 4],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Panel {
    cell_id: usize,
    #[serde(default = "default_font_pixels")]
    font_pixels: usize,
    edit_rect: [usize; 4],
    clear: [u8; 4],
    outline: [u8; 4],
    #[serde(default = "default_outlined")]
    outlined: bool,
    centered: bool,
    #[serde(default)]
    vertically_centered: bool,
    #[serde(default)]
    background: Option<Input>,
    #[serde(default)]
    background_origin: Option<[usize; 2]>,
    lines: Vec<Vec<Run>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    font: Input,
    snt: Input,
    snc: Input,
    panels: Vec<Panel>,
    #[serde(default)]
    shared_views: Vec<snc::SharedView>,
}
fn read(root: &Path, input: &Input) -> Result<Vec<u8>> {
    let b = fs::read(root.join(&input.path))?;
    ensure!(sha256(&b) == input.sha256, "panel input identity mismatch");
    Ok(b)
}
pub fn prepare(root: &Path, manifest: &Path, output: &Path) -> Result<Value> {
    ensure!(!output.exists(), "panel output exists");
    let raw = fs::read(manifest)?;
    let m: Manifest = serde_json::from_slice(&raw)?;
    let font = fontdue::Font::from_bytes(read(root, &m.font)?, fontdue::FontSettings::default())
        .map_err(anyhow::Error::msg)?;
    let snt = read(root, &m.snt)?;
    let table = snc::inspect(&snt, &read(root, &m.snc)?, None)?;
    table.validate_selection(
        &m.panels
            .iter()
            .map(|p| p.cell_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
        &m.shared_views,
    )?;
    let textures = graphics::snt_table(&snt)?;
    let mut prepared = Vec::new();
    validate_regions(&m.panels)?;
    for p in &m.panels {
        ensure!(
            (8..=12).contains(&p.font_pixels),
            "unsupported panel font size"
        );
        let line_height = if p.font_pixels == 12 && p.outlined {
            14
        } else {
            p.font_pixels
        };
        let cell = &table.cells[p.cell_id];
        let [cx, cy, w, h] = cell.rect_xywh.map(|v| v as usize);
        let (o, n) = textures[cell.slot];
        let gim = &snt[o..o + n];
        let source = graphics::decode(gim)?;
        let palette = graphics::palette(gim)?;
        let [x, y, rw, rh] = p.edit_rect;
        ensure!(
            rw > 4
                && rh > 4
                && x.checked_add(rw).is_some_and(|v| v <= w)
                && y.checked_add(rh).is_some_and(|v| v <= h),
            "panel edit rectangle outside cell"
        );
        ensure!(
            (!p.lines.is_empty() || p.background.is_some())
                && p.lines.len() * line_height
                    + if !p.outlined {
                        0
                    } else if p.vertically_centered {
                        2
                    } else {
                        4
                    }
                    <= rh,
            "panel line capacity exceeded"
        );
        ensure!(
            p.background_origin.is_none() || p.background.is_some(),
            "background origin without image"
        );
        ensure!(
            palette.contains(&p.clear) && palette.contains(&p.outline),
            "panel base colors outside palette"
        );
        let mut image = graphics::Image {
            width: w,
            height: h,
            format: 3,
            order: 0,
            rgba: Vec::with_capacity(w * h * 4),
        };
        for row in cy..cy + h {
            image.rgba.extend_from_slice(
                &source.rgba[(row * source.width + cx) * 4..(row * source.width + cx + w) * 4],
            );
        }
        let before = image.rgba.clone();
        if let Some(background) = &p.background {
            read(root, background)?;
            let backdrop = authoring::png_read(&root.join(&background.path))?;
            ensure!(
                backdrop
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|c| palette.iter().any(|p| p.as_slice() == c)),
                "panel background outside palette"
            );
            let origins = p.background_origin.map(|origin| [origin]);
            crate::graphic_archives::merge_rectangles(
                &mut image,
                &backdrop,
                &[p.edit_rect],
                origins.as_ref().map(|a| a.as_slice()),
            )?;
        } else {
            for yy in y..y + rh {
                for xx in x..x + rw {
                    image.rgba[(yy * w + xx) * 4..][..4].copy_from_slice(&p.clear);
                }
            }
        }
        let mut ink = vec![None; w * h];
        for (line, runs) in p.lines.iter().enumerate() {
            let mut glyphs = Vec::new();
            let mut advance = 0i32;
            for run in runs {
                ensure!(
                    palette.contains(&run.color),
                    "panel text color outside palette"
                );
                for c in run.text.chars() {
                    ensure!(font.lookup_glyph_index(c) != 0, "missing panel glyph {c}");
                    let (metric, pixels) = font.rasterize(c, p.font_pixels as f32);
                    glyphs.push((
                        advance + metric.xmin,
                        p.font_pixels as i32 - metric.ymin - metric.height as i32,
                        metric.width,
                        metric.height,
                        pixels,
                        run.color,
                    ));
                    advance += metric.advance_width.round() as i32;
                }
            }
            ensure!(
                advance >= 0 && advance as usize + 4 <= rw,
                "panel line {line} exceeds pixel width"
            );
            let left = x + if p.centered {
                (rw - advance as usize) / 2
            } else {
                2
            };
            let inset = if p.vertically_centered {
                (rh - p.lines.len() * line_height) / 2
            } else {
                2
            };
            let top = y + inset + line * line_height;
            for (gx, gy, gw, gh, pixels, color) in glyphs {
                ensure!(gx >= 0 && gy >= 0, "panel glyph bearing outside layout");
                for yy in 0..gh {
                    for xx in 0..gw {
                        if pixels[yy * gw + xx] >= 128 {
                            let px = left + gx as usize + xx;
                            let py = top + gy as usize + yy;
                            ensure!(
                                px >= x
                                    && py >= y
                                    && px < x + rw
                                    && py < y + rh
                                    && (!p.outlined
                                        || (px > x
                                            && py > y
                                            && px + 1 < x + rw
                                            && py + 1 < y + rh)),
                                "panel outline clips"
                            );
                            ink[py * w + px] = Some(color);
                        }
                    }
                }
            }
        }
        for yy in y + 1..y + rh - 1 {
            for xx in x + 1..x + rw - 1 {
                if p.outlined && ink[yy * w + xx].is_some() {
                    for oy in yy - 1..=yy + 1 {
                        for ox in xx - 1..=xx + 1 {
                            image.rgba[(oy * w + ox) * 4..][..4].copy_from_slice(&p.outline);
                        }
                    }
                }
            }
        }
        for (i, color) in ink.into_iter().enumerate() {
            if let Some(color) = color {
                image.rgba[i * 4..][..4].copy_from_slice(&color);
            }
        }
        ensure!(before != image.rgba, "panel has no changes");
        for yy in 0..h {
            for xx in 0..w {
                if !(x..x + rw).contains(&xx) || !(y..y + rh).contains(&yy) {
                    let i = (yy * w + xx) * 4;
                    ensure!(
                        before[i..i + 4] == image.rgba[i..i + 4],
                        "protected panel pixel changed"
                    );
                }
            }
        }
        prepared.push((p, image));
    }
    // Render every region against the original, then merge disjoint edits per cell.
    let mut merged = BTreeMap::<usize, (graphics::Image, Vec<&Panel>)>::new();
    for (p, image) in prepared {
        if let Some((current, regions)) = merged.get_mut(&p.cell_id) {
            let [x, y, w, h] = p.edit_rect;
            for row in y..y + h {
                let start = (row * image.width + x) * 4;
                current.rgba[start..start + w * 4]
                    .copy_from_slice(&image.rgba[start..start + w * 4]);
            }
            regions.push(p);
        } else {
            merged.insert(p.cell_id, (image, vec![p]));
        }
    }
    fs::create_dir_all(output)?;
    let mut receipts = Vec::new();
    for (id, (image, regions)) in merged {
        let filename = format!("cell-{id:03}.png");
        let path = output.join(&filename);
        authoring::png_write(&path, &image)?;
        receipts.push(json!({"cell_id":id,"png":filename,"png_sha256":sha256(&fs::read(path)?),
            "rgba_sha256":sha256(&image.rgba),
            "allowed_rects":regions.iter().map(|p|p.edit_rect).collect::<Vec<_>>(),
            "font_pixels":regions.iter().map(|p|p.font_pixels).collect::<Vec<_>>(),
            "background_origins":regions.iter().map(|p|p.background_origin).collect::<Vec<_>>(),
            "lines":regions.iter().flat_map(|p|p.lines.iter().map(|runs|runs.iter().map(|r|r.text.as_str()).collect::<String>())).collect::<Vec<_>>() }));
    }
    let receipt = json!({"manifest_sha256":sha256(&raw),"font_sha256":m.font.sha256,"panels":receipts,"shared_views":m.shared_views});
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}

fn validate_regions(panels: &[Panel]) -> Result<()> {
    for (i, a) in panels.iter().enumerate() {
        let [x, y, w, h] = a.edit_rect;
        let right = x
            .checked_add(w)
            .ok_or_else(|| anyhow::anyhow!("panel x overflow"))?;
        let bottom = y
            .checked_add(h)
            .ok_or_else(|| anyhow::anyhow!("panel y overflow"))?;
        for b in &panels[..i] {
            if a.cell_id == b.cell_id {
                let [bx, by, bw, bh] = b.edit_rect;
                ensure!(
                    right <= bx || bx + bw <= x || bottom <= by || by + bh <= y,
                    "overlapping panel regions in cell {}",
                    a.cell_id
                );
            }
        }
    }
    Ok(())
}
