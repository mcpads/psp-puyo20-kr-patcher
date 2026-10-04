use super::*;
#[test]
fn explicit_exclusions_pin_source_and_leave_record_numbers_intact() {
    let mtx = text::Mtx {
        header: vec![],
        group_first_records: vec![0, 3],
        records: [
            vec![0, 0xf813, 0xffff],
            vec![1, 0xffff],
            vec![0xffff],
            vec![0xffff],
        ]
        .into_iter()
        .map(|units| text::Record { offset: 0, units })
        .collect(),
    };
    let original = selected_records(&mtx, &BTreeMap::from([(0, 3)])).unwrap();
    let entry = json!({"group":0,"record":1,"reason":"source authoring note",
        "source_units_sha256":sha256(&[1, 0, 255, 255])});
    let exclusions: Vec<ExcludedRecord> = serde_json::from_value(json!([entry.clone()])).unwrap();
    let mut selected = original.clone();
    let receipts = exclude_records(&mtx, &mut selected, &exclusions).unwrap();
    assert_eq!(selected, [(0, 0, 0), (0, 2, 2)]);
    assert_eq!(receipts[0]["flat_record"], 1);
    assert_eq!(mtx.records[1].units, [1, 0xffff]);
    for (key, value) in [
        ("group", json!(1)),
        ("record", json!(3)),
        ("source_units_sha256", json!("changed")),
        ("reason", json!(" ")),
    ] {
        let mut bad = entry.clone();
        bad[key] = value;
        let bad: Vec<ExcludedRecord> = serde_json::from_value(json!([bad])).unwrap();
        assert!(exclude_records(&mtx, &mut original.clone(), &bad).is_err());
    }
    let duplicates: Vec<ExcludedRecord> =
        serde_json::from_value(json!([entry.clone(), entry])).unwrap();
    assert!(exclude_records(&mtx, &mut original.clone(), &duplicates).is_err());
    assert!(control_units(&[json!({"control":0xffff,"operands":[]})]).is_err());
}
#[test]
fn source_attribute_index_survives_translation_slot_changes() {
    let tokens = vec![
        json!({"control":0xf800,"operands":[2]}),
        json!({"character":"あ","glyph_slot":2}),
        json!({"control":0xf813,"operands":[]}),
        json!({"control":0xffff,"operands":[]}),
    ];
    let controls = control_units(&tokens).unwrap();
    let (units, prose, widths) = encode_segments(
        &["".into(), "가".into(), "".into()],
        &controls,
        &BTreeMap::from([('가', (17, 13))]),
        252,
        3,
    )
    .unwrap();
    assert_eq!(units, [0xf800, 2, 17, 0xf813, 0xffff]);
    assert_eq!(prose, "가");
    assert_eq!(widths, [14]);
}
#[test]
fn source_newline_after_end_wait_is_retained() {
    let tokens = vec![
        json!({"control":0xf813,"operands":[]}),
        json!({"control":0xfffd,"operands":[]}),
        json!({"control":0xffff,"operands":[]}),
    ];
    let controls = control_units(&tokens).unwrap();
    let (units, prose, widths) = encode_segments(
        &["응?".into(), "".into(), "".into()],
        &controls,
        &BTreeMap::from([('응', (0, 13)), ('?', (1, 13))]),
        252,
        3,
    )
    .unwrap();
    assert_eq!(units, [0, 1, 0xf813, 0xfffd, 0xffff]);
    assert_eq!(prose, "응?\n");
    assert_eq!(widths, [28, 0]);
    assert!(control_units(&tokens[1..]).is_err());
}
#[test]
fn untranslated_slots_remap_without_touching_control_operands() {
    let glyphs = BTreeMap::from([('…', (2, 13))]);
    let tokens = vec![
        json!({"character":"あ","glyph_slot":9,"unit_offset":0}),
        json!({"control":0xf881,"operands":[9],"unit_offset":1}),
        json!({"character":"…","glyph_slot":4,"unit_offset":3}),
    ];
    let source = vec![9, 0xf881, 9, 4, 0xf813, 0xffff];
    let (result, missing) = remap_untranslated(&source, &tokens, &glyphs, 0).unwrap();
    assert_eq!(result, [0, 0xf881, 9, 2, 0xf813, 0xffff]);
    assert_eq!(missing, 1);
    let mut bad = tokens;
    bad[0]["unit_offset"] = json!(1);
    assert!(remap_untranslated(&source, &bad, &glyphs, 0).is_err());
}
#[test]
fn selects_multiple_groups_without_shifting_unselected_record_numbers() {
    let mtx = text::Mtx {
        header: Vec::new(),
        group_first_records: vec![0, 2, 3],
        records: (0..5)
            .map(|_| text::Record {
                offset: 0,
                units: vec![0xffff],
            })
            .collect(),
    };
    assert_eq!(
        selected_records(&mtx, &BTreeMap::from([(0, 2), (2, 2)])).unwrap(),
        [(0, 0, 0), (0, 1, 1), (2, 0, 3), (2, 1, 4)]
    );
    assert!(selected_records(&mtx, &BTreeMap::from([(1, 2)])).is_err());
    assert!(selected_records(&mtx, &BTreeMap::from([(3, 1)])).is_err());
    assert!(selected_records(&mtx, &BTreeMap::new()).is_err());
}
#[test]
fn explicit_control_spans_reject_missing_glyphs_layout_and_extra_prose() {
    let glyphs = BTreeMap::from([('가', (3, 13)), ('나', (4, 13))]);
    let controls = vec![vec![0xf881, 2], vec![0xfffd], vec![0xf813]];
    let spans = vec!["가".into(), "".into(), "나".into(), "".into()];
    let (u, p, w) = encode_segments(&spans, &controls, &glyphs, 28, 2).unwrap();
    assert_eq!(u, [3, 0xf881, 2, 0xfffd, 4, 0xf813, 0xffff]);
    assert_eq!(p, "가\n나");
    assert_eq!(w, [14, 14]);
    assert!(encode_segments(&spans, &controls, &glyphs, 13, 2).is_err());
    assert!(encode_segments(&spans, &controls, &glyphs, 28, 1).is_err());
    assert!(encode_segments(&spans[..3], &controls, &glyphs, 28, 2).is_err());
    let mut bad = spans.clone();
    bad[0] = "다".into();
    assert!(encode_segments(&bad, &controls, &glyphs, 28, 2).is_err());
    bad = spans.clone();
    bad[3] = "가".into();
    assert!(encode_segments(&bad, &controls, &glyphs, 28, 2).is_err());
    bad = spans;
    bad[0] = "가\n".into();
    assert!(encode_segments(&bad, &controls, &glyphs, 28, 2).is_err());
}
