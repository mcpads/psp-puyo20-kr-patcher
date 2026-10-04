//! Verified FNT copy instruction and coupled GAME-index write plan.
use super::{
    decrypt::decrypt,
    elf::{map_address, programs, relocation_addresses},
    half, word,
};
use crate::{graphics::bytes, sha256, source};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub(super) fn font_copy_instruction(before: &[u8], address: u32) -> Result<([u8; 4], Value)> {
    use allegrex::{
        Allegrex, AnalysisPoint, GeneralRegister as R, Instruction, ThreeRegisterOperation as Op,
        VfpuPrefixState,
    };
    use typed_isa_core::StaticSemantics;
    let old = Instruction::ThreeRegister {
        operation: Op::AddUnsigned,
        destination: R::S1,
        left: R::S1,
        right: R::A1,
    };
    let new = Instruction::ThreeRegister {
        operation: Op::SubtractUnsigned,
        destination: R::S1,
        left: R::S1,
        right: R::A1,
    };
    ensure!(
        before == allegrex::encode_bytes(&old, address)?
            && allegrex::decode_bytes(before, address)? == old,
        "unexpected font copy instruction"
    );
    let after = allegrex::encode_bytes(&new, address)?;
    ensure!(
        allegrex::decode_bytes(&after, address)? == new,
        "font copy assembly roundtrip mismatch"
    );
    let point = AnalysisPoint::new(address, VfpuPrefixState::unknown());
    let old_sem = Allegrex::semantics(&old, &point)?;
    let new_sem = Allegrex::semantics(&new, &point)?;
    ensure!(
        old_sem.control_flow == new_sem.control_flow
            && old_sem.location_accesses == new_sem.location_accesses,
        "font copy patch changes control flow or accessed state"
    );
    Ok((
        after,
        json!({"address":address,"before_word":format!("{:08X}",u32::from_le_bytes(before.try_into()?)),
        "after_word":format!("{:08X}",u32::from_le_bytes(after)),"before":allegrex::format_instruction(&old),
        "after":allegrex::format_instruction(&new),"control_flow":format!("{:?}",new_sem.control_flow),
        "location_accesses":format!("{:?}",new_sem.location_accesses)}),
    ))
}

/// Evidence that the original executable and the bounded copy patch were verified.
/// Construction is restricted to the successful executable write planner.
pub struct VerifiedFontCopy {
    font_extents: Vec<(u64, usize)>,
}

impl VerifiedFontCopy {
    pub(crate) fn covers_font_extent(&self, offset: u64, size: usize) -> bool {
        self.font_extents.contains(&(offset, size))
    }
}

pub fn plan_font_copy(
    root: &Path,
    iso: &mut fs::File,
    external: &Path,
    story_fonts: &[crate::story_font::Plan],
    menu_changes: &[crate::archive_index::Relocation],
) -> Result<(crate::write_plan::ExpectedWrite, Value, VerifiedFontCopy)> {
    let raw = fs::read(root.join("config/executable.json"))?;
    let config: Value = serde_json::from_slice(&raw)?;
    let profile: source::SourceProfile =
        serde_json::from_slice(&fs::read(root.join("config/source.json"))?)?;
    ensure!(
        config["source_sha256"] == profile.sha256,
        "executable source profile drift"
    );
    let input = &config["encrypted"];
    let offset = input["offset"].as_u64().context("EBOOT offset")?;
    let before = source::read_extent(
        iso,
        offset,
        input["size"].as_u64().context("EBOOT size")? as usize,
    )?;
    ensure!(
        sha256(&before) == input["sha256"].as_str().context("EBOOT hash")?
            && bytes(&before, 0, 4)? == b"~PSP"
            && half(&before, 6)? == 0,
        "EBOOT source mismatch"
    );
    let temp = tempfile::tempdir()?;
    let dependency = decrypt(root, external, &before, temp.path())?;
    let plain = fs::read(temp.path().join("plain.elf"))?;
    ensure!(
        plain.len() as u64 == config["plain"]["size"].as_u64().context("plain size")?
            && sha256(&plain) == config["plain"]["sha256"].as_str().context("plain hash")?
            && word(&before, 0x28)? as usize == plain.len(),
        "decrypted EBOOT mismatch"
    );
    let base = u32::try_from(config["load_base"].as_u64().context("load base")?)?;
    let headers = programs(&plain)?;
    let address = 0x0889661c;
    let site = map_address(&headers, base, address, 4)?;
    let proof = config["proofs"]
        .as_array()
        .context("proofs")?
        .iter()
        .find(|p| p["name"] == "font_copy_add")
        .context("font copy proof")?;
    ensure!(
        proof["address"] == address
            && proof["file_offset"] == site
            && proof["size"] == 4
            && proof["sha256"] == sha256(bytes(&plain, site, 4)?),
        "font copy mapping drift"
    );
    let relocations = relocation_addresses(&plain, &headers, base)?;
    ensure!(
        relocations
            .iter()
            .all(|p| *p as u64 + 4 <= address as u64 || *p >= address + 4),
        "font copy instruction overlaps relocation"
    );
    let (replacement, instruction) = font_copy_instruction(bytes(&plain, site, 4)?, address)?;
    let mut after = plain.clone();
    after[site..site + 4].copy_from_slice(&replacement);
    let mut index_receipt = Value::Null;
    let mut data_writes = Vec::new();
    if !story_fonts.is_empty() || !menu_changes.is_empty() {
        let proof = config["proofs"]
            .as_array()
            .context("proofs")?
            .iter()
            .find(|p| p["name"] == "game_index")
            .context("GAME index proof")?;
        let address = u32::try_from(proof["address"].as_u64().context("index address")?)?;
        let size = usize::try_from(proof["size"].as_u64().context("index size")?)?;
        let offset = map_address(&headers, base, address, size)?;
        let original = bytes(&plain, offset, size)?;
        ensure!(
            proof["file_offset"] == offset
                && proof["sha256"] == sha256(original)
                && relocations
                    .iter()
                    .all(|p| u64::from(*p) + 4 <= u64::from(address)
                        || u64::from(*p) >= u64::from(address) + size as u64),
            "GAME index source/mapping/relocation mismatch"
        );
        let mut changed = original.to_vec();
        for plan in story_fonts {
            changed = plan.relocate_index(&changed)?;
        }
        if !menu_changes.is_empty() {
            let layout: Value =
                serde_json::from_slice(&fs::read(root.join("config/menu-build.json"))?)?;
            changed = crate::archive_index::relocate(
                &changed,
                menu_changes,
                layout["game_size"].as_u64().context("GAME size")?,
            )?;
        }
        // Authorize only individual offset/size words that differ, never the table.
        for (i, (old, new)) in original
            .as_chunks::<4>()
            .0
            .iter()
            .zip(changed.as_chunks::<4>().0.iter())
            .enumerate()
        {
            if old != new {
                let at = offset + i * 4;
                ensure!(at + 4 <= site || at >= site + 4, "code/index write overlap");
                data_writes.push(at..at + 4);
            }
        }
        after[offset..offset + size].copy_from_slice(&changed);
        index_receipt = json!({"address":address,"file_offset":offset,"size":size,
            "before_sha256":sha256(original),"after_sha256":sha256(&changed),
            "changed_words":data_writes.iter().map(|r|json!({"file_offset":r.start,
                "before":word(&plain,r.start).unwrap(),"after":word(&after,r.start).unwrap()})).collect::<Vec<_>>(),
            "relocation_overlap_count":0});
    }
    let string_plan = super::strings::plan(root, &config, &plain, &headers, base, &relocations)?;
    if let Some(plan) = &string_plan {
        for (range, bytes) in &plan.writes {
            ensure!(
                range.end <= site || range.start >= site + 4,
                "code/string write overlap"
            );
            ensure!(
                data_writes
                    .iter()
                    .all(|r| r.end <= range.start || r.start >= range.end),
                "index/string write overlap"
            );
            after[range.clone()].copy_from_slice(bytes);
        }
        data_writes.extend(plan.writes.iter().map(|(r, _)| r.clone()));
    }
    ensure!(
        after
            .iter()
            .zip(&plain)
            .enumerate()
            .all(|(i, (a, b))| a == b
                || (site..site + 4).contains(&i)
                || data_writes.iter().any(|r| r.contains(&i))),
        "unexplained ELF diff"
    );
    let patched_elf_sha256 = sha256(&after);
    ensure!(
        after.len() <= before.len(),
        "plain EBOOT exceeds original ISO extent"
    );
    after.resize(before.len(), 0);
    ensure!(
        after[plain.len()..].iter().all(|b| *b == 0),
        "unexpected ELF tail"
    );

    // The correction applies to every FNT consumer, so check the complete pinned corpus.
    let catalog_raw = fs::read(root.join("config/text-resources.json"))?;
    let catalog: crate::text::Catalog = serde_json::from_slice(&catalog_raw)?;
    ensure!(
        catalog.source_sha256 == profile.sha256 && catalog.pairs.len() == 96,
        "font corpus drift"
    );
    let mut fonts = Vec::new();
    for pair in &catalog.pairs {
        let b = source::read_extent(iso, pair.font.offset, pair.font.size)?;
        ensure!(
            sha256(&b) == pair.font.sha256 && bytes(&b, 0, 4)? == b"FNT\0",
            "font identity mismatch"
        );
        let count = word(&b, 12)? as usize;
        let start = 16usize
            .checked_add(count.checked_mul(4).context("font count overflow")?)
            .context("font header overflow")?;
        let length = b
            .len()
            .checked_sub(start)
            .context("font payload underflow")?;
        let gim = bytes(&b, start, length)?;
        ensure!(
            length.is_multiple_of(4)
                && bytes(gim, 0, 12)? == b"MIG.00.1PSP\0"
                && word(gim, 16)? == 2
                && word(gim, 20)? as usize + 16 == length,
            "font copy boundary is not the complete GIM"
        );
        fonts.push(json!({"name":pair.name,"size":b.len(),"glyphs":count,"gim_offset":start,"copy_length":length}));
    }
    let receipt = json!({"scope":"plain ELF with bounded FNT copy correction and optional paired GAME index relocation; runtime and real hardware compatibility pending",
        "game_index":index_receipt,
        "strings":string_plan.as_ref().map(|p| p.receipt.clone()),
        "config_sha256":sha256(&raw),"plain_sha256":sha256(&plain),"patched_elf_sha256":patched_elf_sha256,
        "plain_size":plain.len(),"trailing_zero_bytes":before.len()-plain.len(),"file_offset":site,
        "instruction":instruction,"relocation_entries_checked":relocations.len(),"relocation_overlap_count":0,
        "dependency":dependency,"dependency_lock_sha256":sha256(&fs::read(root.join("Cargo.lock"))?),
        "font_catalog_sha256":sha256(&catalog_raw),"original_font_copy_boundaries":fonts});
    Ok((
        crate::write_plan::ExpectedWrite {
            offset,
            before,
            after,
        },
        receipt,
        VerifiedFontCopy {
            font_extents: {
                let layout: Value =
                    serde_json::from_slice(&fs::read(root.join("config/menu-build.json"))?)?;
                let game = layout["game_iso_offset"].as_u64().context("GAME offset")?;
                menu_changes
                    .iter()
                    .map(|r| (game + u64::from(r.offset), r.size as usize))
                    .collect()
            },
        },
    ))
}
