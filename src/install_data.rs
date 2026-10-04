//! Remove the encrypted install data (`/PSP_GAME/INSDIR`) from the ISO tail.
//!
//! PSP fan patches usually drop install data: the game prefers an installed copy on the
//! Memory Stick, which would bypass translated GAME.DAT assets. The directory is kept but
//! emptied (path tables stay valid), and the image is truncated at the first INSDIR file
//! after proving that nothing else lives there.
use crate::{source::read_extent, write_plan::ExpectedWrite};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::fs::File;

const SECTOR: u64 = 2048;

struct Record {
    lba: u64,
    size: u64,
    dir: bool,
    name: Vec<u8>,
    raw: Vec<u8>,
}

fn u32le(b: &[u8], o: usize) -> Result<u64> {
    Ok(u32::from_le_bytes(b.get(o..o + 4).context("short field")?.try_into()?) as u64)
}

fn records(iso: &mut File, lba: u64, size: u64) -> Result<Vec<Record>> {
    let data = read_extent(iso, lba * SECTOR, usize::try_from(size)?)?;
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let len = data[pos] as usize;
        if len == 0 {
            // Records never span sectors; skip padding to the next sector.
            pos = (pos / SECTOR as usize + 1) * SECTOR as usize;
            continue;
        }
        let raw = data.get(pos..pos + len).context("truncated record")?;
        ensure!(len >= 34 && raw[1] == 0, "unsupported directory record");
        let name_len = raw[32] as usize;
        out.push(Record {
            lba: u32le(raw, 2)?,
            size: u32le(raw, 10)?,
            dir: raw[25] & 2 != 0,
            name: raw.get(33..33 + name_len).context("name")?.to_vec(),
            raw: raw.to_vec(),
        });
        pos += len;
    }
    Ok(out)
}

fn walk(
    iso: &mut File,
    lba: u64,
    size: u64,
    path: &str,
    dirs: &mut Vec<(String, u64, u64)>,
    files: &mut Vec<(String, u64, u64)>,
) -> Result<()> {
    ensure!(dirs.len() < 4096, "directory tree too large");
    dirs.push((path.to_string(), lba, size));
    for r in records(iso, lba, size)? {
        if r.name == [0] || r.name == [1] {
            continue;
        }
        let name = String::from_utf8_lossy(&r.name);
        let name = name.split(';').next().unwrap_or_default();
        let child = format!("{path}/{name}");
        if r.dir {
            walk(iso, r.lba, r.size, &child, dirs, files)?;
        } else {
            files.push((child, r.lba * SECTOR, r.size));
        }
    }
    Ok(())
}

/// `(path, position, size)`: directories carry their LBA, files their byte offset.
pub(crate) type Entry = (String, u64, u64);

/// Every directory and file below the root.
pub(crate) fn tree(iso: &mut File) -> Result<(Vec<Entry>, Vec<Entry>)> {
    let pvd = read_extent(iso, 16 * SECTOR, SECTOR as usize)?;
    ensure!(
        pvd[0] == 1 && &pvd[1..6] == b"CD001",
        "primary volume descriptor"
    );
    let root = &pvd[156..190];
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    walk(
        iso,
        u32le(root, 2)?,
        u32le(root, 10)?,
        "",
        &mut dirs,
        &mut files,
    )?;
    Ok((dirs, files))
}

pub struct Plan {
    pub writes: Vec<ExpectedWrite>,
    pub output_len: u64,
    pub receipt: Value,
}

pub fn plan(iso: &mut File, iso_size: u64) -> Result<Plan> {
    let pvd = read_extent(iso, 16 * SECTOR, SECTOR as usize)?;
    ensure!(
        pvd[0] == 1 && &pvd[1..6] == b"CD001",
        "primary volume descriptor"
    );
    ensure!(
        u16::from_le_bytes(pvd[128..130].try_into()?) as u64 == SECTOR,
        "logical block size"
    );
    ensure!(
        u32le(&pvd, 80)? * SECTOR == iso_size,
        "volume size mismatch"
    );
    let (dirs, files) = tree(iso)?;
    let insdir = dirs
        .iter()
        .find(|(p, _, _)| p == "/PSP_GAME/INSDIR")
        .context("missing /PSP_GAME/INSDIR")?
        .clone();
    let (install, others): (Vec<_>, Vec<_>) = files
        .iter()
        .cloned()
        .partition(|(p, _, _)| p.starts_with("/PSP_GAME/INSDIR/"));
    ensure!(!install.is_empty(), "empty install directory");
    let cut = install.iter().map(|(_, o, _)| *o).min().context("cut")?;
    ensure!(cut % SECTOR == 0 && cut < iso_size, "unaligned cut");
    for (p, o, s) in &others {
        ensure!(o + s <= cut, "file after install data: {p}");
    }
    for (p, lba, s) in &dirs {
        ensure!(lba * SECTOR + s <= cut, "directory after install data: {p}");
    }
    // Everything discarded must be install data or zero padding.
    let mut spans: Vec<_> = install.iter().map(|(_, o, s)| (*o, o + s)).collect();
    spans.sort();
    let mut pos = cut;
    for (start, end) in spans.iter().copied().chain([(iso_size, iso_size)]) {
        ensure!(
            start >= pos || start == iso_size,
            "overlapping install files"
        );
        let mut gap = start.saturating_sub(pos);
        let mut at = pos;
        while gap > 0 {
            let n = gap.min(1 << 20);
            ensure!(
                read_extent(iso, at, n as usize)?.iter().all(|b| *b == 0),
                "non-zero data outside install files at {at:#x}"
            );
            at += n;
            gap -= n;
        }
        pos = pos.max(end);
    }
    ensure!(pos <= iso_size, "install data exceeds image");

    let mut writes = Vec::new();
    // Every volume descriptor that carries a volume space size gets the new size.
    let blocks = u32::try_from(cut / SECTOR)?;
    let mut sector = 16;
    loop {
        let vd = read_extent(iso, sector * SECTOR, SECTOR as usize)?;
        ensure!(&vd[1..6] == b"CD001", "volume descriptor set");
        match vd[0] {
            255 => break,
            1 | 2 => {
                let mut after = vd[80..88].to_vec();
                after[..4].copy_from_slice(&blocks.to_le_bytes());
                after[4..].copy_from_slice(&blocks.to_be_bytes());
                writes.push(ExpectedWrite {
                    offset: sector * SECTOR + 80,
                    before: vd[80..88].to_vec(),
                    after,
                });
            }
            0 | 3 => {}
            t => bail!("unsupported volume descriptor {t}"),
        }
        sector += 1;
        ensure!(sector < 64, "unterminated volume descriptor set");
    }
    // Keep only "." and ".." in INSDIR so the path tables remain valid.
    let (_, lba, size) = insdir;
    let before = read_extent(iso, lba * SECTOR, usize::try_from(size)?)?;
    let kept = records(iso, lba, size)?;
    ensure!(
        kept.len() >= 2 && kept[0].name == [0] && kept[1].name == [1],
        "INSDIR self and parent records"
    );
    let mut after = vec![0; before.len()];
    let head = kept[0].raw.len() + kept[1].raw.len();
    after[..head].copy_from_slice(&before[..head]);
    writes.push(ExpectedWrite {
        offset: lba * SECTOR,
        before,
        after,
    });
    writes.sort_by_key(|w| w.offset);
    let removed: u64 = install.iter().map(|(_, _, s)| s).sum();
    Ok(Plan {
        writes,
        output_len: cut,
        receipt: json!({
            "removed_files": install.len(),
            "removed_bytes": removed,
            "original_size": iso_size,
            "output_size": cut,
            "insdir_lba": lba,
            "volume_blocks": blocks,
        }),
    })
}
