use super::*;
use serde_json::json;

#[test]
fn selected_symbol_keeps_its_source_identity_and_rejects_drift() {
    let mut fnt = vec![0; 16];
    fnt[12..16].copy_from_slice(&2u32.to_le_bytes());
    for (c, width) in [('あ', 13u16), ('★', 11u16)] {
        fnt.extend_from_slice(&(c as u16).to_le_bytes());
        fnt.extend_from_slice(&width.to_le_bytes());
    }
    let entry = json!({"source_slot":1,"character":"★","width":11,"reason":"source symbol"});
    let config = json!({"preserved_glyphs":[entry.clone()]});
    assert_eq!(
        selected_source_glyphs(&config, &fnt, true).unwrap(),
        vec![1]
    );
    assert!(selected_source_glyphs(&config, &fnt, false).is_err());
    for (key, value) in [
        ("source_slot", json!(2)),
        ("character", json!("☆")),
        ("width", json!(13)),
    ] {
        let mut changed = config.clone();
        changed["preserved_glyphs"][0][key] = value;
        assert!(selected_source_glyphs(&changed, &fnt, true).is_err());
    }
    assert!(
        selected_source_glyphs(
            &json!({"preserved_glyphs":[entry.clone(),entry]}),
            &fnt,
            true
        )
        .is_err()
    );
    assert!(
        selected_source_glyphs(&json!({}), &fnt, true)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        selected_source_glyphs(&json!({}), &fnt, false).unwrap(),
        vec![0, 1]
    );
}

#[test]
fn relocated_symbol_preserves_all_pixels_without_changing_adjacent_cells() {
    let mut source = vec![0; 512 * 28 * 4];
    for y in 14..28 {
        for x in 0..14 {
            let at = (y * 512 + x) * 4;
            source[at..at + 4].copy_from_slice(&[x as u8, y as u8, 90, 255]);
        }
    }
    let mut output = vec![7; 512 * 14 * 4];
    copy_source_glyph(&source, 36, &mut output, 1).unwrap();
    for y in 0..14 {
        for x in 0..512 {
            let at = (y * 512 + x) * 4;
            let expected = if (14..28).contains(&x) {
                [(x - 14) as u8, (y + 14) as u8, 90, 255]
            } else {
                [7; 4]
            };
            assert_eq!(&output[at..at + 4], expected);
        }
    }
    assert!(copy_source_glyph(&source, 72, &mut output, 0).is_err());
}
