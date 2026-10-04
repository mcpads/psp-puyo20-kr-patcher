use super::{Cell, CellTable, overlaps};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Moves the sampling rectangle without changing the cell id or draw geometry.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CellRelocation {
    pub cell_id: usize,
    pub source_slot: usize,
    pub source_rect: [i64; 4],
    pub target_slot: usize,
    pub target_origin: [usize; 2],
}

pub fn relocate_cells(
    before: &[u8],
    dimensions: &[(usize, usize)],
    edits: &[CellRelocation],
) -> Result<Vec<u8>> {
    ensure!(!edits.is_empty(), "empty cell relocation");
    let table = CellTable::parse(before, dimensions)?;
    let mut ids = BTreeSet::new();
    let mut targets = Vec::new();
    let mut after = before.to_vec();
    for edit in edits {
        ensure!(ids.insert(edit.cell_id), "duplicate relocated cell");
        let source = table
            .cells
            .get(edit.cell_id)
            .context("missing relocated cell")?;
        ensure!(
            source.in_bounds
                && source.slot == edit.source_slot
                && source.rect_xywh == edit.source_rect,
            "relocation source mismatch"
        );
        let &(width, height) = dimensions
            .get(edit.target_slot)
            .context("target slot outside table")?;
        let [x, y] = edit.target_origin;
        let [_, _, w, h] = source.rect_xywh;
        ensure!(
            x <= width && y <= height && w as usize <= width - x && h as usize <= height - y,
            "relocation target outside texture"
        );
        let target = Cell {
            id: edit.cell_id,
            slot: edit.target_slot,
            rect_xywh: [x as i64, y as i64, w, h],
            in_bounds: true,
            raw_hex: String::new(),
        };
        ensure!(
            table.cells.iter().all(|c| !overlaps(&target, c)),
            "relocation target intersects original cell"
        );
        ensure!(
            targets.iter().all(|c| !overlaps(&target, c)),
            "relocation targets overlap"
        );
        let at = table.cell_offset + edit.cell_id * 20;
        after[at..at + 4].copy_from_slice(&(edit.target_slot as u32).to_le_bytes());
        for (i, v) in [
            x as f32 / width as f32,
            y as f32 / height as f32,
            (x + w as usize) as f32 / width as f32,
            (y + h as usize) as f32 / height as f32,
        ]
        .iter()
        .enumerate()
        {
            after[at + 4 + i * 4..at + 8 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        targets.push(target);
    }
    let parsed = CellTable::parse(&after, dimensions)?;
    for target in targets {
        let actual = &parsed.cells[target.id];
        ensure!(
            actual.slot == target.slot && actual.rect_xywh == target.rect_xywh,
            "relocated UV round trip mismatch"
        );
    }
    Ok(after)
}
