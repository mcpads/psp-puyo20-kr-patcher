use super::*;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};
fn fixture() -> Vec<u8> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let opt = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9));
    w.start_file("mainmenu.snt", opt).unwrap();
    w.write_all(&(0..=255).cycle().take(1024).collect::<Vec<u8>>())
        .unwrap();
    w.start_file("keep.snc", opt).unwrap();
    w.write_all(b"unchanged").unwrap();
    w.finish().unwrap().into_inner()
}
#[test]
fn identity_and_changed_member() {
    let b = fixture();
    let original = (0..=255).cycle().take(1024).collect::<Vec<u8>>();
    assert_eq!(replace(&b, "mainmenu.snt", &original).unwrap().0, b);
    let (changed, r) = replace(&b, "mainmenu.snt", &vec![65; 1024]).unwrap();
    assert_eq!(changed.len(), b.len());
    assert!(!r.original_stream_reused);
    assert!(r.comment_padding > 0);
}
#[test]
fn resizing_and_growth_beyond_capacity_fail() {
    let b = fixture();
    assert!(replace(&b, "mainmenu.snt", b"bad").is_err());
    let (small, _) = replace(&b, "mainmenu.snt", &vec![65; 1024]).unwrap();
    // Space parked by the earlier replacement is reclaimed, so the original fits again.
    let original = (0..=255).cycle().take(1024).collect::<Vec<u8>>();
    let (restored, _) = replace(&small, "mainmenu.snt", &original).unwrap();
    assert_eq!(restored.len(), b.len());
    let mut state = 7u32;
    let noise: Vec<u8> = (0..1024)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 16) as u8
        })
        .collect();
    assert!(replace(&b, "mainmenu.snt", &noise).is_err());
}

#[test]
fn later_replacement_uses_space_parked_in_an_earlier_member() {
    let mut state = 0x9e37_79b9u32;
    let mut noise = |n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    };
    let first = noise(90_000);
    let second = noise(2_000);
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let opt = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    w.start_file("first", opt).unwrap();
    w.write_all(&first).unwrap();
    w.start_file("second", opt).unwrap();
    w.write_all(&vec![1; 2_000]).unwrap();
    let raw = w.finish().unwrap().into_inner();
    let (shrunk, r) = replace(&raw, "first", &vec![0; first.len()]).unwrap();
    assert!(r.deflate_padding > 0);
    // The incompressible second member only fits by reusing the first member's padding.
    let (both, _) = replace(&shrunk, "second", &second).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(&both)).unwrap();
    let mut out = Vec::new();
    archive
        .by_name("first")
        .unwrap()
        .read_to_end(&mut out)
        .unwrap();
    assert_eq!(out, vec![0; first.len()]);
    out.clear();
    archive
        .by_name("second")
        .unwrap()
        .read_to_end(&mut out)
        .unwrap();
    assert_eq!(out, second);
}

#[test]
fn large_compression_savings_preserve_archive_and_member_contents() {
    let mut state = 0x12345678u32;
    let original: Vec<u8> = (0..150_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.set_comment("preserve comment").unwrap();
    let opt = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    w.start_file("target", opt).unwrap();
    w.write_all(&original).unwrap();
    w.start_file("keep", opt).unwrap();
    w.write_all(b"untouched").unwrap();
    let raw = w.finish().unwrap().into_inner();
    let replacement = vec![0; original.len()];
    let (output, receipt) = replace(&raw, "target", &replacement).unwrap();
    assert_eq!(output.len(), raw.len());
    assert!(receipt.deflate_padding > 0);
    let mut archive = ZipArchive::new(Cursor::new(&output)).unwrap();
    assert!(archive.comment().starts_with(b"preserve comment"));
    let mut decoded = Vec::new();
    archive
        .by_name("target")
        .unwrap()
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, replacement);
    decoded.clear();
    archive
        .by_name("keep")
        .unwrap()
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, b"untouched");
}

#[test]
fn overflow_can_recompress_an_unchanged_member_without_changing_its_content() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let normal = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9));
    writer.start_file("target", normal).unwrap();
    writer.write_all(&[0; 512]).unwrap();
    // A valid but inefficient DEFLATE stream offers space for the edited target.
    let fast = normal.compression_level(Some(1));
    let keep = vec![7; 262_144];
    writer.start_file("keep", fast).unwrap();
    writer.write_all(&keep).unwrap();
    let raw = writer.finish().unwrap().into_inner();
    let replacement: Vec<_> = (0..=255).cycle().take(512).collect();
    let (output, receipt) = replace(&raw, "target", &replacement).unwrap();
    assert_eq!(output.len(), raw.len());
    assert_eq!(receipt.recompressed_unchanged_members, vec!["keep"]);
    let mut archive = ZipArchive::new(Cursor::new(output)).unwrap();
    let mut decoded = Vec::new();
    archive
        .by_name("keep")
        .unwrap()
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, keep);
    decoded.clear();
    archive
        .by_name("target")
        .unwrap()
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, replacement);
}
