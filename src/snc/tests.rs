use super::*;
#[test]
fn relocation_preserves_shared_source_and_non_cell_bytes() {
    let b = fixture(&[[0., 0., 0.5, 0.5], [0.25, 0.25, 0.5, 0.75]]);
    let edit = CellRelocation {
        cell_id: 0,
        source_slot: 0,
        source_rect: [0, 0, 32, 32],
        target_slot: 0,
        target_origin: [32, 32],
    };
    let moved = relocate_cells(&b, &[(64, 64)], &[edit]).unwrap();
    assert_eq!(&b[..68], &moved[..68]);
    assert_eq!(&b[88..], &moved[88..]);
    let t = CellTable::parse(&moved, &[(64, 64)]).unwrap();
    assert_eq!(t.cells[0].rect_xywh, [32, 32, 32, 32]);
    assert_eq!(t.cells[1].rect_xywh, [16, 16, 16, 32]);
}

#[test]
fn relocation_rejects_collisions_stale_sources_and_overflow() {
    let b = fixture(&[[0., 0., 0.25, 0.25], [0.25, 0., 0.5, 0.25]]);
    let edit = CellRelocation {
        cell_id: 0,
        source_slot: 0,
        source_rect: [0, 0, 16, 16],
        target_slot: 0,
        target_origin: [32, 32],
    };
    assert!(relocate_cells(&b, &[(64, 64)], &[edit.clone(), edit.clone()]).is_err());
    let mut other = edit.clone();
    other.cell_id = 1;
    other.source_rect = [16, 0, 16, 16];
    assert!(relocate_cells(&b, &[(64, 64)], &[edit.clone(), other]).is_err());
    for origin in [[0, 0], [16, 0], [usize::MAX, 0], [49, 32]] {
        let mut bad = edit.clone();
        bad.target_origin = origin;
        assert!(relocate_cells(&b, &[(64, 64)], &[bad]).is_err());
    }
    let mut bad = edit;
    bad.source_rect[0] = 1;
    assert!(relocate_cells(&b, &[(64, 64)], &[bad]).is_err());
}
fn fixture(rects: &[[f32; 4]]) -> Vec<u8> {
    let mut b = vec![0; 68];
    b[..4].copy_from_slice(b"NUIF");
    b[12..16].copy_from_slice(&16u32.to_le_bytes());
    b[16..20].copy_from_slice(b"nCSC");
    b[52..56].copy_from_slice(&1u32.to_le_bytes());
    b[60..64].copy_from_slice(&(rects.len() as u32).to_le_bytes());
    b[64..68].copy_from_slice(&52u32.to_le_bytes());
    for rect in rects {
        b.extend(0u32.to_le_bytes());
        for value in rect {
            b.extend(value.to_le_bytes());
        }
    }
    b
}
#[test]
fn clips_are_reported_but_not_approved_and_overlaps_block_edits() {
    let b = fixture(&[
        [0., 0., 0.25, 0.25],
        [0.25, 0., 0.5, 0.25],
        [-0.03125, 0.5, 1., 1.5],
    ]);
    let t = CellTable::parse(&b, &[(64, 64)]).unwrap();
    assert_eq!(t.cells[2].rect_xywh, [-2, 32, 66, 64]);
    assert!(!t.cells[2].in_bounds);
    t.validate_isolated_selection(&[0, 1]).unwrap();
    assert!(t.validate_isolated_selection(&[2]).is_err());
    assert!(t.validate_isolated_selection(&[0, 0]).is_err());
    let t = CellTable::parse(
        &fixture(&[[0., 0., 0.5, 0.5], [0.25, 0.25, 1., 1.]]),
        &[(64, 64)],
    )
    .unwrap();
    assert_eq!(t.overlapping_pairs, [[0, 1]]);
    assert!(t.validate_isolated_selection(&[0]).is_err());
}
#[test]
fn malformed_tables_and_nonfinite_uvs_fail() {
    let b = fixture(&[[0., 0., 1., 1.]]);
    assert!(CellTable::parse(&b[..b.len() - 1], &[(64, 64)]).is_err());
    assert!(CellTable::parse(&b, &[]).is_err());
    for rect in [
        [f32::NAN, 0., 1., 1.],
        [0., 0., f32::INFINITY, 1.],
        [0., 0., 0.1, 1.],
        [1., 0., 0., 1.],
    ] {
        assert!(CellTable::parse(&fixture(&[rect]), &[(64, 64)]).is_err());
    }
}

#[test]
fn shared_view_requires_one_owner_and_complete_overlap_declarations() {
    let t = CellTable::parse(
        &fixture(&[
            [0., 0., 1., 0.5],
            [0., 0., 0.5, 0.5],
            [0.5, 0., 1., 0.5],
            [0., 0.5, 1., 1.],
        ]),
        &[(64, 64)],
    )
    .unwrap();
    let left = SharedView {
        cell_id: 1,
        owner_cell_id: 0,
        partial_overlap: false,
    };
    let right = SharedView {
        cell_id: 2,
        owner_cell_id: 0,
        partial_overlap: false,
    };
    assert!(t.validate_selection(&[0], &[]).is_err());
    assert!(t.validate_selection(&[0], &[left]).is_err());
    let views = [
        SharedView {
            cell_id: 1,
            owner_cell_id: 0,
            partial_overlap: false,
        },
        right,
    ];
    t.validate_selection(&[0, 3], &views).unwrap();
    assert!(t.validate_selection(&[0, 1], &views).is_err());
    assert!(t.validate_selection(&[3], &views).is_err());
    assert!(
        t.validate_selection(
            &[0],
            &[
                SharedView {
                    cell_id: 1,
                    owner_cell_id: 0,
                    partial_overlap: false,
                },
                SharedView {
                    cell_id: 1,
                    owner_cell_id: 0,
                    partial_overlap: false,
                },
            ]
        )
        .is_err()
    );
}

#[test]
fn shared_view_rejects_crossing_but_accepts_declared_exact_alias() {
    let view = [SharedView {
        cell_id: 1,
        owner_cell_id: 0,
        partial_overlap: false,
    }];
    let crossing = CellTable::parse(
        &fixture(&[[0., 0., 0.5, 0.5], [0.25, 0., 0.75, 0.5]]),
        &[(64, 64)],
    )
    .unwrap();
    assert!(crossing.validate_selection(&[0], &view).is_err());
    let alias = CellTable::parse(
        &fixture(&[[0., 0., 0.5, 0.5], [0., 0., 0.5, 0.5]]),
        &[(64, 64)],
    )
    .unwrap();
    alias.validate_selection(&[0], &view).unwrap();
    assert!(alias.validate_selection(&[0], &[]).is_err());
    assert!(alias.validate_selection(&[0, 1], &[]).is_err());
}

#[test]
fn separated_overlapping_writers_need_explicit_pair() {
    let t = CellTable::parse(
        &fixture(&[[0., 0., 1., 0.5], [0., 0.46875, 1., 1.]]),
        &[(64, 64)],
    )
    .unwrap();
    assert_eq!(t.overlapping_pairs, vec![[0, 1]]);
    assert!(t.validate_selection(&[0, 1], &[]).is_err());
    t.validate_selection_with(&[0, 1], &[], &[[0, 1]], &[])
        .unwrap();
}

#[test]
fn partial_shared_view_requires_explicit_overlap_and_one_writer() {
    let table = CellTable::parse(
        &fixture(&[
            [0., 0., 0.5, 0.5],
            [0.25, 0.25, 0.75, 0.75],
            [0.75, 0.75, 1., 1.],
        ]),
        &[(64, 64)],
    )
    .unwrap();
    let mut view = SharedView {
        cell_id: 1,
        owner_cell_id: 0,
        partial_overlap: false,
    };
    assert!(
        table
            .validate_selection(&[0], std::slice::from_ref(&view))
            .is_err()
    );
    view.partial_overlap = true;
    table
        .validate_selection(&[0], std::slice::from_ref(&view))
        .unwrap();
    assert!(
        table
            .validate_selection(&[0, 1], std::slice::from_ref(&view))
            .is_err()
    );
    view.cell_id = 2;
    assert!(table.validate_selection(&[0], &[view]).is_err());
}
