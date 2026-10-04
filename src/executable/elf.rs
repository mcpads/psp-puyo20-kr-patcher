//! PSP ELF segments and relocation addresses.
use super::{half, word};
use crate::graphics::bytes;
use anyhow::{Context, Result, ensure};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct Program {
    pub(super) kind: u32,
    pub(super) offset: u32,
    pub(super) address: u32,
    pub(super) file_size: u32,
    pub(super) memory_size: u32,
    pub(super) flags: u32,
    pub(super) alignment: u32,
}
pub(super) fn programs(b: &[u8]) -> Result<Vec<Program>> {
    ensure!(
        bytes(b, 0, 7)? == b"\x7fELF\x01\x01\x01"
            && half(b, 16)? == 0xffa0
            && half(b, 18)? == 8
            && word(b, 20)? == 1
            && half(b, 40)? == 52
            && half(b, 42)? == 32,
        "unsupported PSP ELF header"
    );
    let start = word(b, 28)? as usize;
    let count = half(b, 44)? as usize;
    ensure!((1..=32).contains(&count), "invalid ELF program count");
    bytes(b, start, count * 32)?;
    let mut out = Vec::new();
    for i in 0..count {
        let p = start + i * 32;
        let s = Program {
            kind: word(b, p)?,
            offset: word(b, p + 4)?,
            address: word(b, p + 8)?,
            file_size: word(b, p + 16)?,
            memory_size: word(b, p + 20)?,
            flags: word(b, p + 24)?,
            alignment: word(b, p + 28)?,
        };
        bytes(b, s.offset as usize, s.file_size as usize)?;
        if s.kind == 1 {
            ensure!(
                s.file_size <= s.memory_size,
                "ELF segment exceeds memory span"
            );
            s.address
                .checked_add(s.memory_size)
                .context("ELF address overflow")?;
        }
        out.push(s);
    }
    Ok(out)
}
pub(super) fn map_address(
    programs: &[Program],
    base: u32,
    address: u32,
    size: usize,
) -> Result<usize> {
    let relative = address
        .checked_sub(base)
        .context("address below ELF base")?;
    let end = relative
        .checked_add(u32::try_from(size)?)
        .context("mapping overflow")?;
    let mut matches = Vec::new();
    for p in programs.iter().filter(|p| p.kind == 1) {
        if relative >= p.address
            && end
                <= p.address
                    .checked_add(p.file_size)
                    .context("file span overflow")?
        {
            matches.push((p.offset + relative - p.address) as usize);
        }
    }
    ensure!(
        matches.len() == 1,
        "address must map to exactly one file-backed ELF segment"
    );
    Ok(matches[0])
}

pub(super) fn relocation_addresses(
    plain: &[u8],
    headers: &[Program],
    base: u32,
) -> Result<Vec<u32>> {
    let mut relocations = Vec::new();
    for p in headers.iter().filter(|p| p.kind == 0x700000a0) {
        ensure!(
            p.file_size.is_multiple_of(8),
            "truncated PSP relocation entries"
        );
        for entry in bytes(plain, p.offset as usize, p.file_size as usize)?
            .as_chunks::<8>()
            .0
        {
            let offset = word(entry, 0)?;
            let info = word(entry, 4)?;
            let index = ((info >> 8) & 255) as usize;
            let segment = headers.get(index).context("relocation segment")?;
            ensure!(segment.kind == 1, "relocation in non-load segment");
            let address = base
                .checked_add(segment.address)
                .and_then(|a| a.checked_add(offset))
                .context("relocation overflow")?;
            map_address(headers, base, address, 4)?;
            relocations.push(address);
        }
    }
    ensure!(
        !relocations.is_empty() && headers.iter().all(|p| p.kind != 0x700000a1),
        "unsupported relocation representation"
    );
    Ok(relocations)
}
