use super::{elf::Program, font_copy::font_copy_instruction, *};
#[test]
fn font_copy_patch_preserves_registers_and_rejects_wrong_original() {
    let (after, _) = font_copy_instruction(&0x02258821u32.to_le_bytes(), 0x0889661c).unwrap();
    assert_eq!(u32::from_le_bytes(after), 0x02258823);
    assert!(font_copy_instruction(&0x02258823u32.to_le_bytes(), 0x0889661c).is_err());
    assert!(font_copy_instruction(&0x02058821u32.to_le_bytes(), 0x0889661c).is_err());
}
#[test]
fn mapping_rejects_bss_and_ambiguous_segments() {
    let mut p = vec![Program {
        kind: 1,
        offset: 0xc0,
        address: 0,
        file_size: 0x100,
        memory_size: 0x200,
        flags: 7,
        alignment: 64,
    }];
    assert_eq!(map_address(&p, 0x08804000, 0x08804020, 4).unwrap(), 0xe0);
    assert!(map_address(&p, 0x08804000, 0x08804100, 4).is_err());
    assert!(map_address(&p, 0x08804000, 0x088040fc, 8).is_err());
    p.push(Program {
        kind: 1,
        offset: 0x200,
        address: 0x20,
        file_size: 16,
        memory_size: 16,
        flags: 7,
        alignment: 4,
    });
    assert!(map_address(&p, 0x08804000, 0x08804020, 4).is_err());
    assert!(programs(b"\x7fELF").is_err());
}
