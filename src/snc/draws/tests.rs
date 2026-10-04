use super::*;
fn fixture() -> Vec<u8> {
    let mut b = vec![0; 600];
    b[..4].copy_from_slice(b"NUIF");
    b[32..36].copy_from_slice(b"nCSC");
    for (at, value) in [
        (12, 32u32),
        (76, 3),
        (84, 1),
        (88, 96),
        (92, 1),
        (96, 128),
        (128, 1),
        (132, 112),
        (144, 224),
        (160, 480),
        (256, 1),
        (260, 1),
        (264, 1),
        (304, 432),
        (316, 32),
        (320, 304),
    ] {
        b[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    b[336..464].fill(255);
    b[336..340].copy_from_slice(&2u32.to_le_bytes());
    b[512..520].copy_from_slice(b"glyph_a\0");
    b
}
#[test]
fn names_resolve_through_group_pointers_to_sparse_cell_slots() {
    let r = named_draws(&fixture()).unwrap();
    assert_eq!(r["draws"][0]["name"], "glyph_a");
    assert_eq!(r["draws"][0]["record_offset"], 256);
    assert_eq!(r["draws"][0]["cells"], json!([{"slot":0,"cell":2}]));
    assert_eq!(r["draws"][0]["transform_offset"], 464);
}
#[test]
fn broken_pointers_cell_indices_and_nonfinite_geometry_are_rejected() {
    for (at, value) in [
        (144, u32::MAX),
        (336, 3),
        (268, f32::NAN.to_bits()),
        (168, 1),
    ] {
        let mut b = fixture();
        b[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(named_draws(&b).is_err());
    }
}

#[test]
fn geometry_edit_preserves_cell_references_and_unrelated_transform_words() {
    let mut b = fixture();
    for (i, v) in quad([0.1, 0.2]).unwrap().iter().enumerate() {
        b[268 + i * 4..272 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    let edit = || DrawEdit {
        name: "glyph_a".into(),
        expected_cells: vec![2],
        expected_half_size: [0.1, 0.2],
        half_size: [0.2, 0.3],
        expected_center_x: 0.0,
        center_x: 0.25,
    };
    let (after, _) = edit_draws(&b, &[edit()]).unwrap();
    for (i, (x, y)) in b.iter().zip(&after).enumerate() {
        if !(268..300).contains(&i) && !(468..472).contains(&i) {
            assert_eq!(x, y);
        }
    }
    assert_eq!(floats(&after, 468, 1).unwrap(), vec![0.25]);
    assert_eq!(
        named_draws(&after).unwrap()["draws"][0]["cells"],
        json!([{"slot":0,"cell":2}])
    );
    assert!(edit_draws(&b, &[edit(), edit()]).is_err());
    let mut wrong = edit();
    wrong.expected_cells = vec![1];
    assert!(edit_draws(&b, &[wrong]).is_err());
    let mut wrong = edit();
    wrong.expected_half_size = [0.15, 0.2];
    assert!(edit_draws(&b, &[wrong]).is_err());
}

#[test]
fn remapping_a_reused_glyph_keeps_animation_and_geometry_bytes() {
    let b = fixture();
    let edit = || CellRemap {
        name: "glyph_a".into(),
        selector_slot: 0,
        expected_cell: 2,
        cell: 1,
        local_x: None,
    };
    let (after, _) = remap_cells(&b, &[edit()]).unwrap();
    assert_eq!(&after[..336], &b[..336]);
    assert_eq!(&after[340..], &b[340..]);
    assert_eq!(
        named_draws(&after).unwrap()["draws"][0]["cells"],
        json!([{"slot":0,"cell":1}])
    );
    assert!(remap_cells(&b, &[edit(), edit()]).is_err());
    let mut wrong = edit();
    wrong.expected_cell = 0;
    assert!(remap_cells(&b, &[wrong]).is_err());
    let mut wrong = edit();
    wrong.cell = 3;
    assert!(remap_cells(&b, &[wrong]).is_err());
    let mut wrong = edit();
    wrong.selector_slot = 1;
    assert!(remap_cells(&b, &[wrong]).is_err());
}

#[test]
fn positioned_remap_changes_only_the_selector_and_verified_local_x() {
    let b = fixture();
    let edit = || CellRemap {
        name: "glyph_a".into(),
        selector_slot: 0,
        expected_cell: 2,
        cell: 1,
        local_x: Some(DrawX {
            expected: 0.0,
            value: 0.125,
        }),
    };
    let (after, _) = remap_cells(&b, &[edit()]).unwrap();
    for (i, (a, z)) in b.iter().zip(&after).enumerate() {
        if !(336..340).contains(&i) && !(468..472).contains(&i) {
            assert_eq!(a, z);
        }
    }
    assert_eq!(floats(&after, 468, 1).unwrap(), [0.125]);
    let mut wrong = edit();
    wrong.local_x.as_mut().unwrap().expected = 0.25;
    assert!(remap_cells(&b, &[wrong]).is_err());
    for invalid in [f32::NAN, f32::INFINITY, 1.1] {
        let mut wrong = edit();
        wrong.local_x.as_mut().unwrap().value = invalid;
        assert!(remap_cells(&b, &[wrong]).is_err());
    }
    let mut alias = edit();
    alias.selector_slot = 1;
    let mut shared = b;
    shared[340..344].copy_from_slice(&2u32.to_le_bytes());
    assert!(remap_cells(&shared, &[edit(), alias]).is_err());
}

#[test]
fn names_resolve_repeated_local_ids_in_distinct_groups() {
    let mut b = fixture();
    b.resize(1100, 0);
    for (at, value) in [
        (84, 2u32),
        (88, 568),
        (92, 2),
        (96, 608),
        (600, 1),
        (604, 112),
        (616, 1),
        (620, 600),
        (632, 768),
        (640, 480),
        (644, 0),
        (648, 0),
        (652, 668),
        (656, 1),
        (660, 0),
        (800, 1),
        (804, 1),
        (808, 1),
        (848, 1000),
        (860, 32),
        (864, 868),
    ] {
        b[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    b[700..708].copy_from_slice(b"glyph_b\0");
    b[900..1028].fill(255);
    b[900..904].copy_from_slice(&1u32.to_le_bytes());
    let r = named_draws(&b).unwrap();
    assert_eq!(r["draws"][0]["group"], 0);
    assert_eq!(r["draws"][1]["group"], 1);
    assert_eq!(r["draws"][1]["cells"], json!([{"slot":0,"cell":1}]));
    b[656..660].copy_from_slice(&2u32.to_le_bytes());
    assert!(named_draws(&b).is_err());
}
