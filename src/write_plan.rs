use crate::{
    sha256,
    source::{SourceProfile, hash_reader, read_extent},
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};

#[derive(Clone)]
pub struct ExpectedWrite {
    pub offset: u64,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}
#[derive(Serialize)]
pub struct WriteReceipt {
    pub offset: u64,
    pub length: usize,
    pub before_sha256: String,
    pub after_sha256: String,
    pub changed_bytes: usize,
}
pub fn validate(
    writes: &[ExpectedWrite],
    size: u64,
    allowed: &[std::ops::Range<u64>],
) -> Result<Vec<WriteReceipt>> {
    let mut end = 0;
    let mut result = Vec::new();
    for w in writes {
        ensure!(
            !w.before.is_empty() && w.before.len() == w.after.len(),
            "empty or resizing write"
        );
        let next = w
            .offset
            .checked_add(w.before.len() as u64)
            .context("write overflow")?;
        ensure!(
            w.offset >= end && next <= size,
            "overlapping/out-of-bounds writes"
        );
        ensure!(
            allowed
                .iter()
                .any(|range| w.offset >= range.start && next <= range.end),
            "protected range write"
        );
        end = next;
        result.push(WriteReceipt {
            offset: w.offset,
            length: w.before.len(),
            before_sha256: sha256(&w.before),
            after_sha256: sha256(&w.after),
            changed_bytes: w
                .before
                .iter()
                .zip(&w.after)
                .filter(|(a, b)| a != b)
                .count(),
        });
    }
    Ok(result)
}
pub fn apply_and_audit(
    source: &mut File,
    dest: &mut File,
    profile: &SourceProfile,
    writes: &[ExpectedWrite],
    allowed: &[std::ops::Range<u64>],
    output_len: u64,
) -> Result<(String, Vec<WriteReceipt>)> {
    // `output_len` below the source size truncates a verified tail (install data removal).
    ensure!(
        output_len <= profile.size_bytes,
        "output longer than source"
    );
    let receipts = validate(writes, output_len, allowed)?;
    for w in writes {
        ensure!(
            read_extent(source, w.offset, w.before.len())? == w.before,
            "expected original mismatch"
        );
    }
    source.rewind()?;
    dest.rewind()?;
    dest.set_len(0)?;
    std::io::copy(&mut Read::by_ref(source).take(output_len), dest)?;
    for w in writes {
        dest.seek(SeekFrom::Start(w.offset))?;
        dest.write_all(&w.after)?;
    }
    dest.flush()?;
    source.rewind()?;
    dest.rewind()?;
    let mut pos = 0u64;
    let mut old = vec![0; 1024 * 1024];
    let mut actual = vec![0; old.len()];
    while pos < output_len {
        let want = (output_len - pos).min(old.len() as u64) as usize;
        let n = source.read(&mut old[..want])?;
        ensure!(n > 0, "short source");
        dest.read_exact(&mut actual[..n])?;
        // Build the exact expected chunk from immutable source and declared writes.
        let mut expected = old[..n].to_vec();
        for w in writes {
            let start = pos.max(w.offset);
            let end = (pos + n as u64).min(w.offset + w.after.len() as u64);
            if start < end {
                expected[(start - pos) as usize..(end - pos) as usize].copy_from_slice(
                    &w.after[(start - w.offset) as usize..(end - w.offset) as usize],
                );
            }
        }
        ensure!(
            expected == actual[..n],
            "unexplained final diff at chunk {pos:#x}"
        );
        pos += n as u64;
    }
    ensure!(
        pos == output_len && dest.metadata()?.len() == pos,
        "output size mismatch"
    );
    source.rewind()?;
    let (size, hash) = hash_reader(source)?;
    ensure!(
        size == profile.size_bytes && hash == profile.sha256,
        "source changed during build"
    );
    dest.rewind()?;
    let (_, hash) = hash_reader(dest)?;
    Ok((hash, receipts))
}
#[cfg(test)]
mod tests;
