use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Deserialize)]
pub struct SourceProfile {
    pub size_bytes: u64,
    pub sha256: String,
}

pub fn hash_reader(reader: &mut impl Read) -> Result<(u64, String)> {
    let mut h = Sha256::new();
    let mut size = 0;
    let mut buffer = vec![0; 8 * 1024 * 1024];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        h.update(&buffer[..n]);
        size += n as u64;
    }
    Ok((size, format!("{:x}", h.finalize())))
}
pub fn verify(path: &Path, profile: &SourceProfile) -> Result<File> {
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let (size, hash) = hash_reader(&mut f)?;
    ensure!(
        size == profile.size_bytes && hash == profile.sha256,
        "source size/SHA-256 mismatch"
    );
    f.rewind()?;
    Ok(f)
}
pub fn read_extent(f: &mut File, offset: u64, length: usize) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(length as u64)
        .context("extent overflow")?;
    ensure!(end <= f.metadata()?.len(), "extent outside source");
    f.seek(SeekFrom::Start(offset))?;
    let mut b = vec![0; length];
    f.read_exact(&mut b)?;
    Ok(b)
}
