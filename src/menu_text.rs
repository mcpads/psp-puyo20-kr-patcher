use crate::{
    font_glyphs::{copy_source_glyph, glyph_pixels, selected_source_glyphs},
    graphics, sha256, source,
    text::Mtx,
    write_plan::ExpectedWrite,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

fn word(b: &[u8], p: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(graphics::bytes(b, p, 4)?.try_into()?) as usize)
}
fn half(b: &[u8], p: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(graphics::bytes(b, p, 2)?.try_into()?))
}

// Omitted records are allowed only as an explicitly counted, empty tail.
// Keep their original spans and pointers; never infer omission from missing input.
fn complete_entries(
    mtx: &[u8],
    entries: &[Value],
    count: usize,
    empty_tail: usize,
    repack: bool,
) -> Result<Vec<Value>> {
    ensure!(
        entries.len().checked_add(empty_tail) == Some(count),
        "all nonempty entries required"
    );
    ensure!(
        empty_tail == 0 || !repack,
        "empty tail requires original spans"
    );
    let parsed = Mtx::parse(mtx)?;
    ensure!(parsed.records.len() == count, "record count mismatch");
    let mut result = entries.to_vec();
    for i in entries.len()..count {
        let start = parsed.records[i].offset;
        let end = if i + 1 == count {
            mtx.len()
        } else {
            parsed.records[i + 1].offset
        };
        ensure!(
            end.checked_sub(start) == Some(2) && half(mtx, start)? == 0xffff,
            "preserved tail record {i} is not exactly empty"
        );
        result.push(json!({"id":i,"source":"<FFFF>","lines":[""]}));
    }
    Ok(result)
}

// The indexed consumer reads group -> record pointer. The first payload offset
// also terminates the loader's pointer relocation loop, so it must stay fixed.
fn place_records(source: &[u8], records: &[Vec<u16>], repack: bool) -> Result<(Vec<u8>, Value)> {
    let parsed = Mtx::parse(source)?;
    ensure!(
        records.len() == parsed.records.len(),
        "record count mismatch"
    );
    let table = word(source, 8)?;
    let payload = parsed.header.len();
    let mut result = source.to_vec();
    let mut cursor = payload;
    let mut placements = Vec::new();
    for (i, record) in records.iter().enumerate() {
        ensure!(
            record
                .iter()
                .position(|u| *u == 0xffff)
                .is_some_and(|at| record[at..].iter().all(|u| *u == 0xffff)),
            "record terminator"
        );
        let original_start = parsed.records[i].offset;
        let original_end = if i + 1 == records.len() {
            source.len()
        } else {
            parsed.records[i + 1].offset
        };
        ensure!(
            original_start >= payload
                && original_start < original_end
                && original_end <= source.len(),
            "record span"
        );
        let start = if repack { cursor } else { original_start };
        let end = start.checked_add(record.len() * 2).context("record size")?;
        ensure!(
            end <= if repack { source.len() } else { original_end },
            "MTX capacity exceeded at {i}"
        );
        result[table + i * 4..table + 4 + i * 4]
            .copy_from_slice(&u32::try_from(start)?.to_le_bytes());
        for (j, unit) in record.iter().enumerate() {
            result[start + j * 2..start + j * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        if !repack {
            result[end..original_end].fill(0xff);
        }
        placements.push(
            json!({"id":i,"original_start":original_start,"start":start,"encoded_bytes":end-start}),
        );
        cursor = end;
    }
    if repack {
        result[cursor..].fill(0xff);
    }
    ensure!(
        result[..table] == source[..table]
            && word(&result, table)? == payload
            && Mtx::parse(&result)?.group_first_records == parsed.group_first_records,
        "MTX hierarchy changed"
    );
    Ok((
        result,
        json!({"policy":if repack { "indexed_pool" } else { "original_spans" },"placements":placements,"size_bytes":source.len(),"payload_end":cursor}),
    ))
}

// Flat entry IDs remain stable; both inputs must pin multi-group boundaries.
fn validate_groups(mtx: &Mtx, declared: Option<&Value>) -> Result<()> {
    let groups = match declared {
        Some(value) => serde_json::from_value::<Vec<usize>>(value.clone())?,
        None => vec![0],
    };
    ensure!(
        groups == mtx.group_first_records,
        "MTX group mapping mismatch"
    );
    Ok(())
}

mod extent;
mod segments;
pub use extent::index_changes;

pub fn plan(
    root: &Path,
    iso: &mut fs::File,
    fixed_copy: Option<&crate::executable::VerifiedFontCopy>,
) -> Result<(Vec<ExpectedWrite>, Value)> {
    let config_raw = fs::read(root.join("config/menu-text.json"))?;
    let config: Value = serde_json::from_slice(&config_raw)?;
    let font_raw = fs::read(root.join(config["font"].as_str().context("font path")?))?;
    ensure!(
        sha256(&font_raw) == config["font_sha256"].as_str().context("font hash")?,
        "font identity mismatch"
    );
    let font = fontdue::Font::from_bytes(font_raw, fontdue::FontSettings::default())
        .map_err(anyhow::Error::msg)?;
    let mut writes = Vec::new();
    let mut receipts = Vec::new();
    let pairs = config["pairs"].as_array().context("text pairs")?;
    ensure!(!pairs.is_empty(), "no text pairs");
    for pair in pairs {
        let (pair_writes, receipt) = plan_pair(root, iso, pair, &font, fixed_copy)
            .with_context(|| format!("text pair {}", pair["translation"]))?;
        writes.extend(pair_writes);
        receipts.push(receipt);
    }
    Ok((
        writes,
        json!({"scope":"configured menu descriptions; draft", "config_sha256":sha256(&config_raw), "pairs":receipts,"baseline":12,"pixel_size":12,"fontdue":"0.9.3","font_sha256":config["font_sha256"]}),
    ))
}

fn plan_pair(
    root: &Path,
    iso: &mut fs::File,
    config: &Value,
    font: &fontdue::Font,
    fixed_copy: Option<&crate::executable::VerifiedFontCopy>,
) -> Result<(Vec<ExpectedWrite>, Value)> {
    let count = config["glyph_count"].as_u64().context("glyph count")? as usize;
    let output_count = match config.get("expanded_glyph_count") {
        Some(value) => value.as_u64().context("expanded glyph count")? as usize,
        None => count,
    };
    let entry_count = config["entry_count"].as_u64().context("entry count")? as usize;
    let source_height = config["image_height"].as_u64().context("image height")? as usize;
    let height = config
        .get("expanded_image_height")
        .unwrap_or(&config["image_height"])
        .as_u64()
        .context("output image height")? as usize;
    ensure!(
        height >= source_height && (height == source_height || config.get("font_growth").is_some()),
        "atlas resize requires font growth"
    );
    let line_cells = config["max_line_cells"].as_u64().context("line cells")? as usize;
    ensure!(
        count > 0 && output_count >= count && output_count < 0xf800 && entry_count > 0,
        "invalid population"
    );
    ensure!(
        [16, 32, 64, 128, 256, 512].contains(&height)
            && output_count.div_ceil(36) * 14 <= height
            && line_cells <= 19,
        "unsupported layout bounds"
    );
    let gim_offset = 16 + count * 4;
    let mut writes = Vec::new();
    for r in config["resources"].as_array().context("resources")? {
        let offset = r["offset"].as_u64().context("offset")?;
        let before =
            source::read_extent(iso, offset, r["size"].as_u64().context("size")? as usize)?;
        ensure!(
            sha256(&before) == r["sha256"].as_str().context("hash")?,
            "text source mismatch"
        );
        writes.push(ExpectedWrite {
            offset,
            after: before.clone(),
            before,
        });
    }
    ensure!(writes.len() == 2, "expected FNT and MTX");
    let fnt = &writes[0].before;
    let mtx = &writes[1].before;
    ensure!(
        graphics::bytes(fnt, 0, 4)? == b"FNT\0"
            && word(fnt, 4)? == 12
            && word(fnt, 8)? == 13
            && word(fnt, 12)? == count,
        "unsupported font layout"
    );
    let parsed = Mtx::parse(mtx)?;
    ensure!(parsed.records.len() == entry_count, "record count mismatch");
    validate_groups(&parsed, config.get("group_first_records"))?;
    let payload = parsed.header.len();
    let loaded = crate::translation_batches::load(root, config)?;
    let trans = loaded.doc;
    validate_groups(&parsed, trans.get("group_first_records"))?;
    let entries = trans["entries"].as_array().context("entries")?;
    let translated_entries = entries.len();
    let segmented = match config["text_encoding"].as_str() {
        None | Some("plain_lines") => false,
        Some("source_segments") => true,
        _ => anyhow::bail!("unknown text encoding"),
    };
    let source_packet = if segmented {
        Some(crate::text::inspect_pair(fnt, mtx)?)
    } else {
        None
    };
    let allowed: Vec<u16> = if segmented {
        serde_json::from_value(config["allowed_controls"].clone())?
    } else {
        Vec::new()
    };
    let empty_tail = match config.get("preserved_empty_tail") {
        Some(value) => usize::try_from(value.as_u64().context("empty tail count")?)?,
        None => 0,
    };
    let source_slots = selected_source_glyphs(config, fnt, true)?;
    let mut preserved = BTreeMap::new();
    for source_slot in source_slots {
        let c = char::from_u32(u32::from(half(fnt, 16 + source_slot * 4)?))
            .context("source glyph scalar")?;
        preserved.insert(c, source_slot);
    }
    let repack = match config.get("text_layout").and_then(Value::as_str) {
        None | Some("original_spans") => false,
        Some("indexed_pool") => true,
        Some(_) => anyhow::bail!("unsupported text layout"),
    };
    let entries = complete_entries(mtx, entries, entry_count, empty_tail, repack)?;
    let mut chars = BTreeSet::new();
    for e in &entries {
        for l in e[if segmented { "segments" } else { "lines" }]
            .as_array()
            .context("prose")?
        {
            chars.extend(l.as_str().context("line")?.chars());
        }
    }
    ensure!(chars.len() <= output_count, "font capacity exceeded");
    ensure!(
        preserved.keys().all(|c| chars.contains(c)),
        "unused preserved glyph"
    );
    let map: BTreeMap<_, _> = chars
        .iter()
        .enumerate()
        .map(|(i, c)| (*c, i as u16))
        .collect();
    let mut records = Vec::new();
    let mut expected_records = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if let Some(packet) = &source_packet {
            let group = parsed
                .group_first_records
                .partition_point(|start| *start <= i)
                - 1;
            records.push(segments::encode(
                e,
                &packet["strings"][i],
                group,
                i - parsed.group_first_records[group],
                &map,
                &allowed,
                line_cells,
            )?);
            continue;
        }
        ensure!(e["id"] == i, "entry order mismatch");
        let start = parsed.records[i].offset;
        let end = if i + 1 == entry_count {
            mtx.len()
        } else {
            parsed.records[i + 1].offset
        };
        ensure!(
            payload <= start && start < end && end <= mtx.len() && (end - start).is_multiple_of(2),
            "bad text span"
        );
        let mut source_text = String::new();
        let mut separators = Vec::<Vec<u16>>::new();
        let mut sep = Vec::new();
        for p in (start..end).step_by(2) {
            let u = half(mtx, p)?;
            if (u as usize) < count {
                if !sep.is_empty() {
                    separators.push(std::mem::take(&mut sep));
                }
                source_text.push(
                    char::from_u32(half(fnt, 16 + u as usize * 4)? as u32)
                        .context("source scalar")?,
                );
            } else {
                ensure!([0xfffd, 0xfffe, 0xffff].contains(&u), "unknown control");
                ensure!(u != 0xffff || p + 2 == end, "early source terminator");
                source_text.push_str(&format!("<{u:04X}>"));
                sep.push(u);
            }
        }
        if sep == [0xfffd, 0xffff] {
            separators.push(vec![0xfffd]);
        } else {
            ensure!(sep == [0xffff], "invalid terminal");
        }
        ensure!(
            e["source"].as_str() == Some(&source_text),
            "translation source drift"
        );
        let lines = e["lines"].as_array().context("lines")?;
        ensure!(
            lines.len() == separators.len() + 1,
            "control structure mismatch"
        );
        let mut encoded = Vec::new();
        let mut expected = String::new();
        for (n, line) in lines.iter().enumerate() {
            let line = line.as_str().context("line")?;
            ensure!(line.chars().count() <= line_cells, "line width exceeded");
            expected.push_str(line);
            for c in line.chars() {
                encoded.push(*map.get(&c).context("unmapped character")?);
            }
            if n < separators.len() {
                encoded.extend(&separators[n]);
                for &control in &separators[n] {
                    if control == 0xfffd {
                        expected.push('\n');
                    }
                }
            }
        }
        encoded.push(0xffff);
        records.push(encoded);
        expected_records.push(expected);
    }
    let (new_mtx, text_layout) = place_records(mtx, &records, repack)?;
    let gim = graphics::bytes(
        fnt,
        gim_offset,
        fnt.len()
            .checked_sub(gim_offset)
            .context("font table bounds")?,
    )?;
    let mut image = graphics::decode(gim)?;
    ensure!(
        image.width == 512
            && image.height == source_height
            && image.format == 5
            && image.order == 0,
        "font GIM layout mismatch"
    );
    let original_rgba = image.rgba.clone();
    let reclaim = (output_count - count) * 4;
    let growing = config.get("font_growth").is_some();
    let growth_bytes = reclaim + (height - source_height) * 512;
    let output_gim = if growing {
        ensure!(growth_bytes > 0, "empty font growth");
        let offset = writes[0].offset;
        let new_size = fnt
            .len()
            .checked_add(growth_bytes)
            .context("grown font size")?;
        ensure!(
            fixed_copy.is_some_and(|proof| proof.covers_font_extent(offset, new_size)),
            "font growth requires coupled index plan"
        );
        if height == source_height {
            gim.to_vec()
        } else {
            graphics::resize_font_atlas(gim, height)?
        }
    } else if reclaim == 0 {
        gim.to_vec()
    } else {
        let allocation = config["source_buffer_size"]
            .as_u64()
            .context("expanded font requires observed source allocation")?
            as usize;
        // Observed uncompressed GAME loader rounds the file allocation to 2048.
        // Original FNT loader reads L + 4*N - 16 bytes from FNT + 16 + 4*N.
        ensure!(
            allocation == fnt.len().div_ceil(2048) * 2048
                && (fixed_copy.is_some() || fnt.len() + 8 * output_count <= allocation),
            "expanded font requires verified --fix-font-copy: legacy copy exceeds source allocation"
        );
        graphics::compact_file_info(gim, reclaim)?
    };
    if height != source_height {
        image = graphics::decode(&output_gim)?;
    }
    let output_gim_offset = 16 + output_count * 4;
    let mut new_fnt = fnt[..gim_offset].to_vec();
    new_fnt.resize(output_gim_offset, 0);
    new_fnt.extend_from_slice(&output_gim);
    ensure!(
        new_fnt.len() == fnt.len() + if growing { growth_bytes } else { 0 },
        "expanded font size changed"
    );
    new_fnt[12..16].copy_from_slice(&(output_count as u32).to_le_bytes());
    for (&c, &slot) in &map {
        if let Some(&source_slot) = preserved.get(&c) {
            copy_source_glyph(&original_rgba, source_slot, &mut image.rgba, slot as usize)?;
            let at = 16 + slot as usize * 4;
            new_fnt[at..at + 4].copy_from_slice(graphics::bytes(fnt, 16 + source_slot * 4, 4)?);
            continue;
        }
        ensure!(font.lookup_glyph_index(c) != 0, "missing font glyph {c}");
        let (metrics, pixels) = font.rasterize(c, 12.0);
        let x0 = metrics.xmin;
        let y0 = 12 - metrics.ymin - metrics.height as i32;
        ensure!(
            x0 >= 0
                && y0 >= 0
                && x0 + metrics.width as i32 <= 13
                && y0 + metrics.height as i32 <= 14,
            "glyph bounds {c}"
        );
        let (sx, sy) = ((slot as usize % 36) * 14, (slot as usize / 36) * 14);
        for y in 0..14 {
            for x in 0..14 {
                let alpha = if x >= x0 as usize
                    && y >= y0 as usize
                    && x < x0 as usize + metrics.width
                    && y < y0 as usize + metrics.height
                {
                    pixels[(y - y0 as usize) * metrics.width + x - x0 as usize]
                } else {
                    0
                };
                image.rgba[((sy + y) * 512 + sx + x) * 4..][..4]
                    .copy_from_slice(&[255, 255, 255, alpha]);
            }
        }
        let cp = u16::try_from(c as u32).context("non BMP glyph")?;
        let cell = 16 + slot as usize * 4;
        new_fnt[cell..cell + 2].copy_from_slice(&cp.to_le_bytes());
        new_fnt[cell + 2..cell + 4].copy_from_slice(&13u16.to_le_bytes());
    }
    new_fnt[output_gim_offset..].copy_from_slice(&graphics::encode(
        &output_gim,
        512,
        height,
        &image.rgba,
    )?);
    let decoded_image = graphics::decode(&new_fnt[output_gim_offset..])?;
    let mut glyph_receipts = Vec::new();
    for (&c, &source_slot) in &preserved {
        let slot = map[&c] as usize;
        let pixels = glyph_pixels(&original_rgba, source_slot)?;
        ensure!(
            glyph_pixels(&decoded_image.rgba, slot)? == pixels,
            "source glyph raster changed"
        );
        ensure!(
            graphics::bytes(&new_fnt, 16 + slot * 4, 4)?
                == graphics::bytes(fnt, 16 + source_slot * 4, 4)?,
            "source glyph table changed"
        );
        glyph_receipts.push(json!({"character":c,"source_slot":source_slot,"output_slot":slot,"width":half(fnt,18 + source_slot * 4)?,"rgba_sha256":sha256(&pixels)}));
    }
    // Decode the completed files independently of the slot numbers used to encode.
    let output_mtx = Mtx::parse(&new_mtx)?;
    for (i, expected) in expected_records.iter().enumerate() {
        let start = output_mtx.records[i].offset;
        let end = if i + 1 == entry_count {
            new_mtx.len()
        } else {
            output_mtx.records[i + 1].offset
        };
        let mut decoded = String::new();
        let mut terminated = false;
        for p in (start..end).step_by(2) {
            let u = half(&new_mtx, p)?;
            if u == 0xffff {
                terminated = true;
                continue;
            }
            ensure!(!terminated, "data after terminator");
            if (u as usize) < output_count {
                decoded.push(
                    char::from_u32(half(&new_fnt, 16 + u as usize * 4)? as u32)
                        .context("output scalar")?,
                );
            } else if u == 0xfffd {
                decoded.push('\n');
            } else {
                ensure!(u == 0xfffe, "unknown output token");
            }
        }
        ensure!(
            terminated && &decoded == expected,
            "text roundtrip mismatch"
        );
    }
    if let Some(packet) = &source_packet {
        let output = crate::text::inspect_pair(&new_fnt, &new_mtx)?;
        for (i, entry) in entries.iter().enumerate() {
            segments::verify(entry, &packet["strings"][i], &output["strings"][i])?;
        }
    }
    if config.get("text_relocation").is_some() {
        ensure!(growing, "text move requires font growth");
        let write =
            extent::relocate_pair(root, iso, config, &writes, &new_fnt, &new_mtx, fixed_copy)?;
        writes = vec![write];
    } else {
        if growing {
            let padding = source::read_extent(
                iso,
                writes[0].offset + writes[0].before.len() as u64,
                growth_bytes,
            )?;
            ensure!(
                padding.iter().all(|b| *b == 0)
                    && config["font_growth"]["padding_sha256"] == sha256(&padding),
                "font growth padding mismatch"
            );
            writes[0].before.extend_from_slice(&padding);
        }
        writes[0].after = new_fnt;
        writes[1].after = new_mtx;
    }
    Ok((
        writes,
        json!({"translation":config["translation"],"text_relocation":config["text_relocation"],"translation_identity":loaded.identity,"glyphs":map.len(),"preserved_glyphs":glyph_receipts,"source_glyph_capacity":count,"glyph_capacity":output_count,"reclaimed_file_info_bytes":if growing {0} else {reclaim},"font_growth_bytes":if growing {growth_bytes} else {0},"entries":entry_count,"translated_entries":translated_entries,"text_encoding":if segmented {"source_segments"} else {"plain_lines"},"preserved_empty_tail":empty_tail,"text_layout":text_layout,"font_copy_fixed":fixed_copy.is_some(),"image_height":height,"advance":13}),
    ))
}

#[cfg(test)]
mod tests;
