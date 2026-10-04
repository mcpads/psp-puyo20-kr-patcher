//! SNT cell writes, SNC geometry and protected pixel validation.
use super::*;

pub(super) fn validate_snt_members(edits: &[SntEdit], gims: &[GimPatch]) -> Result<()> {
    let mut names = BTreeSet::new();
    for edit in edits {
        ensure!(names.insert(&edit.member), "duplicate SNT member edit");
    }
    for gim in gims {
        ensure!(
            !names.contains(&gim.member),
            "SNT and GIM writers target the same member"
        );
    }
    Ok(())
}

pub(super) fn apply_snt_member(
    root: &Path,
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    after: Vec<u8>,
    archive: SntEdit,
) -> Result<(Vec<u8>, Value)> {
    ensure!(
        (!archive.patches.is_empty() || !archive.prepared_dxt.is_empty())
            && !archive.snc_members.is_empty(),
        "empty graphic edit"
    );
    let snt = member(zip, &archive.member)?;
    ensure!(
        sha256(&snt) == archive.member_sha256,
        "graphic SNT identity mismatch"
    );
    let ids: Vec<_> = archive.patches.iter().map(|p| p.cell_id).collect();
    let mut cell_table = None;
    let mut checked = BTreeSet::new();
    for record in &archive.snc_members {
        ensure!(
            checked.insert(record.path.clone()),
            "duplicate SNC declaration"
        );
        let bytes = member(zip, &record.path)?;
        ensure!(sha256(&bytes) == record.sha256, "SNC identity mismatch");
        let table = snc::inspect(&snt, &bytes, None)?;
        let separated = separated_writers(&table, &archive.patches)?;
        let clipped: Vec<_> = archive
            .patches
            .iter()
            .filter(|p| p.clipped)
            .map(|p| p.cell_id)
            .collect();
        if ids.is_empty() {
            ensure!(
                archive.shared_views.is_empty(),
                "shared views require PNG cell writers"
            );
        } else {
            table.validate_selection_with(&ids, &archive.shared_views, &separated, &clipped)?;
        }
        ensure!(
            table.cell_sha256 == archive.cell_table_sha256,
            "SNC cell table changed"
        );
        cell_table = Some(table);
    }
    let cell_table = cell_table.context("no SNC cells")?;
    let textures = graphics::snt_table(&snt)?;
    let mut images = BTreeMap::new();
    let mut patch_receipts = Vec::new();
    for patch in &archive.patches {
        let cell = &cell_table.cells[patch.cell_id];
        let (offset, size) = textures[cell.slot];
        if let std::collections::btree_map::Entry::Vacant(e) = images.entry(cell.slot) {
            e.insert(graphics::decode(graphics::bytes(&snt, offset, size)?)?);
        }
        let image = images.get_mut(&cell.slot).context("missing texture")?;
        let path = root.join(&patch.png);
        ensure!(
            sha256(&fs::read(&path)?) == patch.png_sha256,
            "graphic PNG identity mismatch"
        );
        let replacement = authoring::png_read(&path)?;
        let [x, y, w, h] = if patch.clipped {
            clip_rect(cell.rect_xywh, image.width, image.height)?
        } else {
            cell.rect_xywh.map(|n| n as usize)
        };
        replace_cell(
            image,
            &replacement,
            [x, y, w, h],
            patch.allowed_rects.as_deref(),
        )?;
        patch_receipts.push(json!({"cell_id":patch.cell_id,"slot":cell.slot,"rect_xywh":cell.rect_xywh,"png_sha256":patch.png_sha256,"allowed_rects":patch.allowed_rects}));
    }
    let mut dimensions = vec![(0, 0); textures.len()];
    for region in &archive.unmapped_regions {
        let &(offset, size) = textures
            .get(region.slot)
            .context("unmapped texture slot outside SNT")?;
        if dimensions[region.slot] == (0, 0) {
            let image = graphics::decode(graphics::bytes(&snt, offset, size)?)?;
            dimensions[region.slot] = (image.width, image.height);
        }
    }
    validate_unmapped_regions(&cell_table, &archive.unmapped_regions, &dimensions)?;
    let mut unmapped_receipts = Vec::new();
    for region in &archive.unmapped_regions {
        let (offset, size) = textures[region.slot];
        if let std::collections::btree_map::Entry::Vacant(e) = images.entry(region.slot) {
            e.insert(graphics::decode(graphics::bytes(&snt, offset, size)?)?);
        }
        let path = root.join(&region.png);
        ensure!(
            sha256(&fs::read(&path)?) == region.png_sha256,
            "unmapped PNG identity mismatch"
        );
        let replacement = authoring::png_read(&path)?;
        replace_cell(
            images.get_mut(&region.slot).context("missing texture")?,
            &replacement,
            region.rect_xywh,
            None,
        )?;
        unmapped_receipts.push(
            json!({"slot":region.slot,"rect_xywh":region.rect_xywh,"png_sha256":region.png_sha256}),
        );
    }
    let mut output = snt.clone();
    let mut prepared_receipts = Vec::new();
    let mut prepared_slots = BTreeSet::new();
    for prepared in &archive.prepared_dxt {
        ensure!(
            prepared_slots.insert(prepared.slot) && !images.contains_key(&prepared.slot),
            "duplicate or mixed DXT slot writers"
        );
        let &(offset, size) = textures
            .get(prepared.slot)
            .context("prepared DXT slot outside SNT")?;
        let original = &snt[offset..offset + size];
        ensure!(
            sha256(original) == prepared.source_gim_sha256,
            "prepared DXT source identity mismatch"
        );
        let candidate = fs::read(root.join(&prepared.gim))?;
        ensure!(
            sha256(&candidate) == prepared.gim_sha256,
            "prepared DXT candidate identity mismatch"
        );
        let decoded =
            graphics::validate_prepared_dxt(original, &candidate, &prepared.allowed_rects)?;
        ensure!(
            sha256(&decoded.rgba) == prepared.decoded_rgba_sha256,
            "prepared DXT decoded pixels differ"
        );
        validate_dxt_cells(&cell_table, prepared)?;
        output[offset..offset + size].copy_from_slice(&candidate);
        prepared_receipts.push(json!({"slot":prepared.slot,"source_gim_sha256":prepared.source_gim_sha256,"gim_sha256":prepared.gim_sha256,"decoded_rgba_sha256":prepared.decoded_rgba_sha256,"allowed_rects":prepared.allowed_rects,"affected_cell_ids":prepared.affected_cell_ids}));
    }
    for (slot, image) in images {
        let (offset, size) = textures[slot];
        let encoded = graphics::encode(
            &snt[offset..offset + size],
            image.width,
            image.height,
            &image.rgba,
        )?;
        ensure!(encoded.len() == size, "GIM size changed");
        output[offset..offset + size].copy_from_slice(&encoded);
    }

    let (after, zip_receipt) = zip_patch::replace(&after, &archive.member, &output)?;
    let mut after = after;
    let mut geometry_receipts = Vec::new();
    let mut edited_snc = BTreeSet::new();
    for edit in archive.snc_geometry {
        ensure!(
            checked.contains(&edit.member) && edited_snc.insert(edit.member.clone()),
            "unverified or duplicate SNC geometry member"
        );
        let bytes = member(zip, &edit.member)?;
        let (changed, receipt) = snc::edit_draws(&bytes, &edit.draws)?;
        let (next, packed) = zip_patch::replace(&after, &edit.member, &changed)?;
        after = next;
        geometry_receipts.push(json!({"member":edit.member,"geometry":receipt,"zip":packed}));
    }

    let mut remap_receipts = Vec::new();
    for edit in archive.snc_cell_remaps {
        ensure!(
            checked.contains(&edit.member) && edited_snc.insert(edit.member.clone()),
            "unverified or duplicate SNC remap member"
        );
        let bytes = member(zip, &edit.member)?;
        let (changed, receipt) = snc::remap_cells(&bytes, &edit.draws)?;
        let (next, packed) = zip_patch::replace(&after, &edit.member, &changed)?;
        after = next;
        remap_receipts.push(json!({"member":edit.member,"remaps":receipt,"zip":packed}));
    }
    let receipt = json!({"member":archive.member,"source_snt_sha256":archive.member_sha256,"output_snt_sha256":sha256(&output),"patches":patch_receipts,"unmapped_regions":unmapped_receipts,"prepared_dxt":prepared_receipts,"shared_views":archive.shared_views,"zip":zip_receipt,"snc_geometry":geometry_receipts,"snc_cell_remaps":remap_receipts});
    Ok((after, receipt))
}

// Validate every changed pixel before mutating the atlas.
pub(super) fn clip_rect([x, y, w, h]: [i64; 4], tw: usize, th: usize) -> Result<[usize; 4]> {
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(tw as i64), (y + h).min(th as i64));
    ensure!(x1 > x0 && y1 > y0, "clipped cell is empty");
    Ok([
        x0 as usize,
        y0 as usize,
        (x1 - x0) as usize,
        (y1 - y0) as usize,
    ])
}

// Selected writers whose allowed rectangles avoid the other overlapping cell entirely.
pub(super) fn separated_writers(
    table: &snc::CellTable,
    patches: &[Patch],
) -> Result<Vec<[usize; 2]>> {
    let texture_rects = |patch: &Patch| -> Result<Vec<[usize; 4]>> {
        let cell = table
            .cells
            .get(patch.cell_id)
            .context("unknown selected cell")?;
        let [x, y, w, h] = cell.rect_xywh.map(|n| n.max(0) as usize);
        Ok(match &patch.allowed_rects {
            None => vec![[x, y, w, h]],
            Some(rects) => rects
                .iter()
                .map(|[rx, ry, rw, rh]| [x + rx, y + ry, *rw, *rh])
                .collect(),
        })
    };
    let touches = |rects: &[[usize; 4]], other: usize| {
        let [ox, oy, ow, oh] = table.cells[other].rect_xywh.map(|n| n.max(0) as usize);
        rects
            .iter()
            .any(|&[x, y, w, h]| x < ox + ow && ox < x + w && y < oy + oh && oy < y + h)
    };
    let mut pairs = Vec::new();
    for &[a, b] in &table.overlapping_pairs {
        let pa = patches.iter().find(|p| p.cell_id == a);
        let pb = patches.iter().find(|p| p.cell_id == b);
        if pa.is_none() && pb.is_none() {
            continue;
        }
        let ra = pa.map(&texture_rects).transpose()?.unwrap_or_default();
        let rb = pb.map(&texture_rects).transpose()?.unwrap_or_default();
        if !touches(&ra, b) && !touches(&rb, a) {
            pairs.push([a, b]);
        }
    }
    Ok(pairs)
}

pub(super) fn replace_cell(
    atlas: &mut graphics::Image,
    replacement: &graphics::Image,
    cell: [usize; 4],
    allowed: Option<&[[usize; 4]]>,
) -> Result<()> {
    let [x, y, w, h] = cell;
    ensure!(
        replacement.width == w && replacement.height == h,
        "cell PNG size mismatch"
    );
    ensure!(
        x.checked_add(w).is_some_and(|end| end <= atlas.width)
            && y.checked_add(h).is_some_and(|end| end <= atlas.height),
        "cell outside atlas"
    );
    let full = [[0, 0, w, h]];
    let rects = allowed.unwrap_or(&full);
    ensure!(!rects.is_empty(), "empty cell edit mask");
    let mut mask = vec![false; w * h];
    for &[rx, ry, rw, rh] in rects {
        ensure!(
            rw > 0
                && rh > 0
                && rx.checked_add(rw).is_some_and(|end| end <= w)
                && ry.checked_add(rh).is_some_and(|end| end <= h),
            "edit rectangle outside cell"
        );
        for yy in ry..ry + rh {
            for xx in rx..rx + rw {
                ensure!(!mask[yy * w + xx], "overlapping cell edit rectangles");
                mask[yy * w + xx] = true;
            }
        }
    }
    ensure!(
        replacement.rgba.len() == w * h * 4 && atlas.rgba.len() == atlas.width * atlas.height * 4,
        "pixel length mismatch"
    );
    for row in 0..h {
        for col in 0..w {
            let src = ((y + row) * atlas.width + x + col) * 4;
            let dst = (row * w + col) * 4;
            ensure!(
                mask[row * w + col] || atlas.rgba[src..src + 4] == replacement.rgba[dst..dst + 4],
                "protected cell pixel changed at {col},{row}"
            );
        }
    }
    for row in 0..h {
        let at = ((y + row) * atlas.width + x) * 4;
        atlas.rgba[at..at + w * 4]
            .copy_from_slice(&replacement.rgba[row * w * 4..(row + 1) * w * 4]);
    }
    Ok(())
}

pub(super) fn validate_unmapped_regions(
    table: &snc::CellTable,
    regions: &[UnmappedRegion],
    dimensions: &[(usize, usize)],
) -> Result<()> {
    let overlaps = |a: [i64; 4], b: [i64; 4]| {
        a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
    };
    for (i, region) in regions.iter().enumerate() {
        let &(tw, th) = dimensions
            .get(region.slot)
            .context("unmapped texture slot outside SNT")?;
        let [x, y, w, h] = region.rect_xywh;
        ensure!(
            w > 0
                && h > 0
                && x.checked_add(w).is_some_and(|v| v <= tw)
                && y.checked_add(h).is_some_and(|v| v <= th),
            "unmapped rectangle outside texture"
        );
        let rect = region.rect_xywh.map(|v| v as i64);
        ensure!(
            !table
                .cells
                .iter()
                .any(|cell| cell.slot == region.slot && overlaps(rect, cell.rect_xywh)),
            "unmapped region overlaps SNC cell"
        );
        ensure!(
            !regions[..i].iter().any(|other| other.slot == region.slot
                && overlaps(rect, other.rect_xywh.map(|v| v as i64))),
            "overlapping unmapped writers"
        );
    }
    Ok(())
}

// Explicitly enumerate every stored cell touched by the block-aligned write.
// Compression rectangles can include padding outside a cell, but not an undeclared view.
pub(super) fn validate_dxt_cells(table: &snc::CellTable, patch: &PreparedDxt) -> Result<()> {
    let declared: BTreeSet<_> = patch.affected_cell_ids.iter().copied().collect();
    ensure!(
        declared.len() == patch.affected_cell_ids.len() && !declared.is_empty(),
        "empty or duplicate prepared DXT cell declaration"
    );
    let actual: BTreeSet<_> = table
        .cells
        .iter()
        .filter(|cell| {
            cell.slot == patch.slot
                && patch.allowed_rects.iter().any(|&[x, y, w, h]| {
                    let [cx, cy, cw, ch] = cell.rect_xywh;
                    cx < (x + w) as i64
                        && (x as i64) < cx + cw
                        && cy < (y + h) as i64
                        && (y as i64) < cy + ch
                })
        })
        .map(|cell| cell.id)
        .collect();
    ensure!(declared == actual, "prepared DXT affected cells mismatch");
    ensure!(
        table
            .cells
            .iter()
            .filter(|cell| declared.contains(&cell.id))
            .all(|cell| cell.in_bounds),
        "prepared DXT affects out-of-bounds cell"
    );
    Ok(())
}
