//! Explicit prose spans between immutable source control tokens.
use crate::{sha256, text};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Deserialize)]
struct Entry {
    group: usize,
    record: usize,
    source_units: Vec<u16>,
    segments: Vec<String>,
    expected_text: String,
}
#[derive(Deserialize)]
struct Translation {
    entries: Vec<Entry>,
}

/// Sorted unique prose characters of the pinned translation named by a story text config.
/// The story font is built from exactly these characters, so edits need no separate list.
pub fn characters(root: &Path, config_path: &Path) -> Result<String> {
    let config: Value = serde_json::from_slice(&fs::read(root.join(config_path))?)?;
    let loaded = crate::translation_batches::load(root, &config)?;
    let trans: Translation = serde_json::from_value(loaded.doc)?;
    let chars: BTreeSet<char> = trans
        .entries
        .iter()
        .flat_map(|e| e.expected_text.chars())
        .filter(|c| *c != '\n')
        .collect();
    ensure!(!chars.is_empty(), "empty story translation");
    Ok(chars.into_iter().collect())
}

#[derive(Deserialize)]
struct ExcludedRecord {
    group: usize,
    record: usize,
    source_units_sha256: String,
    reason: String,
}

fn control_units(tokens: &[Value]) -> Result<Vec<Vec<u16>>> {
    let mut result = Vec::new();
    for token in tokens {
        if let Some(code) = token.get("control") {
            let code = u16::try_from(code.as_u64().context("control code")?)?;
            if code == 0xffff {
                break;
            }
            ensure!(
                // F800 selects a source render attribute; keep its table index
                // opaque and byte-exact, never map it as a glyph or speaker ID.
                [0xfffd, 0xf800, 0xf880, 0xf881, 0xf813].contains(&code),
                "unadopted story control"
            );
            let mut units = vec![code];
            for value in token["operands"].as_array().context("control operands")? {
                units.push(u16::try_from(value.as_u64().context("operand")?)?);
            }
            result.push(units);
        }
    }
    ensure!(
        result.iter().rev().find(|v| v[0] != 0xfffd) == Some(&vec![0xf813]),
        "story end-wait missing"
    );
    Ok(result)
}

fn encode_segments(
    segments: &[String],
    controls: &[Vec<u16>],
    glyphs: &BTreeMap<char, (u16, u16)>,
    max_width: usize,
    max_lines: usize,
) -> Result<(Vec<u16>, String, Vec<usize>)> {
    ensure!(
        segments.len() == controls.len() + 1 && segments.last().is_some_and(String::is_empty),
        "one prose span required on each side of every source control"
    );
    let mut units = Vec::new();
    let mut prose = String::new();
    let mut widths = vec![0];
    for (i, span) in segments.iter().enumerate() {
        for c in span.chars() {
            ensure!(!c.is_control(), "line breaks must use source controls");
            let &(slot, width) = glyphs
                .get(&c)
                .with_context(|| format!("unmapped story character {c}"))?;
            units.push(slot);
            // Original renderer advances by glyph width plus one pixel.
            *widths.last_mut().unwrap() += usize::from(width) + 1;
        }
        prose.push_str(span);
        if let Some(control) = controls.get(i) {
            units.extend(control);
            if control[0] == 0xfffd {
                prose.push('\n');
                widths.push(0);
            }
        }
    }
    ensure!(
        widths.len() <= max_lines && widths.iter().all(|w| *w <= max_width),
        "story authoring layout exceeded"
    );
    units.push(0xffff);
    Ok((units, prose, widths))
}

pub fn build(
    root: &Path,
    config_path: &Path,
    pair: &str,
    source_font: &[u8],
    output_font: &[u8],
    original: &[u8],
    compact: bool,
) -> Result<(Vec<u8>, Value)> {
    let raw = fs::read(root.join(config_path))?;
    let config: Value = serde_json::from_slice(&raw)?;
    ensure!(config["pair"] == pair, "story font/text pair mismatch");
    let loaded = crate::translation_batches::load(root, &config)?;
    let trans: Translation = serde_json::from_value(loaded.doc)?;
    let mtx = text::Mtx::parse(original)?;
    let source_packet = text::inspect_pair(source_font, original)?;
    let output_packet = text::inspect_pair(output_font, original)?;
    ensure!(
        source_packet["summary"]["unresolved_records"] == 0,
        "unresolved story source"
    );
    let groups: BTreeMap<usize, usize> = serde_json::from_value(config["groups"].clone())?;
    let mut selected = selected_records(&mtx, &groups)?;
    let exclusions: Vec<ExcludedRecord> = config
        .get("excluded_records")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()?
        .unwrap_or_default();
    let excluded = exclude_records(&mtx, &mut selected, &exclusions)?;
    ensure!(
        trans.entries.len() == selected.len(),
        "story source population"
    );
    let mut glyphs = BTreeMap::new();
    for (slot, glyph) in output_packet["glyphs"]
        .as_array()
        .context("glyphs")?
        .iter()
        .enumerate()
    {
        let c = char::from_u32(u32::try_from(glyph[0].as_u64().context("glyph scalar")?)?)
            .context("scalar")?;
        glyphs.entry(c).or_insert((
            u16::try_from(slot)?,
            u16::try_from(glyph[1].as_u64().context("glyph width")?)?,
        ));
    }
    let max_width = usize::try_from(config["max_line_advance"].as_u64().context("line bound")?)?;
    let max_lines = usize::try_from(config["max_lines"].as_u64().context("line count")?)?;
    ensure!(
        max_width <= 252 && max_lines <= 3,
        "unsupported story layout policy"
    );
    let mut replacements = BTreeMap::new();
    let mut receipts = Vec::new();
    for (e, &(group, record, i)) in trans.entries.iter().zip(&selected) {
        ensure!(
            e.group == group && e.record == record && e.source_units == mtx.records[i].units,
            "story source/order drift"
        );
        let controls = control_units(
            source_packet["strings"][i]["tokens"]
                .as_array()
                .context("source tokens")?,
        )?;
        let (encoded, prose, widths) =
            encode_segments(&e.segments, &controls, &glyphs, max_width, max_lines)?;
        ensure!(
            prose == e.expected_text,
            "aligned prose differs from translation"
        );
        receipts.push(json!({"group":group,"record":record,"flat_record":i,"line_advance":widths,"controls":controls,"encoded_units":encoded.len()}));
        replacements.insert(i, encoded);
    }
    let mut masked_glyphs = 0;
    if compact {
        let fallback = glyphs.get(&'□').context("missing untranslated marker")?.0;
        for (i, record) in mtx.records.iter().enumerate() {
            if replacements.contains_key(&i) {
                continue;
            }
            let (units, missing) = remap_untranslated(
                &record.units,
                source_packet["strings"][i]["tokens"]
                    .as_array()
                    .context("untranslated tokens")?,
                &glyphs,
                fallback,
            )?;
            masked_glyphs += missing;
            replacements.insert(i, units);
        }
    }
    let output = mtx.replace_records(&replacements)?;
    let readback = text::inspect_pair(output_font, &output)?;
    ensure!(
        readback["summary"]["unresolved_records"] == 0,
        "unresolved output tokens"
    );
    for ((e, &(_, _, i)), receipt) in trans.entries.iter().zip(&selected).zip(&receipts) {
        let tokens = readback["strings"][i]["tokens"]
            .as_array()
            .context("readback tokens")?;
        let controls = control_units(tokens)?;
        ensure!(
            serde_json::to_value(controls)? == receipt["controls"],
            "story control sequence changed"
        );
        let mut prose = String::new();
        for t in tokens {
            if let Some(c) = t.get("character") {
                prose.push_str(c.as_str().context("readback character")?);
            } else if t["control"] == 0xfffd {
                prose.push('\n');
            }
        }
        ensure!(prose == e.expected_text, "story text roundtrip differs");
    }
    // Only the untranslated glyph references may change in a compact development font.
    for i in (0..mtx.records.len()).filter(|i| !selected.iter().any(|s| s.2 == *i)) {
        if compact {
            let actual = &readback["raw_mtx"]["records"][i]["units"];
            let actual: Vec<u16> = serde_json::from_value(actual.clone())?;
            let expected = &replacements[&i];
            let body = |v: &[u16]| v.iter().rposition(|u| *u != 0xffff).map_or(0, |p| p + 1);
            ensure!(
                actual[..body(&actual)] == expected[..body(expected)],
                "untranslated remap changed controls or glyphs"
            );
        } else {
            ensure!(
                readback["strings"][i]["text"] == source_packet["strings"][i]["text"],
                "untranslated story text changed"
            );
        }
    }
    Ok((
        output,
        json!({"scope":"configured story prose drafts; immutable source control order and operands; human review pending", "pair":pair,
        "config_sha256":sha256(&raw),"translation_identity":loaded.identity,"entries":receipts,
        "excluded_records":excluded,
        "untranslated_records_preserved":if compact {0} else {mtx.records.len()-selected.len()},
        "untranslated_records_remapped":if compact {mtx.records.len()-selected.len()} else {0},
        "untranslated_glyphs_masked":masked_glyphs,"size_bytes":original.len(),
        "header_bytes":mtx.header.len(),"original_sha256":sha256(original)}),
    ))
}

fn remap_untranslated(
    units: &[u16],
    tokens: &[Value],
    glyphs: &BTreeMap<char, (u16, u16)>,
    fallback: u16,
) -> Result<(Vec<u16>, usize)> {
    let mut result = units.to_vec();
    let mut missing = 0;
    for token in tokens {
        if let Some(character) = token.get("character") {
            let c = character
                .as_str()
                .and_then(|s| s.chars().next())
                .context("source character")?;
            let at = usize::try_from(
                token["unit_offset"]
                    .as_u64()
                    .context("source glyph offset")?,
            )?;
            let slot = result.get_mut(at).context("source glyph bounds")?;
            ensure!(token["glyph_slot"] == *slot, "source glyph token mismatch");
            *slot = match glyphs.get(&c) {
                Some(&(mapped, _)) => mapped,
                None => {
                    missing += 1;
                    fallback
                }
            };
        }
    }
    Ok((result, missing))
}

// Pin whole-group populations before applying any explicit translation exclusions.
fn selected_records(
    mtx: &text::Mtx,
    groups: &BTreeMap<usize, usize>,
) -> Result<Vec<(usize, usize, usize)>> {
    ensure!(!groups.is_empty(), "empty story selection");
    let mut selected = Vec::new();
    for (&group, &count) in groups {
        let start = *mtx
            .group_first_records
            .get(group)
            .context("story group missing")?;
        let end = mtx
            .group_first_records
            .get(group + 1)
            .copied()
            .unwrap_or(mtx.records.len());
        ensure!(
            count > 0 && end.checked_sub(start) == Some(count),
            "story group population drift"
        );
        selected.extend((0..count).map(|record| (group, record, start + record)));
    }
    Ok(selected)
}

fn exclude_records(
    mtx: &text::Mtx,
    selected: &mut Vec<(usize, usize, usize)>,
    exclusions: &[ExcludedRecord],
) -> Result<Vec<Value>> {
    let mut excluded = BTreeSet::new();
    let mut receipts = Vec::new();
    for e in exclusions {
        let &(_, _, index) = selected
            .iter()
            .find(|&&(g, r, _)| g == e.group && r == e.record)
            .context("excluded record outside selected groups")?;
        ensure!(excluded.insert(index), "duplicate excluded record");
        ensure!(
            !e.reason.trim().is_empty(),
            "excluded record needs a reason"
        );
        let bytes: Vec<u8> = mtx.records[index]
            .units
            .iter()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        ensure!(
            sha256(&bytes) == e.source_units_sha256,
            "excluded record source drift"
        );
        receipts.push(
            json!({"group":e.group,"record":e.record,"flat_record":index,
            "source_units_sha256":e.source_units_sha256,"reason":e.reason}),
        );
    }
    // Keep excluded records in the MTX under the existing untranslated policy.
    // In particular, this never relaxes the end-wait requirement for selected text.
    selected.retain(|&(_, _, i)| !excluded.contains(&i));
    ensure!(
        !selected.is_empty(),
        "empty story selection after exclusions"
    );
    Ok(receipts)
}

#[cfg(test)]
mod tests;
