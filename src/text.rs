//! Lossless extraction of the observed FNT/MTX population. Unknown token operands
//! are not decoded as prose; a raw record remains the authority in that case.
use crate::{graphics, sha256, source};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

pub const CONTROL_SPEC: &str = include_str!("../config/text-controls.json");
#[derive(Deserialize)]
pub struct ControlTable {
    pub address: u32,
    pub sha256: String,
    pub entries: Vec<Control>,
}
#[derive(Deserialize)]
pub struct Control {
    pub code: u16,
    pub operand_units: usize,
    pub object_offset: i16,
    pub virtual_slot: i16,
    pub handler: u32,
}

fn decode_record(
    units: &[u16],
    glyphs: &[[u16; 2]],
    controls: &[Control],
) -> Option<(String, Vec<Value>)> {
    let mut preview = String::new();
    let mut tokens = Vec::new();
    let mut pos = 0;
    let mut terminal = false;
    while pos < units.len() {
        let unit = units[pos];
        if terminal && unit != 0xffff {
            return None;
        }
        let start = pos;
        pos += 1;
        if let Some(glyph) = glyphs.get(unit as usize) {
            let c = char::from_u32(glyph[0] as u32)?;
            preview.push(c);
            tokens.push(json!({"unit_offset":start,"glyph_slot":unit,"character":c}));
        } else {
            let control = controls.iter().find(|c| c.code == unit)?;
            let operands = units.get(pos..pos.checked_add(control.operand_units)?)?;
            preview.push_str(&format!("<{unit:04X}"));
            for operand in operands {
                preview.push_str(&format!(":{operand:04X}"));
            }
            preview.push('>');
            tokens.push(json!({"unit_offset":start,"control":unit,"operands":operands}));
            pos += control.operand_units;
            terminal |= unit == 0xffff;
        }
    }
    terminal.then_some((preview, tokens))
}

fn word(b: &[u8], p: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(graphics::bytes(b, p, 4)?.try_into()?) as usize)
}

#[derive(Serialize, Deserialize)]
pub struct Record {
    pub offset: usize,
    pub units: Vec<u16>,
}

#[derive(Serialize, Deserialize)]
pub struct Mtx {
    pub header: Vec<u8>,
    pub group_first_records: Vec<usize>,
    pub records: Vec<Record>,
}

impl Mtx {
    /// Repack the indexed pool without changing the loader's header boundary or
    /// group/record numbering. Spare space belongs to the last replaced record,
    /// so every untranslated record retains its complete original unit sequence.
    pub fn replace_records(&self, replacements: &BTreeMap<usize, Vec<u16>>) -> Result<Vec<u8>> {
        let original = self.to_bytes()?;
        let (&last, _) = replacements
            .last_key_value()
            .context("empty MTX replacement")?;
        ensure!(
            last < self.records.len(),
            "MTX replacement outside record table"
        );
        let mut records: Vec<_> = self
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| replacements.get(&i).unwrap_or(&r.units).clone())
            .collect();
        ensure!(
            records.iter().all(|r| r.last() == Some(&0xffff)),
            "replacement lacks terminator"
        );
        let used = self.header.len() + records.iter().map(|r| r.len() * 2).sum::<usize>();
        ensure!(used <= original.len(), "MTX pool capacity exceeded");
        records[last].extend(std::iter::repeat_n(0xffff, (original.len() - used) / 2));
        let mut out = self.header.clone();
        let table = word(&out, 8)?;
        for (i, units) in records.iter().enumerate() {
            let at = table + 4 * i;
            let offset = u32::try_from(out.len())?;
            out[at..at + 4].copy_from_slice(&offset.to_le_bytes());
            for u in units {
                out.extend_from_slice(&u.to_le_bytes());
            }
        }
        let parsed = Self::parse(&out)?;
        ensure!(
            out.len() == original.len()
                && parsed.header.len() == self.header.len()
                && out[..table] == original[..table]
                && parsed.group_first_records == self.group_first_records
                && parsed.records.len() == self.records.len(),
            "MTX hierarchy changed"
        );
        for (i, r) in parsed.records.iter().enumerate() {
            ensure!(r.units == records[i], "MTX repack readback mismatch");
            if !replacements.contains_key(&i) {
                ensure!(
                    r.units == self.records[i].units,
                    "untranslated MTX record changed"
                );
            }
        }
        Ok(out)
    }
    pub fn parse(b: &[u8]) -> Result<Self> {
        ensure!(
            b.len().is_multiple_of(2) && word(b, 0)? == b.len(),
            "MTX size mismatch"
        );
        // All 96 pinned files use one root pointing to a table of record groups.
        ensure!(word(b, 4)? == 8, "unsupported MTX root");
        let table = word(b, 8)?;
        ensure!(
            table >= 12 && table.is_multiple_of(4),
            "bad MTX group table"
        );
        let payload = word(b, table)?;
        ensure!(
            payload > table && payload <= b.len() && payload.is_multiple_of(4),
            "bad MTX payload"
        );
        let mut group_first_records = Vec::new();
        let mut previous = None;
        for p in (8..table).step_by(4) {
            let target = word(b, p)?;
            ensure!(
                target >= table
                    && target < payload
                    && target.is_multiple_of(4)
                    && previous.is_none_or(|v| target > v),
                "invalid MTX group pointer"
            );
            group_first_records.push((target - table) / 4);
            previous = Some(target);
        }
        let mut records = Vec::new();
        for p in (table..payload).step_by(4) {
            let start = word(b, p)?;
            let end = if p + 4 == payload {
                b.len()
            } else {
                word(b, p + 4)?
            };
            ensure!(
                start >= payload
                    && start < end
                    && end <= b.len()
                    && start.is_multiple_of(2)
                    && end.is_multiple_of(2),
                "invalid MTX record span"
            );
            let units = b[start..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| u16::from_le_bytes([v[0], v[1]]))
                .collect::<Vec<_>>();
            ensure!(units.last() == Some(&0xffff), "MTX record lacks terminator");
            records.push(Record {
                offset: start,
                units,
            });
        }
        Ok(Self {
            header: b[..payload].to_vec(),
            group_first_records,
            records,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut b = self.header.clone();
        for record in &self.records {
            ensure!(record.offset == b.len(), "record gap or overlap");
            for unit in &record.units {
                b.extend_from_slice(&unit.to_le_bytes());
            }
        }
        let parsed = Self::parse(&b)?;
        ensure!(
            parsed.header == self.header
                && parsed.group_first_records == self.group_first_records
                && parsed.records.len() == self.records.len()
                && parsed
                    .records
                    .iter()
                    .zip(&self.records)
                    .all(|(a, b)| a.offset == b.offset && a.units == b.units),
            "MTX metadata disagrees with bytes"
        );
        Ok(b)
    }
}

pub fn inspect_pair(font: &[u8], text: &[u8]) -> Result<Value> {
    ensure!(
        graphics::bytes(font, 0, 4)? == b"FNT\0",
        "FNT signature mismatch"
    );
    let count = word(font, 12)?;
    ensure!(
        count > 0 && count < 0xf800,
        "FNT population outside observed encoding range"
    );
    let table = graphics::bytes(font, 16, count * 4)?;
    let glyphs = table
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| {
            [
                u16::from_le_bytes([v[0], v[1]]),
                u16::from_le_bytes([v[2], v[3]]),
            ]
        })
        .collect::<Vec<_>>();
    let payload = 16 + count * 4;
    let image = graphics::decode(font.get(payload..).context("FNT image missing")?)?;
    let mtx = Mtx::parse(text)?;
    // Verify the serialized exchange data, not only the in-memory parse.
    let raw_mtx = serde_json::to_value(&mtx)?;
    let readback: Mtx = serde_json::from_value(raw_mtx.clone())?;
    ensure!(readback.to_bytes()? == text, "MTX JSON roundtrip mismatch");
    let mut non_slot_units = BTreeMap::<String, usize>::new();
    let mut empty = 0;
    let mut unresolved = 0;
    let mut strings = Vec::new();
    let controls: ControlTable = serde_json::from_str(CONTROL_SPEC)?;
    for record in &mtx.records {
        for &u in &record.units {
            if u as usize >= count {
                *non_slot_units.entry(format!("{u:04X}")).or_default() += 1;
            }
        }
        let decoded = decode_record(&record.units, &glyphs, &controls.entries);
        let is_empty = record.units.iter().all(|&u| u == 0xffff);
        empty += usize::from(is_empty);
        unresolved += usize::from(decoded.is_none());
        strings.push(json!({"offset":record.offset,"units":record.units,
            "text":decoded.as_ref().map(|d| &d.0),
            "tokens":decoded.as_ref().map(|d| &d.1), "empty":is_empty}));
    }
    Ok(
        json!({"font_header_values":[word(font,4)?,word(font,8)?],"glyph_count":count,
        "font_payload_offset":payload,"glyphs":glyphs,
        "image":{"width":image.width,"height":image.height,"format":image.format,"order":image.order,"rgba_sha256":sha256(&image.rgba)},
        "payload_offset":mtx.header.len(),"raw_mtx":raw_mtx,"strings":strings,
        "summary":{"groups":mtx.group_first_records.len(),"records":mtx.records.len(),"empty_records":empty,
            "nonempty_records":mtx.records.len()-empty,"unresolved_records":unresolved,"non_slot_units":non_slot_units},
        "control_semantics":"PSP table defines operand lengths; effects remain numeric; unknown or malformed records omit preview and tokens",
        "control_spec_sha256":sha256(CONTROL_SPEC.as_bytes()),
        "font_sha256":sha256(font),"text_sha256":sha256(text),"mtx_json_roundtrip":true}),
    )
}

#[derive(Deserialize)]
pub(crate) struct Extent {
    pub offset: u64,
    pub size: usize,
    pub sha256: String,
}
#[derive(Deserialize)]
pub(crate) struct Pair {
    pub name: String,
    pub font: Extent,
    pub text: Extent,
}
#[derive(Deserialize)]
pub(crate) struct Catalog {
    pub source_sha256: String,
    pub game_offset: u64,
    pub game_size: u64,
    pub pairs: Vec<Pair>,
}

pub fn extract(root: &Path, source_path: &Path, output: &Path) -> Result<Value> {
    ensure!(!output.exists(), "output already exists");
    let profile: source::SourceProfile =
        serde_json::from_slice(&fs::read(root.join("config/source.json"))?)?;
    let raw = fs::read(root.join("config/text-resources.json"))?;
    let catalog: Catalog = serde_json::from_slice(&raw)?;
    ensure!(
        catalog.source_sha256 == profile.sha256 && catalog.pairs.len() == 96,
        "text catalog identity/population mismatch"
    );
    let mut source = source::verify(source_path, &profile)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = tempfile::Builder::new()
        .prefix(".text-extract-")
        .tempdir_in(parent)?;
    let mut names = std::collections::BTreeSet::new();
    let mut rows = Vec::new();
    let mut totals = [0usize; 4];
    for pair in &catalog.pairs {
        ensure!(
            pair.name.starts_with("text/")
                && pair
                    .name
                    .split('/')
                    .all(|s| !s.is_empty()
                        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
                && names.insert(&pair.name),
            "unsafe/duplicate text path"
        );
        let mut data = Vec::new();
        for extent in [&pair.font, &pair.text] {
            ensure!(
                extent.offset >= catalog.game_offset
                    && extent.size <= 16 * 1024 * 1024
                    && extent
                        .offset
                        .checked_add(extent.size as u64)
                        .context("extent overflow")?
                        <= catalog
                            .game_offset
                            .checked_add(catalog.game_size)
                            .context("GAME overflow")?,
                "text outside GAME extent"
            );
            let bytes = source::read_extent(&mut source, extent.offset, extent.size)?;
            ensure!(
                sha256(&bytes) == extent.sha256,
                "text identity mismatch: {}",
                pair.name
            );
            data.push(bytes);
        }
        let result = inspect_pair(&data[0], &data[1]).with_context(|| pair.name.clone())?;
        let path = temp.path().join(&pair.name);
        fs::create_dir_all(path.parent().context("pair parent")?)?;
        fs::write(path.with_extension("fnt"), &data[0])?;
        fs::write(path.with_extension("mtx"), &data[1])?;
        let packet = serde_json::to_vec(&result)?;
        fs::write(path.with_extension("json"), &packet)?;
        for (i, key) in [
            "records",
            "empty_records",
            "nonempty_records",
            "unresolved_records",
        ]
        .iter()
        .enumerate()
        {
            totals[i] += result["summary"][key].as_u64().context("summary count")? as usize;
        }
        rows.push(json!({"name":pair.name,"font_sha256":pair.font.sha256,"text_sha256":pair.text.sha256,
            "packet_sha256":sha256(&packet),"summary":result["summary"],"glyph_count":result["glyph_count"],"image":result["image"]}));
    }
    // Recheck input identity before publishing a corpus that may take time to extract.
    source::verify(source_path, &profile)?;
    let report = json!({"scope":"raw fixed-source text records; not dialogue count or runtime coverage", "source_sha256":profile.sha256,
        "catalog_sha256":sha256(&raw),"pairs":rows,"totals":{"pairs":rows.len(),"records":totals[0],"empty_records":totals[1],
        "nonempty_records":totals[2],"unresolved_records":totals[3]},"all_mtx_json_roundtrips":true});
    fs::write(
        temp.path().join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    ensure!(!output.exists(), "output appeared during extraction");
    fs::rename(temp.path(), output)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
