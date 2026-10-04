use super::*;
fn fixture() -> Vec<u8> {
    // Two groups, three records; includes a control with a small operand.
    let mut b = [40u32, 8, 16, 24, 28, 32, 38]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect::<Vec<_>>();
    for u in [1u16, 0xffff, 0xf800, 2, 0xffff, 0xffff] {
        b.extend_from_slice(&u.to_le_bytes());
    }
    b
}
#[test]
fn grouped_repack_keeps_untranslated_units_and_rejects_capacity_errors() {
    let m = Mtx::parse(&fixture()).unwrap();
    let replacements = BTreeMap::from([(0, vec![3, 4, 0xffff]), (1, vec![0xffff])]);
    let out = m.replace_records(&replacements).unwrap();
    let p = Mtx::parse(&out).unwrap();
    assert_eq!(p.group_first_records, [0, 2]);
    assert_eq!(p.header.len(), m.header.len());
    assert_eq!(p.records[0].units, [3, 4, 0xffff]);
    assert_eq!(p.records[1].units, [0xffff, 0xffff]);
    assert_eq!(p.records[2].units, m.records[2].units);
    assert!(
        m.replace_records(&BTreeMap::from([(0, vec![3, 4, 0xffff])]))
            .is_err()
    );
    assert!(
        m.replace_records(&BTreeMap::from([(3, vec![0xffff])]))
            .is_err()
    );
    assert!(m.replace_records(&BTreeMap::from([(0, vec![3])])).is_err());
}
#[test]
fn grouped_records_and_unknown_operands_survive_json() {
    let b = fixture();
    let m = Mtx::parse(&b).unwrap();
    assert_eq!(m.group_first_records, [0, 2]);
    assert_eq!(m.records[1].units, [0xf800, 2, 0xffff]);
    let readback: Mtx = serde_json::from_slice(&serde_json::to_vec(&m).unwrap()).unwrap();
    assert_eq!(readback.to_bytes().unwrap(), b);
}
#[test]
fn control_operand_is_not_a_glyph_and_unknowns_stay_unresolved() {
    let controls: ControlTable = serde_json::from_str(CONTROL_SPEC).unwrap();
    let glyphs = [[65, 13], [66, 13]];
    let (text, tokens) =
        decode_record(&[0xf800, 1, 0, 0xffff], &glyphs, &controls.entries).unwrap();
    assert_eq!(text, "<F800:0001>A<FFFF>");
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens[0]["operands"], json!([1]));
    for units in [
        vec![0xf899, 1, 0xffff],
        vec![0xf800],
        vec![0xf800, 0xffff],
        vec![0xffff, 0],
    ] {
        assert!(decode_record(&units, &glyphs, &controls.entries).is_none());
    }
}
#[test]
fn invalid_group_pointer_record_overlap_and_missing_terminal_fail() {
    for (p, val) in [(12, 18u32), (20, 28), (24, 100)] {
        let mut b = fixture();
        b[p..p + 4].copy_from_slice(&val.to_le_bytes());
        assert!(Mtx::parse(&b).is_err());
    }
    let mut b = fixture();
    b[38..40].copy_from_slice(&1u16.to_le_bytes());
    assert!(Mtx::parse(&b).is_err());
    let mut m = Mtx::parse(&fixture()).unwrap();
    m.group_first_records[1] = 1;
    assert!(m.to_bytes().is_err());
}
