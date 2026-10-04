use super::*;
fn block(kind: u32, payload: Vec<u8>) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [
        kind,
        payload.len() as u32 + 16,
        payload.len() as u32 + 16,
        16,
    ] {
        b.extend(v.to_le_bytes());
    }
    b.extend(payload);
    b
}
fn pl(fmt: u16, order: u16, w: u16, h: u16, bits: u16, pixels: Vec<u8>) -> Vec<u8> {
    let mut b = vec![0; 64];
    for (i, v) in [fmt, order, w, h, bits].iter().enumerate() {
        b[4 + i * 2..6 + i * 2].copy_from_slice(&v.to_le_bytes());
    }
    b[28..32].copy_from_slice(&64u32.to_le_bytes());
    b[32..36].copy_from_slice(&(64u32 + pixels.len() as u32).to_le_bytes());
    for (i, v) in [1u16, 3, 1].iter().enumerate() {
        b[42 + i * 2..44 + i * 2].copy_from_slice(&v.to_le_bytes());
    }
    b.extend(pixels);
    b
}
fn fixture(bits: usize, order: usize) -> Vec<u8> {
    let rows: Vec<Vec<u8>> = (0..8)
        .map(|y| {
            let r: Vec<u8> = (0..32).map(|x| ((x + y) % 16) as u8).collect();
            if bits == 4 {
                r.chunks(2).map(|v| v[0] | v[1] << 4).collect()
            } else {
                r
            }
        })
        .collect();
    let mut raw = Vec::new();
    if order == 0 {
        for r in &rows {
            raw.extend(r);
        }
    } else {
        for x in (0..rows[0].len()).step_by(16) {
            for r in &rows {
                raw.extend(&r[x..x + 16]);
            }
        }
    }
    let pal = (0u8..16).flat_map(|i| [i, i * 2, i * 3, 255]).collect();
    let mut picture = block(5, pl(3, 0, 16, 1, 32, pal));
    picture.extend(block(
        4,
        pl(
            if bits == 4 { 4 } else { 5 },
            order as u16,
            32,
            8,
            bits as u16,
            raw,
        ),
    ));
    let mut b = b"MIG.00.1PSP\0\0\0\0\0".to_vec();
    b.extend(block(2, block(3, picture)));
    b
}
#[test]
fn colors_and_exact_roundtrip() {
    for bits in [4, 8] {
        for order in [0, 1] {
            let b = fixture(bits, order);
            let mut im = decode(&b).unwrap();
            for y in 0..8 {
                for x in 0..32 {
                    let i = ((x + y) % 16) as u8;
                    assert_eq!(
                        &im.rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4],
                        &[i, i * 2, i * 3, 255]
                    );
                }
            }
            assert_eq!(encode(&b, 32, 8, &im.rgba).unwrap(), b);
            let i = (5 * 32 + 17) * 4;
            im.rgba[i..i + 4].copy_from_slice(&[7, 14, 21, 255]);
            let out = encode(&b, 32, 8, &im.rgba).unwrap();
            assert_eq!(out.iter().zip(&b).filter(|(a, b)| a != b).count(), 1);
            assert_eq!(decode(&out).unwrap().rgba, im.rgba);
        }
    }
}
#[test]
fn info_compaction_preserves_picture_and_rejects_insufficient_space() {
    let mut b = fixture(8, 0);
    let picture_end = b.len();
    b.extend(block(0xff, vec![0; 72]));
    let root_size = (b.len() - 16) as u32;
    b[20..24].copy_from_slice(&root_size.to_le_bytes());
    b[24..28].copy_from_slice(&16u32.to_le_bytes());
    b[40..44].copy_from_slice(&16u32.to_le_bytes());
    let compact = compact_file_info(&b, 48).unwrap();
    assert_eq!(compact.len(), b.len() - 48);
    assert_eq!(compact[32..picture_end], b[32..picture_end]);
    assert_eq!(decode(&compact).unwrap().rgba, decode(&b).unwrap().rgba);
    assert!(compact_file_info(&b, 64).is_err());
    assert!(compact_file_info(&b, 3).is_err());
    assert!(compact_file_info(&fixture(8, 0), 48).is_err());
}
#[test]
fn font_growth_preserves_source_rows_and_rejects_unknown_frame_layout() {
    let mut pixel_plane = pl(
        5,
        0,
        512,
        64,
        8,
        (0..512 * 64).map(|i| (i % 256) as u8).collect(),
    );
    for (at, value) in [(0, 48u32), (24, 48), (48, 64)] {
        pixel_plane[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut picture = block(
        5,
        pl(
            3,
            0,
            256,
            1,
            32,
            (0..=255).flat_map(|a| [255, 255, 255, a]).collect(),
        ),
    );
    let image_at = 48 + picture.len();
    picture.extend(block(4, pixel_plane));
    let mut root = block(3, picture);
    root[8..12].copy_from_slice(&16u32.to_le_bytes());
    root.extend(block(0xff, vec![0; 60]));
    let mut b = b"MIG.00.1PSP\0\0\0\0\0".to_vec();
    b.extend(block(2, root));
    b[24..28].copy_from_slice(&16u32.to_le_bytes());
    let old = decode(&b).unwrap();
    let grown = resize_font_atlas(&b, 128).unwrap();
    let new = decode(&grown).unwrap();
    assert_eq!(new.height, 128);
    assert_eq!(&new.rgba[..old.rgba.len()], old.rgba);
    assert!(
        new.rgba[old.rgba.len()..]
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 255, 255, 0])
    );
    assert_eq!(grown.len(), b.len() + 32768);
    assert_eq!(resize_font_atlas(&grown, 64).unwrap(), b);
    assert!(resize_font_atlas(&b, 96).is_err());
    b[image_at + 16 + 48..image_at + 16 + 52].copy_from_slice(&68u32.to_le_bytes());
    assert!(resize_font_atlas(&b, 128).is_err());
}
#[test]
fn malformed_and_unmapped_fail() {
    let b = fixture(8, 1);
    assert!(decode(&b[..b.len() - 1]).is_err());
    let mut im = decode(&b).unwrap();
    im.rgba[..4].copy_from_slice(&[255, 0, 0, 255]);
    assert!(encode(&b, 32, 8, &im.rgba).is_err());
    assert!(snt_table(b"NUIF").is_err());
}

#[test]
fn sixteen_bit_palettes_preserve_channels_alpha_and_index_writes() {
    for (format, value, expected) in [
        (1, 0x801fu16, [255, 0, 0, 255]),
        (1, 0x03e0, [0, 255, 0, 0]),
        (2, 0xf12e, [238, 34, 17, 255]),
    ] {
        let palette: Vec<u8> = (0..16)
            .flat_map(|i| if i == 0 { 0u16 } else { value }.to_le_bytes())
            .collect();
        let mut picture = block(5, pl(format, 0, 16, 1, 16, palette));
        picture.extend(block(4, pl(5, 0, 16, 1, 8, vec![1; 16])));
        let mut b = b"MIG.00.1PSP\0\0\0\0\0".to_vec();
        b.extend(block(2, block(3, picture)));
        let mut im = decode(&b).unwrap();
        assert_eq!(&im.rgba[..4], &expected);
        assert_eq!(encode(&b, 16, 1, &im.rgba).unwrap(), b);
        im.rgba[..4].fill(0);
        let out = encode(&b, 16, 1, &im.rgba).unwrap();
        assert_eq!(out.iter().zip(&b).filter(|(a, b)| a != b).count(), 1);
        assert_eq!(decode(&out).unwrap().rgba, im.rgba);
    }
}

#[test]
fn psp_dxt5_block_order_alpha_and_read_only_boundary() {
    let mut tile = [0u8; 16];
    tile[..4].fill(0xe4);
    tile[4..6].copy_from_slice(&0xf800u16.to_le_bytes());
    tile[6..8].copy_from_slice(&0x001fu16.to_le_bytes());
    let selectors: u64 = (0..16).map(|i| ((i % 8) as u64) << (i * 3)).sum();
    tile[8..14].copy_from_slice(&selectors.to_le_bytes()[..6]);
    tile[14] = 255;
    let mut b = b"MIG.00.1PSP\0\0\0\0\0".to_vec();
    b.extend(block(
        2,
        block(3, block(4, pl(10, 0, 16, 4, 8, tile.repeat(4)))),
    ));
    let mut im = decode(&b).unwrap();
    assert_eq!(
        &im.rgba[..16],
        &[
            248, 0, 0, 255, 0, 0, 248, 0, 165, 0, 82, 218, 82, 0, 165, 182
        ]
    );
    assert_eq!(encode(&b, 16, 4, &im.rgba).unwrap(), b);
    im.rgba[0] = 1;
    assert!(encode(&b, 16, 4, &im.rgba).is_err());
    tile[14] = 0;
    tile[15] = 255;
    let decoded = dxt::decode(&tile, 4, 4);
    assert_eq!(&decoded[4 * 6..4 * 8], &[165, 0, 82, 0, 82, 0, 165, 255]);
}

#[test]
fn lossy_dxt_preparation_preserves_unselected_blocks_and_rejects_bad_writers() {
    let mut tile = [0u8; 16];
    tile[4..6].copy_from_slice(&0xf800u16.to_le_bytes());
    tile[14] = 255;
    let mut b = b"MIG.00.1PSP\0\0\0\0\0".to_vec();
    b.extend(block(
        2,
        block(3, block(4, pl(10, 0, 16, 4, 8, tile.repeat(4)))),
    ));
    let old = decode(&b).unwrap();
    let mut draft = old.rgba.clone();
    for y in 0..4 {
        for x in 0..4 {
            let i = (y * 16 + x) * 4;
            draft[i..i + 4].copy_from_slice(&[0, 255, 0, if x % 2 == 0 { 255 } else { 0 }]);
        }
    }
    let candidate = prepare_dxt(&b, &draft, &[[0, 0, 4, 4]]).unwrap();
    assert!(validate_prepared_dxt(&b, &candidate, &[[0, 0, 4, 4]]).is_ok());
    assert!(validate_prepared_dxt(&b, &candidate, &[[4, 0, 4, 4]]).is_err());
    let mut bad_metadata = candidate.clone();
    bad_metadata[12] ^= 1;
    assert!(validate_prepared_dxt(&b, &bad_metadata, &[[0, 0, 4, 4]]).is_err());
    let decoded = decode(&candidate).unwrap();
    assert!(decoded.rgba[1] >= 248);
    assert_eq!(decoded.rgba[3], 255);
    assert_eq!(decoded.rgba[7], 0);
    let (p, _) = layout(&b).unwrap();
    assert_eq!(&candidate[..p.start], &b[..p.start]);
    assert_eq!(&candidate[p.start + 16..], &b[p.start + 16..]);
    for y in 0..4 {
        assert_eq!(
            &decoded.rgba[(y * 16 + 4) * 4..(y + 1) * 16 * 4],
            &old.rgba[(y * 16 + 4) * 4..(y + 1) * 16 * 4]
        );
    }
    for rects in [
        vec![],
        vec![[0, 0, 3, 4]],
        vec![[0, 0, 20, 4]],
        vec![[0, 0, 4, 4]; 2],
        vec![[usize::MAX - 3, 0, 8, 4]],
    ] {
        assert!(prepare_dxt(&b, &draft, &rects).is_err());
    }
    draft[16] = 99;
    assert!(prepare_dxt(&b, &draft, &[[0, 0, 4, 4]]).is_err());
    assert!(encode(&b, 16, 4, &decoded.rgba).is_err());
}
