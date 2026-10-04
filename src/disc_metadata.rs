//! Rewrite the XMB game title in `/PSP_GAME/PARAM.SFO` in place.
//!
//! The SFO keeps each value in a fixed-capacity slot, so a shorter or equal UTF-8 title
//! fits without moving any other entry. Only the TITLE length field and its data slot
//! change; the rest of the file must stay byte-identical.
use crate::{install_data, sha256, source::read_extent, write_plan::ExpectedWrite};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs, fs::File, ops::Range, path::Path};

#[derive(Deserialize)]
struct Config {
    param_sfo: ParamSfo,
}

#[derive(Deserialize)]
struct ParamSfo {
    path: String,
    source_sha256: String,
    title: Title,
}

#[derive(Deserialize)]
struct Title {
    source: String,
    korean: String,
}

pub struct Plan {
    pub writes: Vec<ExpectedWrite>,
    pub allowed: Range<u64>,
    pub receipt: Value,
}

struct Entry {
    index_offset: usize,
    format: u16,
    len: usize,
    max_len: usize,
    data_offset: usize,
}

fn u16le(b: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        b.get(o..o + 2).context("short SFO")?.try_into()?,
    ))
}

fn u32le(b: &[u8], o: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(b.get(o..o + 4).context("short SFO")?.try_into()?) as usize)
}

fn find_entry(sfo: &[u8], key: &str) -> Result<Entry> {
    ensure!(sfo.get(..4) == Some(b"\0PSF"), "PARAM.SFO magic");
    let (key_table, data_table, count) = (u32le(sfo, 8)?, u32le(sfo, 12)?, u32le(sfo, 16)?);
    for i in 0..count {
        let at = 20 + 16 * i;
        let name_at = key_table + u16le(sfo, at)? as usize;
        let end = sfo[name_at..]
            .iter()
            .position(|b| *b == 0)
            .context("unterminated SFO key")?;
        if &sfo[name_at..name_at + end] == key.as_bytes() {
            return Ok(Entry {
                index_offset: at,
                format: u16le(sfo, at + 2)?,
                len: u32le(sfo, at + 4)?,
                max_len: u32le(sfo, at + 8)?,
                data_offset: data_table + u32le(sfo, at + 12)?,
            });
        }
    }
    anyhow::bail!("PARAM.SFO has no {key}")
}

/// Replace one UTF-8 value inside its slot. Returns the edited file.
pub fn replace_utf8(sfo: &[u8], key: &str, expected: &str, value: &str) -> Result<Vec<u8>> {
    let entry = find_entry(sfo, key)?;
    ensure!(entry.format == 0x0204, "{key} is not a UTF-8 string");
    let slot = sfo
        .get(entry.data_offset..entry.data_offset + entry.max_len)
        .context("SFO slot outside file")?;
    let mut original = expected.as_bytes().to_vec();
    original.push(0);
    ensure!(
        entry.len == original.len() && slot[..entry.len] == original[..],
        "{key} does not hold the expected source value"
    );
    let mut replacement = value.as_bytes().to_vec();
    replacement.push(0);
    ensure!(
        replacement.len() <= entry.max_len,
        "{key} needs {} bytes but its slot holds {}",
        replacement.len(),
        entry.max_len
    );
    let mut out = sfo.to_vec();
    let data = &mut out[entry.data_offset..entry.data_offset + entry.max_len];
    data.fill(0);
    data[..replacement.len()].copy_from_slice(&replacement);
    out[entry.index_offset + 4..entry.index_offset + 8]
        .copy_from_slice(&u32::try_from(replacement.len())?.to_le_bytes());
    Ok(out)
}

pub fn plan(root: &Path, iso: &mut File) -> Result<Plan> {
    let config: Config =
        serde_json::from_slice(&fs::read(root.join("config/disc-metadata.json"))?)?;
    let sfo_config = config.param_sfo;
    let (_, files) = install_data::tree(iso)?;
    let (_, offset, size) = files
        .iter()
        .find(|(p, _, _)| *p == sfo_config.path)
        .with_context(|| format!("missing {}", sfo_config.path))?
        .clone();
    let before = read_extent(iso, offset, usize::try_from(size)?)?;
    ensure!(
        sha256(&before) == sfo_config.source_sha256,
        "PARAM.SFO differs from the pinned source"
    );
    let after = replace_utf8(
        &before,
        "TITLE",
        &sfo_config.title.source,
        &sfo_config.title.korean,
    )?;
    Ok(Plan {
        receipt: json!({
            "path": sfo_config.path,
            "source_sha256": sfo_config.source_sha256,
            "output_sha256": sha256(&after),
            "title": sfo_config.title.korean,
        }),
        allowed: offset..offset + size,
        writes: vec![ExpectedWrite {
            offset,
            before,
            after,
        }],
    })
}

#[cfg(test)]
mod tests;
