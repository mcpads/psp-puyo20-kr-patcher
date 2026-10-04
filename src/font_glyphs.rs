//! Select and copy original 14x14 glyph cells shared by menu and story fonts.
use crate::graphics;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::BTreeSet;

fn word(b: &[u8], at: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(graphics::bytes(b, at, 4)?.try_into()?) as usize)
}
fn half(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(graphics::bytes(b, at, 2)?.try_into()?))
}

pub(crate) fn selected_source_glyphs(
    config: &Value,
    fnt: &[u8],
    compact: bool,
) -> Result<Vec<usize>> {
    let count = word(fnt, 12)?;
    let Some(entries) = config.get("preserved_glyphs") else {
        return Ok(if compact {
            Vec::new()
        } else {
            (0..count).collect()
        });
    };
    ensure!(
        compact,
        "explicit glyph selection requires translation_only"
    );
    let mut slots = BTreeSet::new();
    let mut characters = BTreeSet::new();
    for entry in entries.as_array().context("preserved glyph list")? {
        let slot = usize::try_from(entry["source_slot"].as_u64().context("source glyph slot")?)?;
        ensure!(slot < count, "preserved glyph outside source table");
        let c = char::from_u32(u32::from(half(fnt, 16 + slot * 4)?))
            .context("preserved glyph scalar")?;
        ensure!(
            entry["character"].as_str() == Some(c.to_string().as_str())
                && entry["width"] == half(fnt, 18 + slot * 4)?,
            "preserved glyph identity mismatch"
        );
        ensure!(
            slots.insert(slot) && characters.insert(c),
            "duplicate preserved glyph slot or character"
        );
        ensure!(
            !entry["reason"]
                .as_str()
                .context("preserved glyph reason")?
                .trim()
                .is_empty(),
            "preserved glyph needs a reason"
        );
    }
    Ok(slots.into_iter().collect())
}

pub(crate) fn glyph_pixels(rgba: &[u8], slot: usize) -> Result<Vec<u8>> {
    let (x, y) = ((slot % 36) * 14, (slot / 36) * 14);
    let mut pixels = Vec::with_capacity(14 * 14 * 4);
    for row in 0..14 {
        pixels.extend_from_slice(graphics::bytes(rgba, ((y + row) * 512 + x) * 4, 14 * 4)?);
    }
    Ok(pixels)
}

pub(crate) fn copy_source_glyph(
    source: &[u8],
    source_slot: usize,
    output: &mut [u8],
    slot: usize,
) -> Result<()> {
    let pixels = glyph_pixels(source, source_slot)?;
    let (x, y) = ((slot % 36) * 14, (slot / 36) * 14);
    for row in 0..14 {
        let at = ((y + row) * 512 + x) * 4;
        output
            .get_mut(at..at + 14 * 4)
            .context("output glyph bounds")?
            .copy_from_slice(&pixels[row * 14 * 4..(row + 1) * 14 * 4]);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
