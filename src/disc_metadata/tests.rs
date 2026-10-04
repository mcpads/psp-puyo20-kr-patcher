use super::replace_utf8;

/// Minimal SFO with TITLE (UTF-8, 128-byte slot) followed by a second key.
fn sfo(title: &str) -> Vec<u8> {
    let keys = b"TITLE\0XKEY\0\0\0";
    let (key_table, data_table) = (20 + 32, 20 + 32 + keys.len());
    let mut out = b"\0PSF\x01\x01\0\0".to_vec();
    for v in [key_table, data_table, 2] {
        out.extend((v as u32).to_le_bytes());
    }
    let mut value = title.as_bytes().to_vec();
    value.push(0);
    for (key_at, len, max, data) in [(0u16, value.len(), 128, 0), (6, 4, 4, 128)] {
        out.extend(key_at.to_le_bytes());
        out.extend(0x0204u16.to_le_bytes());
        for v in [len, max, data] {
            out.extend((v as u32).to_le_bytes());
        }
    }
    out.extend(keys);
    let mut slot = vec![0; 128];
    slot[..value.len()].copy_from_slice(&value);
    out.extend(slot);
    out.extend(b"abc\0");
    out
}

#[test]
fn replaces_title_inside_its_slot_only() {
    let before = sfo("ぷよぷよ！！");
    let after = replace_utf8(&before, "TITLE", "ぷよぷよ！！", "뿌요뿌요!!").unwrap();
    assert_eq!(after, sfo("뿌요뿌요!!"));
    assert_eq!(after.len(), before.len());
}

#[test]
fn rejects_unexpected_source_and_oversized_value() {
    let before = sfo("ぷよぷよ！！");
    assert!(replace_utf8(&before, "TITLE", "other", "뿌요").is_err());
    assert!(replace_utf8(&before, "TITLE", "ぷよぷよ！！", &"가".repeat(43)).is_err());
}
