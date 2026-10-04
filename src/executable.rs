//! Exact-source PRX extraction and public entry points for executable writes.
mod decrypt;
mod elf;
mod font_copy;
mod strings;
use crate::{graphics::bytes, sha256, source};
use anyhow::{Context, Result, ensure};
use decrypt::decrypt;
use elf::{map_address, programs, relocation_addresses};
pub use font_copy::{VerifiedFontCopy, plan_font_copy};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn word(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(b, at, 4)?.try_into()?))
}
fn half(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(b, at, 2)?.try_into()?))
}
pub fn extract(
    root: &Path,
    source_path: &Path,
    external: &Path,
    snapshot: &Path,
    output: &Path,
) -> Result<Value> {
    ensure!(!output.exists(), "output already exists");
    let raw = fs::read(root.join("config/executable.json"))?;
    let config: Value = serde_json::from_slice(&raw)?;
    let source_profile: source::SourceProfile =
        serde_json::from_slice(&fs::read(root.join("config/source.json"))?)?;
    ensure!(
        config["source_sha256"] == source_profile.sha256,
        "executable source profile drift"
    );
    let mut iso = source::verify(source_path, &source_profile)?;
    let input = &config["encrypted"];
    let encrypted = source::read_extent(
        &mut iso,
        input["offset"].as_u64().context("offset")?,
        input["size"].as_u64().context("size")? as usize,
    )?;
    ensure!(
        sha256(&encrypted) == input["sha256"].as_str().context("hash")?
            && bytes(&encrypted, 0, 4)? == b"~PSP"
            && half(&encrypted, 6)? == 0,
        "encrypted PRX identity or compression mismatch"
    );
    let memory = fs::read(snapshot)?;
    ensure!(
        memory.len() as u64
            == config["snapshot"]["size"]
                .as_u64()
                .context("snapshot size")?
            && sha256(&memory)
                == config["snapshot"]["sha256"]
                    .as_str()
                    .context("snapshot hash")?,
        "snapshot identity mismatch"
    );
    let temp = tempfile::tempdir()?;
    let dependency = decrypt(root, external, &encrypted, temp.path())?;
    let plain = fs::read(temp.path().join("plain.elf"))?;
    ensure!(
        plain.len() as u64 == config["plain"]["size"].as_u64().context("plain size")?
            && sha256(&plain) == config["plain"]["sha256"].as_str().context("plain hash")?
            && word(&encrypted, 0x28)? as usize == plain.len(),
        "decrypted ELF identity mismatch"
    );
    let headers = programs(&plain)?;
    let base = u32::try_from(config["load_base"].as_u64().context("load base")?)?;
    let relocations = relocation_addresses(&plain, &headers, base)?;
    let mut proofs = Vec::new();
    for proof in config["proofs"].as_array().context("proofs")? {
        let address = u32::try_from(proof["address"].as_u64().context("proof address")?)?;
        let size = proof["size"].as_u64().context("proof size")? as usize;
        let offset = map_address(&headers, base, address, size)?;
        ensure!(
            offset as u64 == proof["file_offset"].as_u64().context("proof offset")?,
            "ELF mapping drift"
        );
        let data = bytes(&plain, offset, size)?;
        let memory_offset = address.checked_sub(0x08800000).context("RAM mapping")? as usize;
        ensure!(
            sha256(data) == proof["sha256"].as_str().context("proof hash")?
                && data == bytes(&memory, memory_offset, size)?,
            "ELF/RAM correspondence mismatch"
        );
        let end = address
            .checked_add(u32::try_from(size)?)
            .context("proof overflow")?;
        let overlap = relocations
            .iter()
            .filter(|a| (**a as u64) < end as u64 && (**a as u64 + 4) > address as u64)
            .count();
        let instruction = if size == 4 {
            let decoded = allegrex::decode_bytes(data, address)?;
            ensure!(
                allegrex::encode_bytes(&decoded, address)? == data,
                "ELF instruction roundtrip mismatch"
            );
            Some(allegrex::format_instruction(&decoded))
        } else {
            None
        };
        proofs.push(json!({"name":proof["name"],"address":address,"file_offset":offset,"size":size,
            "sha256":sha256(data),"snapshot_equal":true,"relocation_overlap_count":overlap,"instruction":instruction}));
    }
    let report = json!({"scope":"original JP executable extraction and address proof; no code writes or runtime proof",
        "source_sha256":source_profile.sha256,"config_sha256":sha256(&raw),"encrypted":input,"plain":config["plain"],
        "load_base":base,"entry_address":base.checked_add(word(&plain,24)?).context("entry overflow")?,
        "programs":headers,"relocation_entry_count":relocations.len(),"proofs":proofs,"dependency":dependency,
        "snapshot_sha256":sha256(&memory),"dependency_lock_sha256":sha256(&fs::read(root.join("Cargo.lock"))?)});
    fs::create_dir_all(output)?;
    fs::write(output.join("original.elf"), plain)?;
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests;
