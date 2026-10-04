use super::*;
fn image(w: usize, h: usize) -> graphics::Image {
    graphics::Image {
        width: w,
        height: h,
        format: 3,
        order: 0,
        rgba: vec![0; w * h * 4],
    }
}
#[test]
fn inner_text_edit_keeps_diagram_and_rejects_partial_mutation() {
    let mut atlas = image(8, 4);
    let mut patch = image(6, 2);
    patch.rgba[0] = 255;
    replace_cell(&mut atlas, &patch, [1, 1, 6, 2], Some(&[[0, 0, 3, 2]])).unwrap();
    assert_eq!(atlas.rgba[(8 + 1) * 4], 255);
    let before = atlas.rgba.clone();
    patch.rgba[0] = 128; // An allowed change must not leak when a later pixel fails.
    patch.rgba[5 * 4] = 255;
    assert!(replace_cell(&mut atlas, &patch, [1, 1, 6, 2], Some(&[[0, 0, 3, 2]])).is_err());
    assert_eq!(atlas.rgba, before);
}
#[test]
fn invalid_edit_rectangles_fail_before_copy() {
    let mut atlas = image(8, 4);
    let patch = image(6, 2);
    for rects in [
        vec![],
        vec![[5, 0, 2, 1]],
        vec![[0, 0, 0, 1]],
        vec![[0, 0, 3, 2], [2, 0, 1, 1]],
    ] {
        assert!(replace_cell(&mut atlas, &patch, [1, 1, 6, 2], Some(&rects)).is_err());
        assert!(atlas.rgba.iter().all(|b| *b == 0));
    }
}

#[test]
fn whole_texture_draft_changes_only_declared_button_interiors() {
    let mut source = image(8, 4);
    let mut draft = image(8, 4);
    draft.rgba.fill(255);
    let rects = [[1, 1, 2, 1], [5, 2, 2, 1]];
    merge_rectangles(&mut source, &draft, &rects, None).unwrap();
    for y in 0..4 {
        for x in 0..8 {
            let expected = if (y == 1 && (1..3).contains(&x)) || (y == 2 && (5..7).contains(&x)) {
                255
            } else {
                0
            };
            assert_eq!(&source.rgba[(y * 8 + x) * 4..][..4], &[expected; 4]);
        }
    }
    let before = source.rgba.clone();
    assert!(merge_rectangles(&mut source, &draft, &[[1, 1, 2, 1], [7, 3, 2, 1]], None).is_err());
    assert_eq!(source.rgba, before);
}

#[test]
fn shifted_text_uses_draft_coordinates_without_moving_button_edges() {
    let mut source = image(8, 4);
    let mut draft = image(12, 8);
    draft.rgba[(5 * 12 + 7) * 4..][..8].fill(200);
    merge_rectangles(&mut source, &draft, &[[2, 1, 2, 1]], Some(&[[7, 5]])).unwrap();
    assert_eq!(&source.rgba[(8 + 2) * 4..][..8], &[200; 8]);
    assert_eq!(source.rgba.iter().filter(|&&v| v == 200).count(), 8);
    let before = source.rgba.clone();
    for origins in [vec![], vec![[11, 7]], vec![[usize::MAX, 0]]] {
        assert!(merge_rectangles(&mut source, &draft, &[[2, 1, 2, 1]], Some(&origins)).is_err());
        assert_eq!(source.rgba, before);
    }
}

#[test]
fn clipped_cells_keep_only_the_texture_intersection() {
    assert_eq!(
        clip_rect([-10, 5, 116, 18], 512, 512).unwrap(),
        [0, 5, 106, 18]
    );
    assert_eq!(
        clip_rect([0, 0, 113, 17], 128, 16).unwrap(),
        [0, 0, 113, 16]
    );
    assert!(clip_rect([130, 0, 10, 10], 128, 16).is_err());
}

#[test]
fn additional_snt_edits_cannot_overwrite_another_member_writer() {
    let edit = |name: &str| SntEdit {
        member: name.into(),
        member_sha256: String::new(),
        snc_members: Vec::new(),
        cell_table_sha256: String::new(),
        patches: Vec::new(),
        unmapped_regions: Vec::new(),
        prepared_dxt: Vec::new(),
        shared_views: Vec::new(),
        snc_geometry: Vec::new(),
        snc_cell_remaps: Vec::new(),
    };
    assert!(validate_snt_members(&[edit("record.snt"), edit("continue.snt")], &[]).is_ok());
    assert!(validate_snt_members(&[edit("record.snt"), edit("record.snt")], &[]).is_err());
    let gim = GimPatch {
        member: "continue.snt".into(),
        member_sha256: String::new(),
        png: String::new(),
        png_sha256: String::new(),
        allowed_rects: Vec::new(),
    };
    assert!(validate_snt_members(&[edit("record.snt"), edit("continue.snt")], &[gim]).is_err());
}

#[test]
fn masked_badge_can_avoid_an_unedited_palette_strip() {
    let cell = |id, rect_xywh| snc::Cell {
        id,
        slot: 0,
        rect_xywh,
        in_bounds: true,
        raw_hex: String::new(),
    };
    let table = snc::CellTable {
        cell_offset: 0,
        cell_sha256: String::new(),
        cells: vec![cell(0, [0, 0, 20, 10]), cell(1, [1, 9, 18, 4])],
        overlapping_pairs: vec![[0, 1]],
    };
    let patch = |height| Patch {
        cell_id: 0,
        png: String::new(),
        png_sha256: String::new(),
        allowed_rects: Some(vec![[0, 0, 20, height]]),
        clipped: false,
    };
    let safe = separated_writers(&table, &[patch(9)]).unwrap();
    table
        .validate_selection_with(&[0], &[], &safe, &[])
        .unwrap();
    let touching = separated_writers(&table, &[patch(10)]).unwrap();
    assert!(
        table
            .validate_selection_with(&[0], &[], &touching, &[])
            .is_err()
    );
}

#[test]
fn unmapped_writes_reject_snc_views_duplicate_writes_and_bad_extents() {
    let table = snc::CellTable {
        cell_offset: 0,
        cell_sha256: String::new(),
        overlapping_pairs: vec![],
        cells: vec![snc::Cell {
            id: 0,
            slot: 0,
            rect_xywh: [0, 0, 8, 4],
            in_bounds: true,
            raw_hex: String::new(),
        }],
    };
    let patch = |rect_xywh| UnmappedRegion {
        slot: 0,
        rect_xywh,
        png: String::new(),
        png_sha256: String::new(),
    };
    assert!(validate_unmapped_regions(&table, &[patch([0, 4, 8, 4])], &[(8, 8)]).is_ok());
    for rect in [
        [0, 3, 8, 4],
        [0, 4, 9, 4],
        [0, 4, 8, 0],
        [usize::MAX, 4, 2, 4],
    ] {
        assert!(validate_unmapped_regions(&table, &[patch(rect)], &[(8, 8)]).is_err());
    }
    assert!(
        validate_unmapped_regions(
            &table,
            &[patch([0, 4, 8, 4]), patch([1, 5, 2, 2])],
            &[(8, 8)]
        )
        .is_err()
    );
    assert!(validate_unmapped_regions(&table, &[patch([0, 4, 8, 4])], &[]).is_err());
}

#[test]
fn prepared_dxt_requires_every_affected_cell_including_shared_views() {
    let cell = |id, slot, rect_xywh| snc::Cell {
        id,
        slot,
        rect_xywh,
        in_bounds: true,
        raw_hex: String::new(),
    };
    let table = snc::CellTable {
        cell_offset: 0,
        cell_sha256: String::new(),
        overlapping_pairs: vec![],
        cells: vec![
            cell(0, 1, [1, 0, 6, 8]),
            cell(1, 1, [6, 0, 2, 8]),
            cell(2, 2, [0, 0, 8, 8]),
        ],
    };
    let mut patch = PreparedDxt {
        slot: 1,
        source_gim_sha256: String::new(),
        gim: String::new(),
        gim_sha256: String::new(),
        decoded_rgba_sha256: String::new(),
        allowed_rects: vec![[0, 0, 8, 8]],
        affected_cell_ids: vec![0, 1],
    };
    assert!(validate_dxt_cells(&table, &patch).is_ok());
    for ids in [vec![], vec![0], vec![0, 1, 1], vec![0, 1, 2], vec![0, 99]] {
        patch.affected_cell_ids = ids;
        assert!(validate_dxt_cells(&table, &patch).is_err());
    }
}
