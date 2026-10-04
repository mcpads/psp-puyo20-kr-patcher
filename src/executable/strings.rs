//! In-place replacement of UTF-8 strings in the plain EBOOT (system dialogs, save titles).
//!
//! Each entry names a segment-relative address and the exact original string. The Korean
//! text plus its terminator must fit the original slot (the string and the zero padding up
//! to the next string), must keep the same printf-style specifiers, and the slot must not
//! overlap a relocation. The rest of the slot is zero-filled.
use super::elf::{Program, map_address};
use crate::sha256;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, ops::Range, path::Path};

fn specifiers(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let start = i;
            i += 1;
            while i < b.len() && !b[i].is_ascii_alphabetic() && b[i] != b'%' {
                i += 1;
            }
            out.push(s[start..(i + 1).min(s.len())].to_string());
        }
        i += 1;
    }
    out
}

pub(super) struct StringPlan {
    pub writes: Vec<(Range<usize>, Vec<u8>)>,
    pub receipt: Value,
}

/// Plan the configured string replacements against the original plain ELF.
pub(super) fn plan(
    root: &Path,
    config: &Value,
    plain: &[u8],
    headers: &[Program],
    base: u32,
    relocations: &[u32],
) -> Result<Option<StringPlan>> {
    let Some(strings) = config.get("strings") else {
        return Ok(None);
    };
    let path = strings["translation"]
        .as_str()
        .context("strings translation")?;
    let raw = fs::read(root.join(path))?;
    ensure!(
        strings["translation_sha256"] == sha256(&raw),
        "EBOOT string translation identity mismatch"
    );
    let doc: Value = serde_json::from_slice(&raw)?;
    let mut writes: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
    let mut receipts = Vec::new();
    for e in doc["entries"].as_array().context("entries")? {
        let vaddr = u32::from_str_radix(
            e["vaddr"]
                .as_str()
                .context("vaddr")?
                .trim_start_matches("0x"),
            16,
        )?;
        let source = e["source"].as_str().context("source")?.as_bytes();
        let korean = e["korean"].as_str().context("korean")?;
        ensure!(!korean.contains('\0'), "embedded terminator");
        ensure!(
            specifiers(e["source"].as_str().unwrap_or_default()) == specifiers(korean),
            "format specifiers differ: {}",
            e["id"]
        );
        let address = base.checked_add(vaddr).context("address overflow")?;
        let offset = map_address(headers, base, address, source.len() + 1)?;
        ensure!(
            plain.get(offset..offset + source.len()) == Some(source)
                && plain[offset + source.len()] == 0,
            "EBOOT string source mismatch: {}",
            e["id"]
        );
        let mut end = offset + source.len();
        while end < plain.len() && plain[end] == 0 {
            end += 1;
        }
        let slot = end - offset;
        ensure!(
            korean.len() < slot,
            "EBOOT string exceeds its slot: {} needs {} of {}",
            e["id"],
            korean.len() + 1,
            slot
        );
        ensure!(
            relocations
                .iter()
                .all(|r| u64::from(*r) + 4 <= u64::from(address)
                    || u64::from(*r) >= u64::from(address) + slot as u64),
            "EBOOT string slot overlaps a relocation: {}",
            e["id"]
        );
        ensure!(
            writes
                .iter()
                .all(|(r, _)| r.end <= offset || r.start >= end),
            "overlapping EBOOT string slots"
        );
        let mut bytes = korean.as_bytes().to_vec();
        bytes.resize(slot, 0);
        receipts.push(json!({"id":e["id"],"address":address,"file_offset":offset,"slot":slot,"korean_bytes":korean.len()}));
        writes.push((offset..end, bytes));
    }
    Ok(Some(StringPlan {
        receipt: json!({"translation":path,"translation_sha256":sha256(&raw),"entries":receipts}),
        writes,
    }))
}

#[cfg(test)]
mod tests {
    use super::specifiers;

    #[test]
    fn specifiers_are_compared_in_order() {
        assert_eq!(specifiers("あと%s 以上"), vec!["%s"]);
        assert_eq!(specifiers("%d개 %s"), vec!["%d", "%s"]);
        assert!(specifiers("없음").is_empty());
    }
}
