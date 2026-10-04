use super::*;
#[test]
fn paired_relocation_enforces_source_alignment_and_rounded_read_boundaries() {
    let mut b = vec![0; 176];
    b[..16].copy_from_slice(b"oo_disk_image___");
    for (at, value) in [
        (0x54, 2u32),
        (116, 2),
        (120, 24),
        (128, 7),
        (132, 0),
        (136, 2048),
        (144, u32::MAX),
        (152, 8),
        (156, 2048),
        (160, 100),
        (168, u32::MAX),
    ] {
        b[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut changes = vec![
        Relocation {
            record_offset: 128,
            hash_path: vec![7],
            before_offset: 0,
            before_size: 2048,
            offset: 0,
            size: 2049,
        },
        Relocation {
            record_offset: 152,
            hash_path: vec![8],
            before_offset: 2048,
            before_size: 100,
            offset: 4096,
            size: 100,
        },
    ];
    assert!(relocate(&b, &changes[..1], 8192).is_err());
    let out = relocate(&b, &changes, 8192).unwrap();
    assert_eq!(word(&out, 156).unwrap(), 4096);
    assert_eq!(&out[164..], &b[164..]);
    changes[1].offset = 4095;
    assert!(relocate(&b, &changes, 8192).is_err());
    changes[1].offset = 4096;
    assert!(relocate(&b, &changes, 4196).is_err());
    changes[1].before_size = 99;
    assert!(relocate(&b, &changes, 8192).is_err());
}
#[test]
fn duplicate_paths_keep_order_and_invalid_layouts_fail() {
    let mut b = vec![0; 176];
    b[..16].copy_from_slice(b"oo_disk_image___");
    let set = |b: &mut Vec<u8>, at, value: u32| b[at..at + 4].copy_from_slice(&value.to_le_bytes());
    set(&mut b, 0x54, 2);
    set(&mut b, 116, 2);
    set(&mut b, 120, 24);
    set(&mut b, 128, 7);
    set(&mut b, 152, 7);
    set(&mut b, 132, 100);
    set(&mut b, 156, 200);
    let parsed = Index::parse(&b).unwrap();
    assert_eq!(parsed.records[0].hash_path, parsed.records[1].hash_path);
    assert_eq!(parsed.records[0].offset, 100);
    assert_eq!(parsed.records[1].offset, 200);
    assert!(Index::parse(&b[..175]).is_err());
    set(&mut b, 120, 0); // file table would overwrite the directory node.
    assert!(Index::parse(&b).is_err());
    set(&mut b, 120, 24);
    set(&mut b, 0x54, 1);
    assert!(Index::parse(&b).is_err());
}
