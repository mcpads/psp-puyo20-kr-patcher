//! Relative oo_disk_image tree. Coordinates are relative to the table start.
use crate::graphics::bytes;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, ops::Range};

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Record {
    pub hash_path: Vec<u32>,
    pub record_offset: usize,
    pub offset: u32,
    pub size_bytes: u32,
    pub tag_unresolved: u32,
    pub member: u32,
    pub ordinal: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Relocation {
    pub record_offset: usize,
    pub hash_path: Vec<u32>,
    pub before_offset: u32,
    pub before_size: u32,
    pub offset: u32,
    pub size: u32,
}

/// Change only GAME offset/size words, retaining every other record and tree byte.
pub fn relocate(data: &[u8], changes: &[Relocation], archive_size: u64) -> Result<Vec<u8>> {
    let original = Index::parse(data)?;
    ensure!(original.size_bytes == data.len(), "index trailing bytes");
    let mut out = data.to_vec();
    let mut selected = BTreeSet::new();
    for change in changes {
        ensure!(
            selected.insert(change.record_offset),
            "duplicate index write"
        );
        let record = original
            .records
            .iter()
            .find(|r| r.record_offset == change.record_offset)
            .context("index relocation record missing")?;
        ensure!(
            record.hash_path == change.hash_path
                && record.offset == change.before_offset
                && record.size_bytes == change.before_size
                && record.member == u32::MAX,
            "index relocation source mismatch"
        );
        ensure!(
            change.offset.is_multiple_of(2048)
                && change.size > 0
                && u64::from(change.offset) + u64::from(change.size).div_ceil(2048) * 2048
                    <= archive_size,
            "invalid sector-aligned GAME extent"
        );
        let at = record.record_offset + 4;
        out[at..at + 4].copy_from_slice(&change.offset.to_le_bytes());
        out[at + 4..at + 8].copy_from_slice(&change.size.to_le_bytes());
    }
    let after = Index::parse(&out)?;
    ensure!(
        after.node_offsets == original.node_offsets
            && after.records.len() == original.records.len(),
        "index structure changed"
    );
    for (old, new) in original.records.iter().zip(&after.records) {
        if !selected.contains(&old.record_offset) {
            ensure!(old == new, "unplanned index record change");
            continue;
        }
        // The loader reads complete sectors. Keep its rounded read out of every
        // other indexed asset, including unchanged duplicates elsewhere.
        let start = u64::from(new.offset);
        let end = start + u64::from(new.size_bytes).div_ceil(2048) * 2048;
        for other in &after.records {
            if other.record_offset != new.record_offset && other.size_bytes > 0 {
                ensure!(
                    end <= u64::from(other.offset)
                        || start >= u64::from(other.offset) + u64::from(other.size_bytes),
                    "relocated GAME read overlaps another asset"
                );
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Index {
    pub records: Vec<Record>,
    pub node_offsets: BTreeSet<usize>,
    pub size_bytes: usize,
}

fn word(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(data, at, 4)?.try_into()?))
}

fn claim(data: &[u8], ranges: &mut Vec<Range<usize>>, at: usize, length: usize) -> Result<()> {
    if length == 0 {
        return Ok(());
    }
    bytes(data, at, length)?;
    ensure!(at.is_multiple_of(4), "unaligned index structure");
    ranges.push(at..at.checked_add(length).context("index range overflow")?);
    Ok(())
}

impl Index {
    pub fn parse(input: &[u8]) -> Result<Self> {
        // Analysis callers may pass a RAM tail; the tree is bounded independently.
        let data = &input[..input.len().min(1024 * 1024)];
        ensure!(
            bytes(data, 0, 16)? == b"oo_disk_image___",
            "archive signature mismatch"
        );
        let expected = word(data, 0x54)? as usize;
        ensure!(expected <= 10000, "archive population limit");
        let mut result = Self {
            records: Vec::new(),
            node_offsets: BTreeSet::new(),
            size_bytes: 0,
        };
        let mut ranges = Vec::new();
        claim(data, &mut ranges, 0, 0x68)?;
        result.walk(data, 0x68, &[], &mut ranges)?;
        ensure!(
            result.records.len() == expected,
            "archive population mismatch"
        );
        ranges.sort_by_key(|r| r.start);
        ensure!(
            ranges.windows(2).all(|r| r[0].end <= r[1].start),
            "overlapping index structures"
        );
        result.size_bytes = ranges.last().context("empty index")?.end;
        Ok(result)
    }

    fn walk(
        &mut self,
        data: &[u8],
        node: usize,
        parts: &[u32],
        ranges: &mut Vec<Range<usize>>,
    ) -> Result<()> {
        ensure!(
            parts.len() <= 32 && self.node_offsets.len() < 10000 && self.node_offsets.insert(node),
            "recursive or excessive archive tree"
        );
        claim(data, ranges, node, 24)?;
        let dirs = word(data, node + 4)? as usize;
        let files = word(data, node + 12)? as usize;
        ensure!(
            dirs <= 10000 && files <= 10000 && self.records.len() + files <= 10000,
            "archive node population"
        );
        let dir_at = node
            .checked_add(word(data, node + 8)? as usize)
            .context("directory offset")?;
        let file_at = node
            .checked_add(word(data, node + 16)? as usize)
            .context("file offset")?;
        claim(data, ranges, dir_at, dirs * 4)?;
        claim(data, ranges, file_at, files * 24)?;
        for i in 0..dirs {
            let slot = dir_at + i * 4;
            let child = slot
                .checked_add(word(data, slot)? as usize)
                .context("child offset")?;
            let mut path = parts.to_vec();
            path.push(word(data, child)?);
            self.walk(data, child, &path, ranges)?;
        }
        for i in 0..files {
            let at = file_at + i * 24;
            let mut path = parts.to_vec();
            path.push(word(data, at)?);
            self.records.push(Record {
                hash_path: path,
                record_offset: at,
                offset: word(data, at + 4)?,
                size_bytes: word(data, at + 8)?,
                tag_unresolved: word(data, at + 12)?,
                member: word(data, at + 16)?,
                ordinal: word(data, at + 20)?,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
