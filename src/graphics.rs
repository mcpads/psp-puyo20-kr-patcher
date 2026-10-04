mod dxt;
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
pub fn bytes(b: &[u8], o: usize, n: usize) -> Result<&[u8]> {
    b.get(o..o.checked_add(n).context("range overflow")?)
        .context("asset range exceeded")
}
fn word(b: &[u8], o: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(bytes(b, o, 4)?.try_into()?) as usize)
}
fn half(b: &[u8], o: usize) -> Result<usize> {
    Ok(u16::from_le_bytes(bytes(b, o, 2)?.try_into()?) as usize)
}
fn chunks(
    b: &[u8],
    start: usize,
    end: usize,
    depth: usize,
    out: &mut BTreeMap<usize, (usize, usize)>,
) -> Result<()> {
    ensure!(depth < 8, "GIM nesting too deep");
    let mut p = start;
    while p < end {
        let kind = word(b, p)?;
        let size = word(b, p + 4)?;
        ensure!(
            size >= 16
                && p.checked_add(size).context("chunk overflow")? <= end
                && word(b, p + 12)? == 16,
            "bad GIM chunk"
        );
        if kind == 2 || kind == 3 {
            chunks(b, p + 16, p + size, depth + 1, out)?;
        } else {
            ensure!(
                out.insert(kind, (p, size)).is_none(),
                "multiple GIM planes unsupported"
            );
        }
        p += size;
    }
    Ok(())
}
struct Plane {
    fmt: usize,
    order: usize,
    w: usize,
    h: usize,
    bits: usize,
    start: usize,
    stride: usize,
}
impl Plane {
    fn offset(&self, xbyte: usize, y: usize) -> usize {
        self.start
            + if self.order == 0 {
                y * self.stride + xbyte
            } else {
                ((y / 8) * (self.stride / 16) + xbyte / 16) * 128 + (y % 8) * 16 + xbyte % 16
            }
    }
}
fn plane(b: &[u8], pos: usize, size: usize) -> Result<Plane> {
    let h = pos + 16;
    let p = Plane {
        fmt: half(b, h + 4)?,
        order: half(b, h + 6)?,
        w: half(b, h + 8)?,
        h: half(b, h + 10)?,
        bits: half(b, h + 12)?,
        start: h + word(b, h + 28)?,
        stride: 0,
    };
    ensure!(
        (1..=4096).contains(&p.w) && (1..=4096).contains(&p.h) && p.order <= 1,
        "unsupported geometry"
    );
    ensure!(
        half(b, h + 42)? == 1 && half(b, h + 44)? == 3 && half(b, h + 46)? == 1,
        "multiple frames/levels unsupported"
    );
    let stride = (p.w * p.bits).div_ceil(8).div_ceil(16) * 16;
    let rows = if p.order == 1 {
        p.h.div_ceil(8) * 8
    } else {
        p.h
    };
    let end = h + word(b, h + 32)?;
    ensure!(
        p.start >= h + 48 && end <= pos + size && end >= p.start && end - p.start == stride * rows,
        "invalid pixel extent"
    );
    bytes(b, p.start, end - p.start)?;
    Ok(Plane { stride, ..p })
}
fn layout(b: &[u8]) -> Result<(Plane, Vec<[u8; 4]>)> {
    ensure!(bytes(b, 0, 12)? == b"MIG.00.1PSP\0", "not PSP GIM");
    let mut c = BTreeMap::new();
    chunks(b, 16, b.len(), 0, &mut c)?;
    let (pos, size) = *c.get(&4).context("missing image")?;
    let p = plane(b, pos, size)?;
    ensure!(
        matches!((p.fmt, p.bits), (3, 32) | (4, 4) | (5, 8) | (10, 8)),
        "unsupported image format"
    );
    let mut palette = Vec::new();
    if matches!(p.fmt, 4 | 5) {
        let (pos, size) = *c.get(&5).context("missing palette")?;
        let q = plane(b, pos, size)?;
        ensure!(
            matches!((q.fmt, q.bits), (1, 16) | (2, 16) | (3, 32)) && q.h == 1 && q.order == 0,
            "unsupported palette"
        );
        for i in 0..q.w {
            palette.push(if q.fmt == 3 {
                bytes(b, q.start + 4 * i, 4)?.try_into()?
            } else {
                palette_color(half(b, q.start + 2 * i)? as u16, q.fmt)
            });
        }
    }
    Ok((p, palette))
}
fn palette_color(value: u16, format: usize) -> [u8; 4] {
    if format == 2 {
        std::array::from_fn(|i| (((value >> (i * 4)) & 15) * 17) as u8)
    } else {
        let mut out = [0; 4];
        for (i, channel) in out[..3].iter_mut().enumerate() {
            let v = ((value >> (i * 5)) & 31) as u8;
            *channel = (v << 3) | (v >> 2);
        }
        out[3] = if value & 0x8000 != 0 { 255 } else { 0 };
        out
    }
}
#[derive(Debug)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub format: usize,
    pub order: usize,
    pub rgba: Vec<u8>,
}
pub fn decode(b: &[u8]) -> Result<Image> {
    let (p, pal) = layout(b)?;
    if p.fmt == 10 {
        ensure!(
            p.order == 0 && p.w.is_multiple_of(4) && p.h.is_multiple_of(4) && p.stride == p.w,
            "unsupported DXT5 layout"
        );
        return Ok(Image {
            width: p.w,
            height: p.h,
            format: p.fmt,
            order: p.order,
            rgba: dxt::decode(bytes(b, p.start, p.w * p.h)?, p.w, p.h),
        });
    }
    let mut rgba = Vec::with_capacity(p.w * p.h * 4);
    for y in 0..p.h {
        for x in 0..p.w {
            if p.fmt == 3 {
                rgba.extend(bytes(b, p.offset(x * 4, y), 4)?);
            } else {
                let byte = b[p.offset(if p.fmt == 5 { x } else { x / 2 }, y)];
                let index = if p.fmt == 5 {
                    byte as usize
                } else {
                    ((byte >> (4 * (x % 2))) & 15) as usize
                };
                rgba.extend(pal.get(index).context("palette index overflow")?);
            }
        }
    }
    Ok(Image {
        width: p.w,
        height: p.h,
        format: p.fmt,
        order: p.order,
        rgba,
    })
}

/// Shorten the final authoring-info block without changing the picture subtree.
/// This deliberately accepts only the root/picture/file-info layout observed in FNT.
pub fn compact_file_info(b: &[u8], reclaim: usize) -> Result<Vec<u8>> {
    let before = decode(b)?;
    ensure!(
        reclaim > 0 && reclaim.is_multiple_of(4),
        "unaligned GIM reclaim"
    );
    ensure!(
        word(b, 16)? == 2
            && word(b, 20)? == b.len() - 16
            && word(b, 24)? == 16
            && word(b, 28)? == 16
            && word(b, 32)? == 3
            && word(b, 40)? == 16,
        "unsupported GIM info hierarchy"
    );
    let info = 32 + word(b, 36)?;
    let size = word(b, info + 4)?;
    ensure!(
        word(b, info)? == 0xff
            && info + size == b.len()
            && word(b, info + 8)? == size
            && word(b, info + 12)? == 16,
        "unsupported GIM file info"
    );
    let new_size = size.checked_sub(reclaim).context("GIM info capacity")?;
    // Four NUL-terminated authoring strings; keep the block itself present.
    let metadata = b"puyo20\0\0\0\0";
    ensure!(new_size >= 16 + metadata.len(), "GIM info capacity");
    let mut out = b[..info + new_size].to_vec();
    let root_size = (out.len() - 16) as u32;
    out[20..24].copy_from_slice(&root_size.to_le_bytes());
    out[info + 4..info + 8].copy_from_slice(&(new_size as u32).to_le_bytes());
    out[info + 8..info + 12].copy_from_slice(&(new_size as u32).to_le_bytes());
    out[info + 16..].fill(0);
    out[info + 16..info + 16 + metadata.len()].copy_from_slice(metadata);
    let after = decode(&out)?;
    ensure!(
        before.rgba == after.rgba
            && before.width == after.width
            && before.height == after.height
            && before.format == after.format
            && before.order == after.order,
        "GIM compaction changed image"
    );
    Ok(out)
}
pub fn encode(b: &[u8], width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>> {
    let old = decode(b)?;
    ensure!(
        old.width == width && old.height == height && rgba.len() == old.rgba.len(),
        "RGBA dimensions mismatch"
    );
    let (p, pal) = layout(b)?;
    if p.fmt == 10 {
        ensure!(
            rgba == old.rgba,
            "DXT5 editing requires a separately validated encoder"
        );
        return Ok(b.to_vec());
    }
    let mut out = b.to_vec();
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            let color = &rgba[i..i + 4];
            if color == &old.rgba[i..i + 4] {
                continue;
            }
            if p.fmt == 3 {
                let off = p.offset(x * 4, y);
                out[off..off + 4].copy_from_slice(color);
            } else {
                let index = pal
                    .iter()
                    .position(|v| v == color)
                    .context("color absent from original palette")?;
                ensure!(index < 1 << p.bits, "index exceeds bit depth");
                let off = p.offset(if p.fmt == 5 { x } else { x / 2 }, y);
                if p.fmt == 5 {
                    out[off] = index as u8;
                } else {
                    let shift = 4 * (x % 2);
                    out[off] = (out[off] & !(15 << shift)) | ((index as u8) << shift);
                }
            }
        }
    }
    ensure!(decode(&out)?.rgba == rgba, "encoded pixels differ");
    Ok(out)
}

/// Resize the linear 8-bit font atlas observed in the source FNTs.
/// Palette and authoring metadata are retained; only image/root/picture lengths
/// and image height change. Frame offsets remain relative to their plane header.
pub fn resize_font_atlas(b: &[u8], height: usize) -> Result<Vec<u8>> {
    let before = decode(b)?;
    let (p, _) = layout(b)?;
    ensure!(
        p.fmt == 5 && p.bits == 8 && p.order == 0 && p.w == 512,
        "unsupported font atlas"
    );
    ensure!(
        height > 0 && height <= 512 && height.is_power_of_two(),
        "unsupported font atlas height"
    );
    ensure!(
        word(b, 16)? == 2
            && word(b, 20)? == b.len() - 16
            && word(b, 24)? == 16
            && word(b, 32)? == 3
            && word(b, 40)? == 16,
        "unsupported font hierarchy"
    );
    let mut c = BTreeMap::new();
    chunks(b, 16, b.len(), 0, &mut c)?;
    let (image, size) = *c.get(&4).context("font image")?;
    let h = image + 16;
    let picture_end = 32 + word(b, 36)?;
    ensure!(
        c.len() == 3
            && word(b, 48)? == 5
            && image == 48 + word(b, 52)?
            && image + size == picture_end
            && word(b, image + 8)? == size
            && word(b, h)? == 48
            && word(b, h + 24)? == 48
            && word(b, h + 28)? == 64
            && word(b, h + 48)? == 64
            && p.start + p.stride * p.h == image + size
            && word(b, picture_end)? == 0xff
            && picture_end + word(b, picture_end + 4)? == b.len(),
        "unsupported font plane offsets"
    );
    let old_plane = p.stride * p.h;
    let new_plane = p.stride * height;
    let new_picture_end = picture_end - old_plane + new_plane;
    let mut out = Vec::with_capacity(b.len() - old_plane + new_plane);
    out.extend_from_slice(&b[..p.start + old_plane.min(new_plane)]);
    out.resize(new_picture_end, 0);
    out.extend_from_slice(&b[picture_end..]);
    for field in [20, 36, image + 4, image + 8, h + 32] {
        out[field..field + 4].copy_from_slice(
            &u32::try_from(
                word(b, field)?
                    .checked_sub(old_plane)
                    .context("font plane size")?
                    + new_plane,
            )?
            .to_le_bytes(),
        );
    }
    out[h + 10..h + 12].copy_from_slice(&u16::try_from(height)?.to_le_bytes());
    let after = decode(&out)?;
    ensure!(
        after.height == height
            && after.rgba[..after.rgba.len().min(before.rgba.len())]
                == before.rgba[..after.rgba.len().min(before.rgba.len())]
            && out[48..image] == b[48..image]
            && out[new_picture_end..] == b[picture_end..],
        "font resize changed retained pixels, palette or metadata"
    );
    Ok(out)
}
pub fn snt_table(b: &[u8]) -> Result<Vec<(usize, usize)>> {
    ensure!(bytes(b, 0, 4)? == b"NUIF", "not NUIF");
    let base = word(b, 12)?;
    ensure!(bytes(b, base, 4)? == b"NUTL", "not NUTL");
    let count = word(b, base + 16)?;
    ensure!(count <= 4096, "excessive textures");
    let table = base + word(b, base + 24)?;
    let mut end = table + count * 8;
    let mut rows = Vec::new();
    for i in 0..count {
        let size = word(b, table + i * 8)?;
        let off = base + word(b, table + i * 8 + 4)?;
        ensure!(off >= end, "overlapping texture");
        let g = bytes(b, off, size)?;
        ensure!(g.starts_with(b"MIG.00.1PSP"), "not GIM texture");
        rows.push((off, size));
        end = off + size;
    }
    Ok(rows)
}
/// Original palette for explicit asset preparation; encoding remains exact.
pub fn palette(b: &[u8]) -> Result<Vec<[u8; 4]>> {
    Ok(layout(b)?.1)
}

#[cfg(test)]
mod tests;

/// Lossy preparation only. Product encode remains lossless and rejects DXT edits.
/// Every permitted rectangle must comprise whole blocks; all other bytes and pixels
/// retain their original values. The caller must review the decoded candidate.
pub fn prepare_dxt(b: &[u8], rgba: &[u8], rects: &[[usize; 4]]) -> Result<Vec<u8>> {
    let original = decode(b)?;
    let (p, _) = layout(b)?;
    ensure!(p.fmt == 10, "DXT5 source required");
    ensure!(
        rgba.len() == original.rgba.len(),
        "RGBA dimensions mismatch"
    );
    let selected = dxt_blocks(&p, rects)?;
    let mut out = b.to_vec();
    for (slot, &edit) in selected.iter().enumerate() {
        let bx = slot % (p.w / 4) * 4;
        let by = slot / (p.w / 4) * 4;
        let mut pixels = [[0; 4]; 16];
        for y in 0..4 {
            for x in 0..4 {
                let pos = ((by + y) * p.w + bx + x) * 4;
                if !edit {
                    ensure!(
                        rgba[pos..pos + 4] == original.rgba[pos..pos + 4],
                        "draft changes protected DXT pixel"
                    );
                }
                pixels[y * 4 + x].copy_from_slice(&rgba[pos..pos + 4]);
            }
        }
        if edit {
            let pos = p.start + slot * 16;
            out[pos..pos + 16].copy_from_slice(&dxt::compress_block(pixels));
        }
    }
    Ok(out)
}

fn dxt_blocks(p: &Plane, rects: &[[usize; 4]]) -> Result<Vec<bool>> {
    ensure!(!rects.is_empty(), "empty DXT block selection");
    let mut selected = vec![false; p.w * p.h / 16];
    for &[x, y, w, h] in rects {
        ensure!(
            w > 0
                && h > 0
                && [x, y, w, h].iter().all(|v| v % 4 == 0)
                && x.checked_add(w).is_some_and(|end| end <= p.w)
                && y.checked_add(h).is_some_and(|end| end <= p.h),
            "DXT rectangle must be in bounds and aligned to whole 4x4 blocks"
        );
        for by in y / 4..(y + h) / 4 {
            for bx in x / 4..(x + w) / 4 {
                let slot = by * (p.w / 4) + bx;
                ensure!(!selected[slot], "overlapping DXT block writers");
                selected[slot] = true;
            }
        }
    }
    Ok(selected)
}

/// Adopt an already reviewed compressed texture without recompressing any pixels.
pub fn validate_prepared_dxt(
    source: &[u8],
    candidate: &[u8],
    rects: &[[usize; 4]],
) -> Result<Image> {
    let before = decode(source)?;
    let after = decode(candidate)?;
    let (p, _) = layout(source)?;
    ensure!(
        p.fmt == 10 && after.format == 10,
        "DXT5 source and candidate required"
    );
    ensure!(
        source.len() == candidate.len()
            && before.width == after.width
            && before.height == after.height,
        "prepared DXT geometry or length mismatch"
    );
    let end = p.start + p.w * p.h;
    ensure!(
        source[..p.start] == candidate[..p.start] && source[end..] == candidate[end..],
        "prepared DXT changed protected metadata"
    );
    let selected = dxt_blocks(&p, rects)?;
    for (slot, edit) in selected.into_iter().enumerate() {
        if !edit {
            let pos = p.start + slot * 16;
            ensure!(
                source[pos..pos + 16] == candidate[pos..pos + 16],
                "prepared DXT changed protected block"
            );
        }
    }
    Ok(after)
}
