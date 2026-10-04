//! Stored named draw records in the observed SNC group layout.
use super::word;
use crate::{graphics, sha256};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawEdit {
    pub name: String,
    pub expected_cells: Vec<usize>,
    pub expected_half_size: [f32; 2],
    pub half_size: [f32; 2],
    pub expected_center_x: f32,
    pub center_x: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellRemap {
    pub name: String,
    pub selector_slot: usize,
    pub expected_cell: usize,
    pub cell: usize,
    #[serde(default)]
    pub local_x: Option<DrawX>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawX {
    pub expected: f32,
    pub value: f32,
}

pub fn remap_cells(before: &[u8], edits: &[CellRemap]) -> Result<(Vec<u8>, Value)> {
    ensure!(!edits.is_empty(), "empty SNC cell remap");
    let report = named_draws(before)?;
    let nodes = report["draws"].as_array().context("SNC draws")?;
    let count = report["cell_count"].as_u64().context("SNC cell count")? as usize;
    let mut after = before.to_vec();
    let mut offsets = BTreeSet::new();
    let mut receipts = Vec::new();
    for e in edits {
        ensure!(
            e.selector_slot < 32 && e.cell < count,
            "invalid SNC cell remap"
        );
        let node = nodes
            .iter()
            .find(|n| n["name"] == e.name)
            .context("unknown SNC draw name")?;
        let at = node["selector_offset"]
            .as_u64()
            .context("SNC selector offset")? as usize
            + 4 * e.selector_slot;
        for byte in at..at + 4 {
            ensure!(offsets.insert(byte), "overlapping SNC cell remap");
        }
        ensure!(
            e.expected_cell < count && word(before, at)? == e.expected_cell,
            "SNC expected cell mismatch"
        );
        after[at..at + 4].copy_from_slice(&(e.cell as u32).to_le_bytes());
        let mut position_receipt = Value::Null;
        if let Some(x) = &e.local_x {
            let position = node["transform_offset"]
                .as_u64()
                .context("SNC transform offset")? as usize
                + 4;
            ensure!(
                x.expected.is_finite()
                    && x.value.is_finite()
                    && x.value.abs() <= 1.0
                    && floats(before, position, 1)? == [x.expected],
                "SNC expected local x mismatch or invalid position"
            );
            for byte in position..position + 4 {
                ensure!(offsets.insert(byte), "overlapping SNC local x write");
            }
            after[position..position + 4].copy_from_slice(&x.value.to_le_bytes());
            position_receipt = json!({"offset":position,"before":x.expected,"after":x.value});
        }
        receipts.push(json!({"name":e.name,"offset":at,"selector_slot":e.selector_slot,"before":e.expected_cell,"after":e.cell,"local_x":position_receipt}));
    }
    ensure!(
        before
            .iter()
            .zip(&after)
            .enumerate()
            .all(|(i, (a, b))| a == b || offsets.contains(&i)),
        "unexplained SNC remap diff"
    );
    named_draws(&after)?;
    Ok((
        after,
        json!({"source_sha256":sha256(before),"edits":receipts}),
    ))
}

fn quad([x, y]: [f32; 2]) -> Result<[f32; 8]> {
    ensure!(
        x.is_finite() && y.is_finite() && x > 0.0 && y > 0.0 && x <= 1.0 && y <= 1.0,
        "invalid SNC half size"
    );
    Ok([-x, -y, -x, y, x, -y, x, y])
}

pub fn edit_draws(before: &[u8], edits: &[DrawEdit]) -> Result<(Vec<u8>, Value)> {
    ensure!(!edits.is_empty(), "empty SNC geometry edits");
    let report = named_draws(before)?;
    let nodes = report["draws"].as_array().context("SNC draws")?;
    let mut names = BTreeSet::new();
    let mut output = before.to_vec();
    let mut writes = BTreeSet::new();
    let mut receipts = Vec::new();
    for e in edits {
        ensure!(names.insert(&e.name), "duplicate SNC geometry edit");
        let node = nodes
            .iter()
            .find(|n| n["name"] == e.name)
            .context("unknown SNC draw name")?;
        let cells: Vec<_> = node["cells"]
            .as_array()
            .context("SNC cells")?
            .iter()
            .map(|c| c["cell"].as_u64().map(|x| x as usize).context("SNC cell"))
            .collect::<Result<_>>()?;
        ensure!(
            cells == e.expected_cells && !cells.is_empty(),
            "SNC draw cell identity mismatch"
        );
        let at = node["quad_offset"].as_u64().context("quad offset")? as usize;
        let center = node["transform_offset"]
            .as_u64()
            .context("transform offset")? as usize
            + 4;
        let old_quad = quad(e.expected_half_size)?;
        let new_quad = quad(e.half_size)?;
        ensure!(
            floats(before, at, 8)? == old_quad
                && floats(before, center, 1)? == [e.expected_center_x],
            "SNC expected geometry mismatch"
        );
        ensure!(
            e.center_x.is_finite() && e.center_x.abs() <= 1.0,
            "invalid SNC center"
        );
        for (offset, value) in (0..8)
            .map(|i| (at + i * 4, new_quad[i]))
            .chain([(center, e.center_x)])
        {
            for byte in offset..offset + 4 {
                ensure!(writes.insert(byte), "overlapping SNC geometry writers");
            }
            output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        receipts.push(
            json!({"name":e.name,"cells":cells,"quad_offset":at,"center_x_offset":center,
            "half_size":e.half_size,"center_x":e.center_x}),
        );
    }
    ensure!(
        before
            .iter()
            .zip(&output)
            .enumerate()
            .all(|(i, (a, b))| a == b || writes.contains(&i)),
        "unexplained SNC geometry diff"
    );
    named_draws(&output)?;
    Ok((
        output,
        json!({"source_sha256":sha256(before),"edits":receipts}),
    ))
}

fn relative(b: &[u8], base: usize, at: usize) -> Result<usize> {
    base.checked_add(word(b, at)?)
        .context("SNC pointer overflow")
}

fn floats(b: &[u8], at: usize, count: usize) -> Result<Vec<f32>> {
    graphics::bytes(b, at, count * 4)?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| {
            let f = f32::from_le_bytes(*v);
            ensure!(f.is_finite(), "nonfinite SNC draw value");
            Ok(f)
        })
        .collect()
}

pub fn named_draws(b: &[u8]) -> Result<Value> {
    ensure!(graphics::bytes(b, 0, 4)? == b"NUIF", "not SNC NUIF");
    let base = word(b, 12)?;
    ensure!(graphics::bytes(b, base, 4)? == b"nCSC", "not nCSC");
    let cell_count = word(b, base + 44)?;
    ensure!((1..=10000).contains(&cell_count), "invalid SNC cell count");
    let group_count = word(b, base + 52)?;
    ensure!(
        (1..=10000).contains(&group_count),
        "invalid SNC group count"
    );
    let group = relative(b, base, base + 56)?;
    graphics::bytes(b, group, group_count * 16)?;
    let mut groups = Vec::new();
    let mut total = 0;
    for id in 0..group_count {
        let at = group + id * 16;
        let count = word(b, at)?;
        ensure!((1..=10000).contains(&count), "invalid SNC draw count");
        let pointers = relative(b, base, at + 4)?;
        graphics::bytes(b, pointers, count * 4)?;
        total += count;
        groups.push((count, pointers));
    }
    let name_count = word(b, base + 60)?;
    ensure!(name_count == total, "draw names do not cover groups");
    let names = relative(b, base, base + 64)?;
    graphics::bytes(b, names, name_count * 12)?;
    let mut ids = BTreeSet::new();
    let mut seen_names = BTreeSet::new();
    let mut draws = Vec::new();
    for row in 0..name_count {
        let entry = names + row * 12;
        let name_at = relative(b, base, entry)?;
        let tail = b.get(name_at..).context("SNC name outside file")?;
        let len = tail
            .iter()
            .take(128)
            .position(|&x| x == 0)
            .context("unterminated SNC name")?;
        let name = std::str::from_utf8(&tail[..len])?;
        ensure!(
            !name.is_empty() && seen_names.insert(name),
            "empty or duplicate SNC name"
        );
        let group_id = word(b, entry + 4)?;
        let &(count, pointers) = groups
            .get(group_id)
            .context("SNC name group outside table")?;
        let id = word(b, entry + 8)?;
        ensure!(
            id < count && ids.insert((group_id, id)),
            "invalid or duplicate SNC draw id"
        );
        let at = relative(b, base, pointers + id * 4)?;
        graphics::bytes(b, at, 80)?;
        ensure!(
            word(b, at)? == 1
                && word(b, at + 4)? <= 1
                && word(b, at + 8)? == 1
                && word(b, at + 60)? == 32,
            "unsupported SNC draw record"
        );
        let selector = relative(b, base, at + 64)?;
        graphics::bytes(b, selector, 128)?;
        let mut cells = Vec::new();
        for slot in 0..32 {
            let cell = word(b, selector + slot * 4)?;
            if cell == u32::MAX as usize {
                continue;
            }
            ensure!(cell < cell_count, "draw cell reference outside table");
            cells.push(json!({"slot":slot,"cell":cell}));
        }
        let transform = relative(b, base, at + 48)?;
        draws.push(json!({"name":name,"id":id,"group":group_id,"record_offset":at,"enabled_word":word(b,at+4)?,
            "quad_offset":at+12,"quad_xy":floats(b,at+12,8)?,"selector_offset":selector,"cells":cells,
            "transform_offset":transform,"transform_words_f32":floats(b,transform,7)?}));
    }
    Ok(
        json!({"scope":"stored named records; animation evaluation and transform semantics not interpreted",
        "snc_sha256":sha256(b),"cell_count":cell_count,"group_offset":group,"draws":draws}),
    )
}

#[cfg(test)]
mod tests;
