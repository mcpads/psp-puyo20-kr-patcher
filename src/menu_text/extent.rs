//! Couple bounded font growth and optional text relocation to the GAME index.
use super::*;

/// Declare font length updates before the executable planner authorizes writes.
pub fn index_changes(root: &Path) -> Result<Vec<crate::archive_index::Relocation>> {
    let config: Value = serde_json::from_slice(&fs::read(root.join("config/menu-text.json"))?)?;
    let layout: Value = serde_json::from_slice(&fs::read(root.join("config/menu-build.json"))?)?;
    let game = layout["game_iso_offset"].as_u64().context("GAME offset")?;
    let mut changes = Vec::new();
    for pair in config["pairs"].as_array().context("text pairs")? {
        if let Some(growth) = pair.get("font_growth") {
            let r = &pair["resources"][0];
            let before_size = r["size"].as_u64().context("font size")?;
            let count = pair["glyph_count"].as_u64().context("glyph count")?;
            let output = pair["expanded_glyph_count"]
                .as_u64()
                .context("expanded count")?;
            let extra = output
                .checked_sub(count)
                .and_then(|n| n.checked_mul(4))
                .context("font growth")?;
            let source_height = pair["image_height"]
                .as_u64()
                .context("source atlas height")?;
            let height = pair
                .get("expanded_image_height")
                .unwrap_or(&pair["image_height"])
                .as_u64()
                .context("output atlas height")?;
            let plane_growth = height
                .checked_sub(source_height)
                .and_then(|n| n.checked_mul(512))
                .context("atlas growth")?;
            let extra = extra
                .checked_add(plane_growth)
                .context("font growth overflow")?;
            ensure!(extra > 0, "empty font growth");
            let offset = u32::try_from(
                r["offset"]
                    .as_u64()
                    .context("font offset")?
                    .checked_sub(game)
                    .context("GAME font offset")?,
            )?;
            changes.push(crate::archive_index::Relocation {
                record_offset: usize::try_from(
                    growth["record_offset"].as_u64().context("index record")?,
                )?,
                hash_path: serde_json::from_value(growth["hash_path"].clone())?,
                before_offset: offset,
                before_size: u32::try_from(before_size)?,
                offset,
                size: u32::try_from(
                    before_size
                        .checked_add(extra)
                        .context("font size overflow")?,
                )?,
            });
            if let Some(move_text) = pair.get("text_relocation") {
                let text = &pair["resources"][1];
                changes.push(crate::archive_index::Relocation {
                    record_offset: usize::try_from(
                        move_text["record_offset"]
                            .as_u64()
                            .context("text index record")?,
                    )?,
                    hash_path: serde_json::from_value(move_text["hash_path"].clone())?,
                    before_offset: u32::try_from(
                        text["offset"]
                            .as_u64()
                            .context("text offset")?
                            .checked_sub(game)
                            .context("GAME text offset")?,
                    )?,
                    before_size: u32::try_from(text["size"].as_u64().context("text size")?)?,
                    offset: u32::try_from(
                        move_text["offset"]
                            .as_u64()
                            .context("new text offset")?
                            .checked_sub(game)
                            .context("new GAME text offset")?,
                    )?,
                    size: u32::try_from(text["size"].as_u64().context("text size")?)?,
                });
                for asset in preserved_assets(move_text)? {
                    let record: crate::archive_index::Record =
                        serde_json::from_value(asset["record"].clone())?;
                    ensure!(
                        record.member == u32::MAX,
                        "preserved asset is not standalone"
                    );
                    changes.push(crate::archive_index::Relocation {
                        record_offset: record.record_offset,
                        hash_path: record.hash_path,
                        before_offset: record.offset,
                        before_size: record.size_bytes,
                        offset: u32::try_from(
                            asset["offset"]
                                .as_u64()
                                .context("preserved output offset")?
                                .checked_sub(game)
                                .context("preserved GAME offset")?,
                        )?,
                        size: record.size_bytes,
                    });
                }
            }
        } else {
            ensure!(
                pair.get("text_relocation").is_none(),
                "text move requires font growth"
            );
        }
    }
    Ok(changes)
}

pub(super) fn relocate_pair(
    root: &Path,
    iso: &mut fs::File,
    config: &Value,
    writes: &[ExpectedWrite],
    font: &[u8],
    text: &[u8],
    proof: Option<&crate::executable::VerifiedFontCopy>,
) -> Result<ExpectedWrite> {
    let move_text = &config["text_relocation"];
    let offset = writes[0].offset;
    let text_offset = move_text["offset"].as_u64().context("new text offset")?;
    let length = usize::try_from(move_text["joint_size"].as_u64().context("joint size")?)?;
    let before = source::read_extent(iso, offset, length)?;
    ensure!(
        move_text["joint_sha256"] == sha256(&before),
        "joint source mismatch"
    );
    let old_text = usize::try_from(
        writes[1]
            .offset
            .checked_sub(offset)
            .context("old text order")?,
    )?;
    let new_text = usize::try_from(text_offset.checked_sub(offset).context("new text order")?)?;
    ensure!(
        offset.is_multiple_of(2048)
            && length.is_multiple_of(2048)
            && new_text == font.len().div_ceil(2048) * 2048
            && new_text
                .checked_add(text.len().div_ceil(2048) * 2048)
                .is_some_and(|end| end <= length),
        "joint output bounds"
    );
    ensure!(
        proof.is_some_and(|p| p.covers_font_extent(text_offset, text.len())),
        "text move requires coupled index plan"
    );
    let layout: Value = serde_json::from_slice(&fs::read(
        // The caller has already pinned this GAME layout for the index plan.
        root.join("config/menu-build.json"),
    )?)?;
    let game = layout["game_iso_offset"].as_u64().context("GAME offset")?;
    let mut pieces = vec![
        (0, 0, writes[0].before.as_slice(), font),
        (old_text, new_text, writes[1].before.as_slice(), text),
    ];
    let mut next = new_text + text.len().div_ceil(2048) * 2048;
    for asset in preserved_assets(move_text)? {
        let record: crate::archive_index::Record = serde_json::from_value(asset["record"].clone())?;
        let at = usize::try_from(
            game.checked_add(u64::from(record.offset))
                .and_then(|v| v.checked_sub(offset))
                .context("preserved source placement")?,
        )?;
        let data = graphics::bytes(&before, at, record.size_bytes as usize)?;
        let destination = asset["offset"]
            .as_u64()
            .context("preserved output offset")?;
        ensure!(
            asset["sha256"] == sha256(data),
            "preserved asset identity mismatch"
        );
        ensure!(
            destination == offset + next as u64
                && proof.is_some_and(|p| p.covers_font_extent(destination, data.len())),
            "preserved asset requires consecutive placement and coupled index plan"
        );
        pieces.push((at, next, data, data));
        next = next
            .checked_add(data.len().div_ceil(2048) * 2048)
            .context("preserved capacity overflow")?;
    }
    let after = place_joint(&before, &pieces)?;
    Ok(ExpectedWrite {
        offset,
        before,
        after,
    })
}

fn preserved_assets(config: &Value) -> Result<&[Value]> {
    match config.get("preserved_assets") {
        None => Ok(&[]),
        Some(value) => Ok(value.as_array().context("preserved assets")?),
    }
}

/// Preserve every declared source byte and reject unexplained padding or overlaps.
pub(super) fn place_joint(
    before: &[u8],
    pieces: &[(usize, usize, &[u8], &[u8])],
) -> Result<Vec<u8>> {
    let mut source_spans = Vec::new();
    let mut output_spans = Vec::new();
    let mut after = vec![0; before.len()];
    for &(source, destination, original, replacement) in pieces {
        ensure!(
            !original.is_empty() && !replacement.is_empty(),
            "empty joint asset"
        );
        ensure!(
            graphics::bytes(before, source, original.len())? == original,
            "joint source mismatch"
        );
        let source_end = source
            .checked_add(original.len())
            .context("joint source end")?;
        let output_end = destination
            .checked_add(replacement.len().div_ceil(2048) * 2048)
            .context("joint output end")?;
        ensure!(
            destination.is_multiple_of(2048) && output_end <= before.len(),
            "joint output capacity"
        );
        source_spans.push((source, source_end));
        output_spans.push((destination, output_end));
        after[destination..destination + replacement.len()].copy_from_slice(replacement);
    }
    source_spans.sort_unstable();
    let mut end = 0;
    for (start, next) in source_spans {
        ensure!(
            start >= end && before[end..start].iter().all(|b| *b == 0),
            "joint source overlap or nonzero padding"
        );
        end = next;
    }
    ensure!(
        before[end..].iter().all(|b| *b == 0),
        "nonzero joint trailing padding"
    );
    output_spans.sort_unstable();
    ensure!(
        output_spans.windows(2).all(|p| p[0].1 <= p[1].0),
        "joint output overlap"
    );
    Ok(after)
}
