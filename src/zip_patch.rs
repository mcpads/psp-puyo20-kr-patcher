use anyhow::{Context, Result, ensure};
use flate2::{Compression, write::DeflateEncoder};
use serde::Serialize;
use std::io::{Cursor, Read, Write};
use zip::ZipArchive;

fn slice(b: &[u8], o: usize, n: usize) -> Result<&[u8]> {
    b.get(o..o.checked_add(n).context("range overflow")?)
        .context("truncated ZIP")
}
fn u16le(b: &[u8], o: usize) -> Result<usize> {
    Ok(u16::from_le_bytes(slice(b, o, 2)?.try_into()?) as usize)
}
fn u32le(b: &[u8], o: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(slice(b, o, 4)?.try_into()?) as usize)
}
fn put32(b: &mut [u8], o: usize, n: usize) -> Result<()> {
    b.get_mut(o..o + 4)
        .context("short output field")?
        .copy_from_slice(&u32::try_from(n)?.to_le_bytes());
    Ok(())
}
struct Member {
    name: String,
    offset: usize,
    compressed: usize,
    content: Vec<u8>,
    crc: u32,
}
#[derive(Serialize)]
pub struct ZipReceipt {
    pub members: usize,
    pub original_compressed_size: usize,
    pub replacement_compressed_size: usize,
    pub original_stream_reused: bool,
    pub comment_padding: usize,
    pub deflate_padding: usize,
    pub compressor: String,
    pub recompressed_unchanged_members: Vec<String>,
}

pub fn replace(raw: &[u8], target: &str, replacement: &[u8]) -> Result<(Vec<u8>, ZipReceipt)> {
    let mut archive = ZipArchive::new(Cursor::new(raw))?;
    let mut members = Vec::new();
    let mut names = std::collections::HashSet::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        ensure!(names.insert(f.name().to_string()), "duplicate member");
        ensure!(f.size() <= 128 * 1024 * 1024, "oversized member");
        let mut content = Vec::new();
        f.read_to_end(&mut content)?;
        members.push(Member {
            name: f.name().to_string(),
            offset: f.header_start().try_into()?,
            compressed: f.compressed_size().try_into()?,
            content,
            crc: f.crc32(),
        });
    }
    let t = members
        .iter()
        .find(|m| m.name == target)
        .context("missing target")?;
    ensure!(
        t.content.len() == replacement.len(),
        "member resizing unsupported"
    );
    let mut receipt = ZipReceipt {
        members: members.len(),
        original_compressed_size: t.compressed,
        replacement_compressed_size: t.compressed,
        original_stream_reused: t.content == replacement,
        comment_padding: 0,
        deflate_padding: 0,
        compressor: "original-stream".into(),
        recompressed_unchanged_members: Vec::new(),
    };
    if receipt.original_stream_reused {
        return Ok((raw.to_vec(), receipt));
    }
    let end = raw
        .windows(4)
        .rposition(|v| v == b"PK\x05\x06")
        .context("missing EOCD")?;
    let cdsize = u32le(raw, end + 12)?;
    let cd = u32le(raw, end + 16)?;
    let comment = u16le(raw, end + 20)?;
    ensure!(
        u16le(raw, end + 4)? == 0
            && u16le(raw, end + 6)? == 0
            && u16le(raw, end + 8)? == members.len()
            && u16le(raw, end + 10)? == members.len()
            && end + 22 + comment == raw.len()
            && cd + cdsize == end,
        "unsupported EOCD layout"
    );
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::best());
    enc.write_all(replacement)?;
    let mut encoded = enc.finish()?;
    receipt.compressor = "flate2-best".into();
    if encoded.len() > t.compressed {
        let mut compact = Vec::new();
        zopfli::compress(
            zopfli::Options::default(),
            zopfli::Format::Deflate,
            replacement,
            &mut compact,
        )?;
        if compact.len() < encoded.len() {
            encoded = compact;
            receipt.compressor = "zopfli-0.8.3-default-15".into();
        }
    }
    // Earlier replacements may have parked free space as leading empty stored blocks
    // in a member stream and as trailing zero bytes of the comment. Reclaim both so
    // that every replacement in the same archive sees the whole free capacity.
    const PAD: [u8; 5] = [0, 0, 0, 255, 255];
    let kept_comment = slice(raw, end + 22, comment)?;
    let kept_comment = &kept_comment[..kept_comment
        .iter()
        .rposition(|b| *b != 0)
        .map_or(0, |i| i + 1)];
    let mut order: Vec<_> = members.iter().collect();
    order.sort_by_key(|m| m.offset);
    let mut streams = std::collections::HashMap::new();
    let mut headers = std::collections::HashMap::new();
    let mut old_end = 0;
    for m in &order {
        let off = m.offset;
        ensure!(
            off == old_end && slice(raw, off, 4)? == b"PK\x03\x04",
            "gapped local records"
        );
        ensure!(
            u16le(raw, off + 6)? & 9 == 0 && u16le(raw, off + 8)? == 8,
            "unsupported ZIP flags or method"
        );
        let data = off + 30 + u16le(raw, off + 26)? + u16le(raw, off + 28)?;
        let stop = data + m.compressed;
        ensure!(
            stop <= cd
                && u32le(raw, off + 14)? == m.crc as usize
                && u32le(raw, off + 18)? == m.compressed
                && u32le(raw, off + 22)? == m.content.len(),
            "local/central mismatch"
        );
        let mut stream = slice(raw, data, m.compressed)?;
        if m.name != target {
            while stream.len() > PAD.len() && stream.starts_with(&PAD) {
                stream = &stream[PAD.len()..];
            }
        }
        headers.insert(m.name.as_str(), slice(raw, off, data - off)?);
        streams.insert(m.name.as_str(), stream);
        old_end = stop;
    }
    ensure!(old_end == cd, "unexpected local trailer");
    let assemble = |target_stream: &[u8],
                    optimized: &std::collections::BTreeMap<String, Vec<u8>>|
     -> Result<(Vec<u8>, Vec<u8>)> {
        let mut local = Vec::new();
        let mut offsets = std::collections::HashMap::new();
        let mut sizes = std::collections::HashMap::new();
        for m in &order {
            let stream = if m.name == target {
                target_stream
            } else {
                optimized
                    .get(&m.name)
                    .map_or(streams[m.name.as_str()], Vec::as_slice)
            };
            offsets.insert(m.name.as_str(), local.len());
            sizes.insert(m.name.as_str(), stream.len());
            let mut h = headers[m.name.as_str()].to_vec();
            if m.name == target {
                put32(&mut h, 14, crc32fast::hash(replacement) as usize)?;
            }
            put32(&mut h, 18, stream.len())?;
            local.extend(h);
            local.extend(stream);
        }
        let mut central = Vec::new();
        let mut pos = cd;
        for m in &members {
            ensure!(slice(raw, pos, 4)? == b"PK\x01\x02", "bad central record");
            let n = 46 + u16le(raw, pos + 28)? + u16le(raw, pos + 30)? + u16le(raw, pos + 32)?;
            let mut record = slice(raw, pos, n)?.to_vec();
            put32(&mut record, 42, offsets[m.name.as_str()])?;
            put32(&mut record, 20, sizes[m.name.as_str()])?;
            if m.name == target {
                put32(&mut record, 16, crc32fast::hash(replacement) as usize)?;
            }
            central.extend(record);
            pos += n;
        }
        ensure!(pos == end, "unexpected central trailer");
        Ok((local, central))
    };
    let mut optimized = std::collections::BTreeMap::new();
    let (mut local, mut central) = assemble(&encoded, &optimized)?;
    let mut needed = local.len() + central.len() + 22 + kept_comment.len();
    // Reclaim space without changing any other member's decompressed bytes.
    // Ordinary fits retain their original streams; only a real overflow triggers this.
    if needed > raw.len() {
        for m in &order {
            if m.name == target {
                continue;
            }
            let mut compact = Vec::new();
            zopfli::compress(
                zopfli::Options::default(),
                zopfli::Format::Deflate,
                m.content.as_slice(),
                &mut compact,
            )?;
            if compact.len() < streams[m.name.as_str()].len() {
                optimized.insert(m.name.clone(), compact);
                receipt.recompressed_unchanged_members.push(m.name.clone());
                (local, central) = assemble(&encoded, &optimized)?;
                needed = local.len() + central.len() + 22 + kept_comment.len();
                if needed <= raw.len() {
                    break;
                }
            }
        }
    }
    ensure!(
        needed <= raw.len(),
        "ZIP capacity exceeded by {} bytes replacing {target}",
        needed.saturating_sub(raw.len())
    );
    // Empty, non-final stored blocks preserve byte alignment and emit no data.
    // Keep space beyond the ZIP comment limit inside the target DEFLATE stream.
    let free = raw.len() - needed;
    let excess = (kept_comment.len() + free).saturating_sub(u16::MAX as usize);
    let (mut local, central) = if excess > 0 {
        let blocks = excess.div_ceil(PAD.len());
        let mut padded = Vec::with_capacity(blocks * PAD.len() + encoded.len());
        for _ in 0..blocks {
            padded.extend_from_slice(&PAD);
        }
        padded.extend(&encoded);
        receipt.deflate_padding = blocks * PAD.len();
        encoded = padded;
        assemble(&encoded, &optimized)?
    } else {
        (local, central)
    };
    receipt.replacement_compressed_size = encoded.len();
    let needed = local.len() + central.len() + 22 + kept_comment.len();
    let padding = raw.len() - needed;
    ensure!(
        kept_comment.len() + padding <= u16::MAX as usize,
        "ZIP comment capacity exceeded"
    );
    receipt.comment_padding = padding;
    let comment = kept_comment.len();
    let mut tail = slice(raw, end, 22)?.to_vec();
    put32(&mut tail, 12, central.len())?;
    put32(&mut tail, 16, local.len())?;
    tail[20..22].copy_from_slice(&((comment + padding) as u16).to_le_bytes());
    local.extend(central);
    local.extend(tail);
    local.extend(kept_comment);
    local.resize(raw.len(), 0);
    let mut check = ZipArchive::new(Cursor::new(&local))?;
    ensure!(check.len() == members.len(), "member count changed");
    for (i, m) in members.iter().enumerate() {
        let mut f = check.by_index(i)?;
        ensure!(f.name() == m.name, "member order changed");
        let mut b = Vec::new();
        f.read_to_end(&mut b)?;
        ensure!(
            b == if m.name == target {
                replacement
            } else {
                &m.content
            },
            "member content mismatch"
        );
    }
    Ok((local, receipt))
}
#[cfg(test)]
mod tests;
