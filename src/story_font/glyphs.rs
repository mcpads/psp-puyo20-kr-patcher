//! Render the adopted character set and preserve selected original symbol cells.
use crate::{
    font_glyphs::{copy_source_glyph, glyph_pixels, selected_source_glyphs},
    graphics, sha256,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

pub(super) struct BuiltFont {
    pub bytes: Vec<u8>,
    pub source_glyphs: usize,
    pub added_glyphs: usize,
    pub glyphs: usize,
    pub height: usize,
    pub retained_source_glyphs: usize,
    pub preserved_glyphs: Vec<Value>,
    pub characters_sha256: String,
}

fn word(b: &[u8], at: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(graphics::bytes(b, at, 4)?.try_into()?) as usize)
}
fn half(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(graphics::bytes(b, at, 2)?.try_into()?))
}

fn shift_into_rows(cell: &mut [u8; 196], rows: usize) -> Result<()> {
    let end = cell.iter().rposition(|&a| a != 0).map_or(0, |i| i / 14 + 1);
    if end > rows {
        let shift = (end - rows) * 14;
        ensure!(
            cell[..shift].iter().all(|&a| a == 0),
            "glyph ink exceeds display height"
        );
        cell.copy_within(shift.., 0);
        cell[196 - shift..].fill(0);
    }
    Ok(())
}

/// `derived` is the translation's character set; a config `characters` file is the
/// fallback for font-only builds without a translation.
pub(super) fn build(
    root: &Path,
    config: &Value,
    fnt: &[u8],
    compact: bool,
    derived: Option<&str>,
) -> Result<BuiltFont> {
    let glyph_count = word(fnt, 12)?;
    ensure!(
        graphics::bytes(fnt, 0, 4)? == b"FNT\0"
            && word(fnt, 4)? == 12
            && word(fnt, 8)? == 13
            && glyph_count > 0
            && glyph_count < 0xf800,
        "source font header"
    );
    let gim_start = 16 + glyph_count * 4;
    let gim = graphics::bytes(
        fnt,
        gim_start,
        fnt.len().checked_sub(gim_start).context("FNT table")?,
    )?;
    let original_image = graphics::decode(gim)?;
    let height = usize::try_from(config["height"].as_u64().context("atlas height")?)?;
    ensure!(
        original_image.width == 512 && original_image.format == 5 && original_image.order == 0,
        "unsupported story atlas"
    );
    let grown = if original_image.height == height {
        gim.to_vec()
    } else {
        graphics::resize_font_atlas(gim, height)?
    };
    let mut image = graphics::decode(&grown)?;
    let listed = config
        .get("characters")
        .map(|path| -> Result<String> {
            let raw = fs::read(root.join(path.as_str().context("characters path")?))?;
            ensure!(
                config["characters_sha256"] == sha256(&raw),
                "story character inventory drift"
            );
            let charset: Value = serde_json::from_slice(&raw)?;
            Ok(charset["characters"]
                .as_str()
                .context("characters")?
                .to_string())
        })
        .transpose()?;
    let characters = match (listed, derived) {
        (Some(list), Some(chars)) => {
            ensure!(
                list == chars,
                "story character list differs from translation"
            );
            list
        }
        (Some(list), None) => list,
        (None, Some(chars)) => chars.to_string(),
        (None, None) => anyhow::bail!("story font has no character source"),
    };
    let source_slots = selected_source_glyphs(config, fnt, compact)?;
    let mut known = BTreeSet::new();
    for &slot in &source_slots {
        known.insert(
            char::from_u32(half(fnt, 16 + 4 * slot)? as u32).context("source glyph scalar")?,
        );
    }
    let mut additions: BTreeSet<_> = characters.chars().filter(|c| !known.contains(c)).collect();
    if compact {
        if !known.contains(&'□') {
            additions.insert('□');
        }
        for pixel in image.rgba.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[255, 255, 255, 0]);
        }
    }
    let retained = source_slots.len();
    let count = retained + additions.len();
    ensure!(
        !additions.is_empty() && count < 0xf800 && count.div_ceil(36) * 14 <= height,
        "expanded atlas capacity"
    );
    let font_raw = fs::read(root.join(config["font"].as_str().context("font path")?))?;
    ensure!(
        config["font_sha256"] == sha256(&font_raw),
        "font identity mismatch"
    );
    let font = fontdue::Font::from_bytes(font_raw.as_slice(), fontdue::FontSettings::default())
        .map_err(anyhow::Error::msg)?;
    let mut new_fnt = fnt[..16].to_vec();
    new_fnt[12..16].copy_from_slice(&u32::try_from(count)?.to_le_bytes());
    for (slot, &source_slot) in source_slots.iter().enumerate() {
        new_fnt.extend_from_slice(graphics::bytes(fnt, 16 + source_slot * 4, 4)?);
        copy_source_glyph(&original_image.rgba, source_slot, &mut image.rgba, slot)?;
    }
    // Rendering defaults match the original 12px Galmuri11 setup; a font whose ink sits
    // lower or wider picks its own size, baseline row and horizontal shift in the config.
    let px = config
        .get("glyph_px")
        .and_then(Value::as_f64)
        .unwrap_or(12.0) as f32;
    let baseline = config.get("baseline").and_then(Value::as_i64).unwrap_or(12) as i32;
    let x_shift = config.get("x_shift").and_then(Value::as_i64).unwrap_or(0) as i32;
    // The 14-row atlas stride includes padding. The consumer passes header +4 (12)
    // as the sprite height; row 12 is outside that rectangle, even when the cell fits.
    let display_rows = word(fnt, 4)? as i32;
    let rows = config
        .get("glyph_rows")
        .and_then(Value::as_i64)
        .unwrap_or(i64::from(display_rows)) as i32;
    ensure!(
        (1..=display_rows).contains(&rows),
        "glyph_rows exceeds font display height"
    );
    // "autohint" renders with the FreeType-style auto-hinter, which snaps stems to the pixel
    // grid; the default unhinted rasterizer spreads thin stems over two pixels.
    let hinted = match config.get("rasterizer").and_then(Value::as_str) {
        None | Some("fontdue") => None,
        Some("autohint") => Some(crate::hinting::Hinted::new(&font_raw, px)?),
        Some(other) => anyhow::bail!("unknown rasterizer {other}"),
    };
    for (i, c) in additions.iter().enumerate() {
        ensure!(font.lookup_glyph_index(*c) != 0, "missing font glyph {c}");
        let mut cell = match &hinted {
            Some(h) => h.cell(*c, x_shift, baseline)?,
            None => {
                let (metrics, pixels) = font.rasterize(*c, px);
                let (x0, y0) = (
                    metrics.xmin + x_shift,
                    baseline - metrics.ymin - metrics.height as i32,
                );
                ensure!(
                    x0 >= 0 && y0 >= 0 && x0 + metrics.width as i32 <= 13,
                    "glyph bounds {c}"
                );
                let mut cell = [0u8; 14 * 14];
                for y in 0..metrics.height {
                    for x in 0..metrics.width {
                        let (cx, cy) = (x0 as usize + x, y0 as usize + y);
                        if cy < 14 {
                            cell[cy * 14 + cx] = pixels[y * metrics.width + x];
                        } else {
                            ensure!(pixels[y * metrics.width + x] == 0, "glyph bounds {c}");
                        }
                    }
                }
                cell
            }
        };
        if config.get("fit_to_display_height").and_then(Value::as_bool) == Some(true) {
            shift_into_rows(&mut cell, rows as usize).with_context(|| format!("glyph {c}"))?;
        }
        for y in rows as usize..14 {
            ensure!(
                cell[y * 14..(y + 1) * 14].iter().all(|&a| a == 0),
                "glyph bounds {c}"
            );
        }
        ensure!((0..14).all(|y| cell[y * 14 + 13] == 0), "glyph bounds {c}");
        let slot = retained + i;
        let (sx, sy) = ((slot % 36) * 14, (slot / 36) * 14);
        for y in 0..14 {
            for x in 0..14 {
                image.rgba[((sy + y) * 512 + sx + x) * 4..][..4].copy_from_slice(&[
                    255,
                    255,
                    255,
                    cell[y * 14 + x],
                ]);
            }
        }
        new_fnt.extend_from_slice(
            &u16::try_from(*c as u32)
                .context("non BMP glyph")?
                .to_le_bytes(),
        );
        new_fnt.extend_from_slice(&13u16.to_le_bytes());
    }
    let encoded = graphics::encode(&grown, 512, height, &image.rgba)?;
    let decoded = graphics::decode(&encoded)?;
    let mut glyph_receipts = Vec::new();
    for (slot, &source_slot) in source_slots.iter().enumerate() {
        ensure!(
            graphics::bytes(&new_fnt, 16 + slot * 4, 4)?
                == graphics::bytes(fnt, 16 + source_slot * 4, 4)?,
            "source glyph table changed"
        );
        let pixels = glyph_pixels(&original_image.rgba, source_slot)?;
        ensure!(
            glyph_pixels(&decoded.rgba, slot)? == pixels,
            "source glyph raster changed"
        );
        glyph_receipts.push(json!({"source_slot":source_slot,"output_slot":slot,
            "character":char::from_u32(u32::from(half(fnt, 16 + source_slot * 4)?)),
            "width":half(fnt,18 + source_slot * 4)?,"rgba_sha256":sha256(&pixels)}));
    }
    new_fnt.extend_from_slice(&encoded);
    Ok(BuiltFont {
        bytes: new_fnt,
        source_glyphs: glyph_count,
        added_glyphs: additions.len(),
        glyphs: count,
        height,
        retained_source_glyphs: retained,
        preserved_glyphs: glyph_receipts,
        characters_sha256: sha256(characters.as_bytes()),
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
