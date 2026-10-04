//! Stored SNC cell rectangles. Parsing does not establish animation selection.
mod draws;
mod relocation;
use crate::{graphics, sha256};
use anyhow::{Context, Result, ensure};
pub use draws::{CellRemap, DrawEdit, edit_draws, named_draws, remap_cells};
pub use relocation::{CellRelocation, relocate_cells};
use serde::{Deserialize, Serialize};

/// A read-only subrectangle deliberately affected by its sole owning writer.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SharedView {
    pub cell_id: usize,
    pub owner_cell_id: usize,
    #[serde(default)]
    pub partial_overlap: bool,
}

#[derive(Debug, Serialize)]
pub struct Cell {
    pub id: usize,
    pub slot: usize,
    pub rect_xywh: [i64; 4],
    pub in_bounds: bool,
    pub raw_hex: String,
}
#[derive(Serialize)]
pub struct CellTable {
    pub cell_offset: usize,
    pub cell_sha256: String,
    pub cells: Vec<Cell>,
    pub overlapping_pairs: Vec<[usize; 2]>,
}
fn word(b: &[u8], p: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(graphics::bytes(b, p, 4)?.try_into()?) as usize)
}
fn overlaps(a: &Cell, b: &Cell) -> bool {
    let [x, y, w, h] = a.rect_xywh;
    let [xx, yy, ww, hh] = b.rect_xywh;
    a.slot == b.slot && x < xx + ww && xx < x + w && y < yy + hh && yy < y + h
}
impl CellTable {
    pub fn parse(b: &[u8], dimensions: &[(usize, usize)]) -> Result<Self> {
        ensure!(graphics::bytes(b, 0, 4)? == b"NUIF", "not SNC NUIF");
        let base = word(b, 12)?;
        graphics::bytes(b, base, 52)?;
        ensure!(graphics::bytes(b, base, 4)? == b"nCSC", "not nCSC");
        ensure!(
            word(b, base + 36)? == dimensions.len(),
            "texture count mismatch"
        );
        let count = word(b, base + 44)?;
        ensure!(count > 0 && count <= 10000, "invalid cell count");
        let offset = base
            .checked_add(word(b, base + 48)?)
            .context("cell offset overflow")?;
        ensure!(offset >= base + 52, "cell table overlaps header");
        let raw = graphics::bytes(b, offset, count * 20)?;
        let mut cells = Vec::with_capacity(count);
        for (id, record) in raw.as_chunks::<20>().0.iter().enumerate() {
            let slot = word(record, 0)?;
            let &(w, h) = dimensions.get(slot).context("cell texture outside table")?;
            ensure!(
                w > 0 && h > 0 && w <= 4096 && h <= 4096,
                "invalid texture dimensions"
            );
            let mut p = [0i64; 4];
            for (i, scale) in [w, h, w, h].into_iter().enumerate() {
                let f = f32::from_le_bytes(record[4 + i * 4..8 + i * 4].try_into()?);
                let value = f64::from(f) * scale as f64;
                ensure!(
                    value.is_finite()
                        && value.abs() <= i32::MAX as f64
                        && (value - value.round()).abs() < 0.0001,
                    "noninteger or invalid cell UV"
                );
                p[i] = value.round() as i64;
            }
            let [x, y, right, bottom] = p;
            ensure!(right > x && bottom > y, "empty or reversed cell");
            cells.push(Cell {
                id,
                slot,
                rect_xywh: [x, y, right - x, bottom - y],
                in_bounds: x >= 0 && y >= 0 && right <= w as i64 && bottom <= h as i64,
                raw_hex: record.iter().map(|b| format!("{b:02x}")).collect(),
            });
        }
        let mut overlapping_pairs = Vec::new();
        for (i, a) in cells.iter().enumerate() {
            for b in &cells[i + 1..] {
                if overlaps(a, b) {
                    overlapping_pairs.push([a.id, b.id]);
                }
            }
        }
        Ok(Self {
            cell_offset: offset,
            cell_sha256: sha256(raw),
            cells,
            overlapping_pairs,
        })
    }

    /// Selected cells must be unique, bounded and isolated from all other cells.
    /// Shared/partial cells require an explicit authoring policy before editing.
    pub fn validate_isolated_selection(&self, ids: &[usize]) -> Result<()> {
        self.validate_selection(ids, &[])
    }

    pub fn validate_selection(&self, ids: &[usize], views: &[SharedView]) -> Result<()> {
        self.validate_selection_with(ids, views, &[], &[])
    }
    /// `separated` lists overlapping pairs whose selected writers leave the other cell untouched.
    pub fn validate_selection_with(
        &self,
        ids: &[usize],
        views: &[SharedView],
        separated: &[[usize; 2]],
        clipped: &[usize],
    ) -> Result<()> {
        let mut selected = std::collections::BTreeSet::new();
        ensure!(!ids.is_empty(), "empty cell selection");
        for &id in ids {
            ensure!(selected.insert(id), "duplicate selected cell");
            ensure!(
                self.cells
                    .get(id)
                    .context("unknown selected cell")?
                    .in_bounds
                    || clipped.contains(&id),
                "selected cell outside texture"
            );
        }
        let mut declared = std::collections::BTreeSet::new();
        let mut view_ids = std::collections::BTreeSet::new();
        for view in views {
            ensure!(
                selected.contains(&view.owner_cell_id),
                "shared view owner is not selected"
            );
            ensure!(
                !selected.contains(&view.cell_id),
                "shared view cannot also be a writer"
            );
            ensure!(view_ids.insert(view.cell_id), "duplicate shared view");
            let owner = &self.cells[view.owner_cell_id];
            let cell = self
                .cells
                .get(view.cell_id)
                .context("unknown shared view")?;
            let [x, y, w, h] = owner.rect_xywh;
            let [vx, vy, vw, vh] = cell.rect_xywh;
            ensure!(
                cell.in_bounds
                    && cell.slot == owner.slot
                    && overlaps(owner, cell)
                    && (view.partial_overlap
                        || (vx >= x && vy >= y && vx + vw <= x + w && vy + vh <= y + h)),
                "shared view must be contained or explicitly declare partial overlap"
            );
            declared.insert([
                view.owner_cell_id.min(view.cell_id),
                view.owner_cell_id.max(view.cell_id),
            ]);
        }
        let mut used = std::collections::BTreeSet::new();
        for &[a, b] in &self.overlapping_pairs {
            if (selected.contains(&a) || selected.contains(&b)) && separated.contains(&[a, b]) {
                continue;
            }
            if selected.contains(&a) || selected.contains(&b) {
                ensure!(
                    declared.contains(&[a, b]),
                    "selected cell overlaps an undeclared cell"
                );
                used.insert([a, b]);
            }
        }
        ensure!(used == declared, "unused shared view declaration");
        Ok(())
    }
}

pub fn inspect(snt: &[u8], snc: &[u8], selected: Option<&[usize]>) -> Result<CellTable> {
    let dimensions = graphics::snt_table(snt)?
        .into_iter()
        .map(|(offset, size)| {
            let image = graphics::decode(graphics::bytes(snt, offset, size)?)?;
            Ok((image.width, image.height))
        })
        .collect::<Result<Vec<_>>>()?;
    let table = CellTable::parse(snc, &dimensions)?;
    if let Some(ids) = selected {
        table.validate_isolated_selection(ids)?;
    }
    Ok(table)
}

#[cfg(test)]
mod tests;
