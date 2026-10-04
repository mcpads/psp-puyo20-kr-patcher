use super::*;

#[test]
fn segmented_page_reset_restores_width_but_wait_does_not() {
    let source = json!({"units":[0,63507,63506,0,65535],
        "text":"a<F813><F812>a<FFFF>",
        "tokens":[{"glyph_slot":0},{"control":63507,"operands":[]},
            {"control":63506,"operands":[]},{"glyph_slot":0},
            {"control":65535,"operands":[]}]});
    let entry = json!({"group":0,"record":0,"source_units":source["units"],
        "source_text":source["text"],"segments":["가","","가",""],
        "expected_text":"가<F813><F812>가<FFFF>"});
    let map = BTreeMap::from([('가', 4)]);
    let allowed = [0xf813, 0xf812, 0xffff];
    assert_eq!(
        segments::encode(&entry, &source, 0, 0, &map, &allowed, 1).unwrap(),
        [4, 0xf813, 0xf812, 4, 0xffff]
    );
    let mut overflowing = entry;
    overflowing["segments"] = json!(["가", "가", "", ""]);
    overflowing["expected_text"] = json!("가<F813>가<F812><FFFF>");
    assert!(segments::encode(&overflowing, &source, 0, 0, &map, &allowed, 1).is_err());
}

#[test]
fn segmented_translation_keeps_attribute_operands_and_termination_padding() {
    let source = json!({"units":[0,63488,1,1,63489,65535,65535],
        "text":"a<F800:0001>b<F801><FFFF><FFFF>",
        "tokens":[{"glyph_slot":0},{"control":63488,"operands":[1]},
            {"glyph_slot":1},{"control":63489,"operands":[]},
            {"control":65535,"operands":[]},{"control":65535,"operands":[]}]});
    let mut entry = json!({"group":0,"record":0,"source_units":source["units"],
        "source_text":source["text"],"segments":["가","나","","",""],
        "expected_text":"가<F800:0001>나<F801><FFFF><FFFF>"});
    let map = BTreeMap::from([('가', 4), ('나', 5)]);
    let allowed = [0xf800, 0xf801, 0xffff];
    let units = segments::encode(&entry, &source, 0, 0, &map, &allowed, 2).unwrap();
    assert_eq!(units, [4, 0xf800, 1, 5, 0xf801, 0xffff, 0xffff]);
    assert!(segments::encode(&entry, &source, 0, 0, &map, &allowed, 1).is_err());
    assert!(segments::encode(&entry, &source, 0, 0, &map, &[0xffff], 2).is_err());
    let mut output = source.clone();
    output["text"] = json!("가<F800:0001>나<F801><FFFF><FFFF><FFFF>");
    output["tokens"]
        .as_array_mut()
        .unwrap()
        .push(json!({"control":65535,"operands":[]}));
    segments::verify(&entry, &source, &output).unwrap();
    output["tokens"][1]["operands"] = json!([4]);
    assert!(segments::verify(&entry, &source, &output).is_err());
    entry["segments"][4] = json!("가");
    assert!(segments::encode(&entry, &source, 0, 0, &map, &allowed, 2).is_err());
}

#[test]
fn joint_growth_preserves_neighbors_and_rejects_unaccounted_bytes() {
    let mut source = vec![0; 8192];
    source[..3].copy_from_slice(b"fnt");
    source[2048..2051].copy_from_slice(b"mtx");
    source[4096..4100].copy_from_slice(b"enum");
    let font = vec![9; 3000];
    let pieces = [
        (0, 0, b"fnt".as_slice(), font.as_slice()),
        (2048, 4096, b"mtx".as_slice(), b"new".as_slice()),
        (4096, 6144, b"enum".as_slice(), b"enum".as_slice()),
    ];
    let out = extent::place_joint(&source, &pieces).unwrap();
    assert_eq!(&out[6144..6148], b"enum");
    assert_eq!(&out[4096..4099], b"new");
    assert!(extent::place_joint(&source, &pieces[..2]).is_err());
    let mut overlap = pieces;
    overlap[2].1 = 4096;
    assert!(extent::place_joint(&source, &overlap).is_err());
    overlap[2].1 = 8192;
    assert!(extent::place_joint(&source, &overlap).is_err());
    source[8000] = 1;
    assert!(extent::place_joint(&source, &pieces).is_err());
}

#[test]
fn indexed_pool_borrows_capacity_without_moving_table_boundary() {
    let mut source = vec![0xff; 40];
    for (i, value) in [40u32, 8, 12, 20, 24].iter().enumerate() {
        source[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    let records = vec![vec![1, 2, 0xffff], vec![3, 0xfffd, 0xffff]];
    assert!(place_records(&source, &records, false).is_err());
    let (output, _) = place_records(&source, &records, true).unwrap();
    assert_eq!(word(&output, 12).unwrap(), 20);
    assert_eq!(word(&output, 16).unwrap(), 26);
    assert_eq!(half(&output, 28).unwrap(), 0xfffd);
    assert_eq!(half(&output, 30).unwrap(), 0xffff);
    assert!(output[32..].iter().all(|b| *b == 0xff));
    assert_eq!(&output[..12], &source[..12]);
    assert!(place_records(&source, &[vec![1; 11], vec![0xffff]], true).is_err());
    assert!(
        place_records(
            &source,
            &[
                vec![1; 10].into_iter().chain([0xffff]).collect(),
                vec![0xffff]
            ],
            true
        )
        .is_err()
    );
}

#[test]
fn omitted_tail_must_be_exactly_empty_and_stay_at_original_addresses() {
    let mut source = Vec::new();
    for value in [30u32, 8, 12, 24, 26, 28] {
        source.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0xffffu16, 0xffff, 0xffff] {
        source.extend_from_slice(&value.to_le_bytes());
    }
    let entries = [json!({"id":0,"source":"x<FFFF>","lines":["가"]})];
    let completed = complete_entries(&source, &entries, 3, 2, false).unwrap();
    assert_eq!(completed.len(), 3);
    let (output, _) =
        place_records(&source, &[vec![0xffff], vec![0xffff], vec![0xffff]], false).unwrap();
    assert_eq!(&output[..24], &source[..24]);
    assert_eq!(&output[26..], &source[26..]);
    assert!(complete_entries(&source, &entries, 3, 0, false).is_err());
    assert!(complete_entries(&source, &entries, 3, 2, true).is_err());
    source[28] = 0;
    assert!(complete_entries(&source, &entries, 3, 2, false).is_err());
}

#[test]
fn grouped_pool_preserves_selection_and_requires_matching_declarations() {
    let mut source = Vec::new();
    for value in [44u32, 8, 16, 24, 28, 32, 38] {
        source.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0u16, 0xffff, 1, 2, 0xffff, 3, 4, 0xffff] {
        source.extend_from_slice(&value.to_le_bytes());
    }
    let parsed = Mtx::parse(&source).unwrap();
    assert_eq!(parsed.group_first_records, [0, 2]);
    assert!(validate_groups(&parsed, None).is_err());
    assert!(validate_groups(&parsed, Some(&json!([0, 1]))).is_err());
    validate_groups(&parsed, Some(&json!([0, 2]))).unwrap();
    let records = vec![vec![5, 6, 0xffff], vec![7, 0xffff], vec![8, 0xffff]];
    assert!(place_records(&source, &records, false).is_err());
    let (output, _) = place_records(&source, &records, true).unwrap();
    let output = Mtx::parse(&output).unwrap();
    assert_eq!(output.group_first_records, [0, 2]);
    assert_eq!(&output.header[..16], &source[..16]);
    assert_eq!(output.records[0].offset, 28);
    assert_eq!(output.records[1].units, records[1]);
    assert_eq!(output.records[2].offset, 38);
    assert_eq!(output.records[2].units, [8, 0xffff, 0xffff]);
    assert!(place_records(&source, &records[..2], true).is_err());
}
