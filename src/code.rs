//! Read-only, address-aware inspection of pinned PSP code windows.
use crate::{sha256, source::SourceProfile};
use allegrex::{Allegrex, AnalysisPoint, VfpuPrefixState};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs, path::Path};
use typed_isa_core::StaticSemantics;

#[derive(Deserialize)]
struct Window {
    name: String,
    address: u32,
    length: usize,
    sha256: String,
}

#[derive(Deserialize)]
struct Profile {
    snapshot: SourceProfile,
    base_address: u32,
    windows: Vec<Window>,
}

fn inspect_window(bytes: &[u8], address: u32) -> Result<Vec<Value>> {
    ensure!(
        !bytes.is_empty() && bytes.len().is_multiple_of(4) && address.is_multiple_of(4),
        "unaligned or empty Allegrex window"
    );
    address
        .checked_add(u32::try_from(bytes.len() - 1)?)
        .context("Allegrex window address overflow")?;
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .map(|(index, bytes)| {
            let at = address + (index * 4) as u32;
            let instruction = match allegrex::decode_bytes(bytes, at) {
                Ok(instruction) => instruction,
                Err(error) => {
                    return Ok(json!({
                        "address": at,
                        "word": format!("{:08X}", u32::from_le_bytes(*bytes)),
                        "decode_error": error.to_string(),
                    }));
                }
            };
            ensure!(
                allegrex::encode_bytes(&instruction, at)? == *bytes,
                "instruction roundtrip mismatch at {at:#010x}"
            );
            // No execution or cross-instruction value analysis: incoming prefixes
            // are unknown rather than assumed inactive at an arbitrary window.
            let semantics = Allegrex::semantics(
                &instruction,
                &AnalysisPoint::new(at, VfpuPrefixState::unknown()),
            )?;
            Ok(json!({
                "address": at,
                "word": format!("{:08X}", u32::from_le_bytes(*bytes)),
                "instruction": allegrex::format_instruction(&instruction),
                "control_flow": format!("{:?}", semantics.control_flow),
                "location_accesses": format!("{:?}", semantics.location_accesses),
                "static_references": format!("{:?}", semantics.static_references),
            }))
        })
        .collect()
}

pub fn inspect(root: &Path, snapshot: &Path, output: &Path) -> Result<Value> {
    ensure!(!output.exists(), "output already exists");
    let config = fs::read(root.join("config/text-code.json"))?;
    let profile: Profile = serde_json::from_slice(&config)?;
    // Hash the same immutable in-memory bytes consumed by the decoder.
    let bytes = fs::read(snapshot)?;
    ensure!(
        bytes.len() as u64 == profile.snapshot.size_bytes
            && sha256(&bytes) == profile.snapshot.sha256,
        "snapshot size/SHA-256 mismatch"
    );
    let controls: crate::text::ControlTable = serde_json::from_str(crate::text::CONTROL_SPEC)?;
    let table_offset = controls
        .address
        .checked_sub(profile.base_address)
        .context("control table before snapshot")? as usize;
    let table_bytes = crate::graphics::bytes(&bytes, table_offset, controls.entries.len() * 12)?;
    ensure!(
        sha256(table_bytes) == controls.sha256,
        "control table hash mismatch"
    );
    let mut expected_table = Vec::new();
    for control in &controls.entries {
        expected_table.extend_from_slice(&control.code.to_le_bytes());
        expected_table.extend_from_slice(&u16::try_from(control.operand_units)?.to_le_bytes());
        expected_table.extend_from_slice(&control.object_offset.to_le_bytes());
        expected_table.extend_from_slice(&control.virtual_slot.to_le_bytes());
        expected_table.extend_from_slice(&control.handler.to_le_bytes());
    }
    ensure!(
        expected_table == table_bytes,
        "control specification differs from PSP table"
    );
    let mut windows = Vec::new();
    for window in &profile.windows {
        let offset = window
            .address
            .checked_sub(profile.base_address)
            .context("code address before snapshot base")? as usize;
        let code = crate::graphics::bytes(&bytes, offset, window.length)?;
        ensure!(sha256(code) == window.sha256, "code window hash mismatch");
        windows.push(json!({
            "name": window.name, "address": window.address,
            "length": window.length, "sha256": window.sha256,
            "instructions": inspect_window(code, window.address)?,
        }));
    }
    let count: usize = windows
        .iter()
        .map(|w| w["instructions"].as_array().unwrap().len())
        .sum();
    let unresolved = windows
        .iter()
        .flat_map(|w| w["instructions"].as_array().unwrap())
        .filter(|i| i.get("decode_error").is_some())
        .count();
    let report = json!({
        "scope": "static snapshot inspection; no execution, reachability proof or code writes",
        "isa": allegrex::PROFILE_ID,
        "dependency_lock_sha256": sha256(&fs::read(root.join("Cargo.lock"))?),
        "config_sha256": sha256(&config),
        "snapshot_sha256": profile.snapshot.sha256,
        "base_address": profile.base_address,
        "control_spec_sha256": sha256(crate::text::CONTROL_SPEC.as_bytes()),
        "control_table_verified": true,
        "word_count": count,
        "decoded_instruction_count": count - unresolved,
        "unresolved_word_count": unresolved,
        "decoded_instructions_roundtrip_equal": true,
        "all_words_decoded": unresolved == 0,
        "vfpu_input_prefixes": "unknown at every instruction",
        "windows": windows,
    });
    let parent = output.parent().context("output parent missing")?;
    fs::create_dir_all(parent)?;
    let temp = tempfile::tempdir_in(parent)?;
    fs::write(
        temp.path().join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    fs::rename(temp.path(), output)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
