use crate::{authoring, graphics, sha256};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

mod boundary;
mod pattern;

// Compare premultiplied colors so invisible RGB cannot dominate alpha edges.
fn distance(a: [u8; 4], b: [u8; 4]) -> u64 {
    let mut d = (i64::from(a[3]) - i64::from(b[3])).pow(2) * 255 * 255;
    for i in 0..3 {
        d += (i64::from(a[i]) * i64::from(a[3]) - i64::from(b[i]) * i64::from(b[3])).pow(2);
    }
    d as u64
}

fn palette_color(pixel: [u8; 4], palette: &[[u8; 4]], match_alpha: bool) -> [u8; 4] {
    palette
        .iter()
        .min_by_key(|p| {
            (
                if match_alpha {
                    pixel[3].abs_diff(p[3])
                } else {
                    0
                },
                distance(pixel, **p),
            )
        })
        .copied()
        .unwrap_or(pixel)
}

fn restrict_palette(original: Vec<[u8; 4]>, selected: &[[u8; 4]]) -> Result<Vec<[u8; 4]>> {
    if selected.is_empty() {
        return Ok(original);
    }
    ensure!(
        selected.iter().all(|color| original.contains(color)),
        "selected backing color is absent from the original palette"
    );
    Ok(selected.to_vec())
}

fn visible_bounds(src: &graphics::Image) -> Result<(usize, usize, usize, usize)> {
    let (mut left, mut top, mut right, mut bottom) = (src.width, src.height, 0, 0);
    for (i, p) in src.rgba.as_chunks::<4>().0.iter().enumerate() {
        if p[3] >= 16 {
            let (x, y) = (i % src.width, i / src.width);
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    ensure!(right > left && bottom > top, "empty visible image");
    Ok((left, top, right - left, bottom - top))
}

#[derive(Default)]
pub struct RegionOptions {
    pub trim_alpha: bool,
    pub trim_padding: Option<usize>,
    pub palette_colors: Option<usize>,
    /// Use only these verified original palette colors for a text-free backing.
    pub palette_rgba: Vec<[u8; 4]>,
    /// Align a generated two-color pattern using repeated original background observations.
    pub pattern_guide: Option<std::path::PathBuf>,
    /// Prefer the closest representable alpha before matching RGB.
    pub match_alpha: bool,
    /// Match an already text-free opaque backing to the unchanged protected boundary.
    pub match_backing_boundary: bool,
    pub left_align: bool,
    pub source_rect: Option<[usize; 4]>,
    pub protect: Option<Protect>,
    /// `[low, high]`: pixels whose brightest channel is at most `low` become transparent and
    /// alpha ramps up to `high`, undoing the blend with black. For drafts generated on a
    /// black ground; applied before alpha trimming.
    pub key_black: Option<[u8; 2]>,
    /// After keying, clear 8-connected visible pieces smaller than a tenth of the largest
    /// (fragments of neighbouring drawings caught by the source rectangle).
    pub drop_specks: bool,
}

fn drop_specks(img: &mut graphics::Image) {
    let (w, h) = (img.width, img.height);
    let mut label = vec![0usize; w * h];
    let mut sizes = vec![0usize];
    for start in 0..w * h {
        if label[start] != 0 || img.rgba[start * 4 + 3] == 0 {
            continue;
        }
        let id = sizes.len();
        let (mut stack, mut size) = (vec![start], 0);
        label[start] = id;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if label[j] == 0 && img.rgba[j * 4 + 3] != 0 {
                        label[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        sizes.push(size);
    }
    let largest = sizes.iter().copied().max().unwrap_or(0);
    for (i, &l) in label.iter().enumerate() {
        if l != 0 && sizes[l] * 10 < largest {
            img.rgba[i * 4..i * 4 + 4].fill(0);
        }
    }
}

fn key_black(img: &mut graphics::Image, [low, high]: [u8; 2]) -> Result<()> {
    ensure!(low < high, "key-black needs low < high");
    for p in img.rgba.as_chunks_mut::<4>().0 {
        let m = p[0].max(p[1]).max(p[2]);
        if m <= low {
            *p = [0; 4];
        } else if m < high {
            let f = f64::from(m - low) / f64::from(high - low);
            for c in &mut p[..3] {
                *c = (f64::from(*c) / f).round().min(255.0) as u8;
            }
            p[3] = (f64::from(p[3]) * f).round() as u8;
        }
    }
    Ok(())
}

/// Keep the original cell outside the edited rectangles. `base` is cropped at `origin` to
/// the output size; prepared pixels are copied only inside `rects`, or inside the bounding
/// box where the draft differs from the base when `rects` is empty. Palette fitting may
/// change transparent RGB values, so this keeps protected pixels byte-identical.
pub struct Protect {
    pub base: std::path::PathBuf,
    pub origin: [usize; 2],
    pub rects: Vec<[usize; 4]>,
}

fn crop_at(
    src: &graphics::Image,
    [x, y]: [usize; 2],
    w: usize,
    h: usize,
) -> Result<graphics::Image> {
    ensure!(
        x + w <= src.width && y + h <= src.height,
        "protected base outside image"
    );
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        rgba.extend_from_slice(&src.rgba[(row * src.width + x) * 4..][..w * 4]);
    }
    Ok(graphics::Image {
        width: w,
        height: h,
        format: 3,
        order: 0,
        rgba,
    })
}

fn diff_bounds(a: &graphics::Image, b: &graphics::Image) -> Result<[usize; 4]> {
    ensure!(
        a.width == b.width && a.height == b.height,
        "draft and base size differ"
    );
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..a.height {
        for x in 0..a.width {
            let i = (y * a.width + x) * 4;
            if a.rgba[i..i + 4] != b.rgba[i..i + 4] {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
    }
    ensure!(x0 != usize::MAX, "draft does not differ from base");
    Ok([x0, y0, x1 - x0, y1 - y0])
}

fn crop(src: graphics::Image, rect: Option<[usize; 4]>) -> Result<graphics::Image> {
    let Some([x, y, w, h]) = rect else {
        return Ok(src);
    };
    ensure!(
        w > 0
            && h > 0
            && x.checked_add(w).is_some_and(|v| v <= src.width)
            && y.checked_add(h).is_some_and(|v| v <= src.height),
        "source rectangle outside image"
    );
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        let start = (row * src.width + x) * 4;
        rgba.extend_from_slice(&src.rgba[start..start + w * 4]);
    }
    Ok(graphics::Image {
        width: w,
        height: h,
        rgba,
        ..src
    })
}

pub fn region(
    image: &Path,
    gim: &Path,
    [w, h]: [usize; 2],
    options: RegionOptions,
    output: &Path,
) -> Result<Value> {
    let RegionOptions {
        trim_alpha,
        trim_padding,
        palette_colors,
        palette_rgba,
        pattern_guide,
        match_alpha,
        match_backing_boundary,
        left_align,
        source_rect,
        protect,
        key_black: key,
        drop_specks: specks,
    } = options;
    ensure!(!output.exists(), "output exists");
    ensure!(
        trim_alpha || trim_padding.is_none(),
        "trim padding needs alpha trimming"
    );
    let padding = trim_padding.unwrap_or(2);
    let border = padding
        .checked_mul(2)
        .ok_or_else(|| anyhow::anyhow!("trim padding overflow"))?;
    ensure!(
        !match_backing_boundary || (protect.is_some() && palette_colors.is_none()),
        "backing correction needs protection and no color-count reduction"
    );
    ensure!(
        !match_alpha || palette_colors.is_none(),
        "alpha matching cannot be combined with color-count reduction"
    );
    ensure!(
        !left_align || trim_alpha,
        "left alignment requires alpha trimming"
    );
    let mut src = crop(authoring::png_read(image)?, source_rect)?;
    if let Some(k) = key {
        key_black(&mut src, k)?;
    }
    if specks {
        drop_specks(&mut src);
    }
    ensure!(
        w > 0 && h > 0 && w <= src.width && h <= src.height,
        "only bounded downsampling supported"
    );
    let original = fs::read(gim)?;
    ensure!(
        palette_rgba.is_empty() || palette_colors.is_none(),
        "explicit palette cannot be combined with color-count reduction"
    );
    let palette = restrict_palette(graphics::palette(&original)?, &palette_rgba)?;
    let direct_rgba = palette.is_empty();
    ensure!(
        !direct_rgba || matches!(graphics::decode(&original)?.format, 3 | 10),
        "unsupported non-indexed GIM"
    );
    let (left, top, cw, ch) = if trim_alpha {
        ensure!(w > border && h > border, "insufficient title canvas");
        visible_bounds(&src)?
    } else {
        (0, 0, src.width, src.height)
    };
    let (rw, rh) = if trim_alpha {
        if cw * (h - border) > ch * (w - border) {
            (w - border, (ch * (w - border) / cw).max(1))
        } else {
            ((cw * (h - border) / ch).max(1), h - border)
        }
    } else {
        (w, h)
    };
    ensure!(rw <= cw && rh <= ch, "only downsampling supported");
    let clear = palette
        .iter()
        .min_by_key(|p| p[3])
        .copied()
        .unwrap_or([0; 4]);
    ensure!(
        !trim_alpha || clear[3] == 0,
        "transparent palette entry required"
    );
    let mut rgba = clear.repeat(w * h);
    let mut error = 0u64;
    for y in 0..rh {
        for x in 0..rw {
            let mut sum = [0u64; 4];
            let mut count = 0;
            for sy in top + y * ch / rh..top + (y + 1) * ch / rh {
                for sx in left + x * cw / rw..left + (x + 1) * cw / rw {
                    let p = &src.rgba[(sy * src.width + sx) * 4..][..4];
                    for i in 0..3 {
                        sum[i] += u64::from(p[i]) * u64::from(p[3]);
                    }
                    sum[3] += u64::from(p[3]);
                    count += 1;
                }
            }
            let mut pixel = [0u8; 4];
            for i in 0..3 {
                pixel[i] = (sum[i] + sum[3] / 2).checked_div(sum[3]).unwrap_or(0) as u8;
            }
            pixel[3] = ((sum[3] + count / 2) / count) as u8;
            let color = palette_color(pixel, &palette, match_alpha);
            error += distance(pixel, color);
            let dx = x + if left_align { padding } else { (w - rw) / 2 };
            let dy = y + (h - rh) / 2;
            rgba[(dy * w + dx) * 4..][..4].copy_from_slice(&color);
        }
    }
    if let Some(limit) = palette_colors {
        reduce_colors(&mut rgba, limit)?;
    }
    let result = graphics::Image {
        width: w,
        height: h,
        format: 3,
        order: 0,
        rgba,
    };
    let mut result = result;
    let pattern_correction = pattern_guide
        .as_ref()
        .map(|path| {
            ensure!(
                protect.is_some() && !trim_alpha,
                "pattern guide needs an untrimmed protected image"
            );
            pattern::align(&mut result, &graphics::decode(&original)?, &palette, path)
        })
        .transpose()?;
    let mut allowed = None;
    let mut backing_correction = None;
    if let Some(Protect {
        base,
        origin,
        rects,
    }) = &protect
    {
        ensure!(!trim_alpha, "protection needs an untrimmed draft");
        let base = crop_at(&authoring::png_read(base)?, *origin, w, h)?;
        let rects = if rects.is_empty() {
            vec![diff_bounds(&src, &base)?]
        } else {
            rects.clone()
        };
        if match_backing_boundary {
            backing_correction = Some(boundary::match_backing(
                &mut result,
                &base,
                &rects,
                &palette,
            )?);
        }
        let mut merged = base.rgba.clone();
        for &[x, y, rw, rh] in &rects {
            ensure!(x + rw <= w && y + rh <= h, "allowed rectangle outside cell");
            for row in y..y + rh {
                let i = (row * w + x) * 4;
                merged[i..i + rw * 4].copy_from_slice(&result.rgba[i..i + rw * 4]);
            }
        }
        result.rgba = merged;
        allowed = Some(rects);
    }
    fs::create_dir_all(output)?;
    authoring::png_write(&output.join("region.png"), &result)?;
    let receipt = json!({"scope":"draft preparation; visual review required", "method":"integer-box-premultiplied; nearest original RGBA palette; no dithering", "match_alpha_first":match_alpha,"palette_color_limit":palette_colors,"source_png_sha256":sha256(&fs::read(image)?), "gim_sha256":sha256(&original), "output_rgba_sha256":sha256(&result.rgba), "size":[w,h], "trim_alpha":trim_alpha, "trim_bounds_alpha_threshold":if trim_alpha {16} else {0}, "source_crop":[left,top,cw,ch], "fitted_size":[rw,rh], "palette_entries":palette.len(), "summed_premultiplied_squared_error":error});
    let mut receipt = receipt;
    receipt["selected_palette_rgba"] = json!(palette_rgba);
    if let Some(correction) = pattern_correction {
        receipt["pattern_correction"] = correction;
    }
    receipt["trim_padding"] = if trim_alpha {
        json!(padding)
    } else {
        Value::Null
    };
    if let Some(correction) = backing_correction {
        receipt["backing_correction"] = correction;
    }
    if direct_rgba {
        receipt["method"] = json!("integer-box-premultiplied; direct RGBA; no dithering");
    }
    if graphics::decode(&original)?.format == 10 {
        receipt["encoding_status"] =
            json!("uncompressed draft; DXT5 encoding and insertion not performed");
    }
    receipt["source_rect"] = json!(source_rect);
    receipt["source_crop"] = json!([
        left + source_rect.map_or(0, |r| r[0]),
        top + source_rect.map_or(0, |r| r[1]),
        cw,
        ch
    ]);
    receipt["left_align"] = json!(left_align);
    if let Some(k) = key {
        receipt["key_black"] = json!(k);
    }
    if specks {
        receipt["drop_specks"] = json!(true);
    }
    if let (Some(p), Some(rects)) = (&protect, allowed) {
        receipt["allowed_rects"] = json!(rects);
        receipt["protected_base_sha256"] = json!(sha256(&fs::read(&p.base)?));
        receipt["protected_base_origin"] = json!(p.origin);
        receipt["output_rgba_sha256"] = json!(sha256(&result.rgba));
    }
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}

fn reduce_colors(rgba: &mut [u8], limit: usize) -> Result<()> {
    ensure!(
        (2..=256).contains(&limit),
        "draft color limit must be 2..256"
    );
    let mut counts = std::collections::BTreeMap::<[u8; 4], usize>::new();
    for p in rgba.as_chunks::<4>().0 {
        *counts.entry(*p).or_default() += 1;
    }
    let mut ranked: Vec<_> = counts.into_iter().collect();
    ranked.sort_by_key(|(color, count)| (std::cmp::Reverse(*count), *color));
    let palette: Vec<_> = ranked
        .into_iter()
        .take(limit)
        .map(|(color, _)| color)
        .collect();
    for p in rgba.as_chunks_mut::<4>().0 {
        let color = palette.iter().min_by_key(|c| distance(*p, **c)).unwrap();
        *p = *color;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

/// Freeze original, draft, and lossy result separately; never label it an insertion.
pub fn dxt(manifest: &Path, output: &Path) -> Result<Value> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        gim: std::path::PathBuf,
        gim_sha256: String,
        png: std::path::PathBuf,
        png_sha256: String,
        allowed_rects: Vec<[usize; 4]>,
        #[serde(default)]
        alpha_levels: Option<usize>,
        #[serde(default)]
        rgb_palette: Option<Vec<[u8; 3]>>,
    }
    ensure!(!output.exists(), "output exists");
    let raw = fs::read(manifest)?;
    let input: Input = serde_json::from_slice(&raw)?;
    let base = manifest.parent().unwrap_or(Path::new("."));
    let original = fs::read(base.join(&input.gim))?;
    ensure!(
        sha256(&original) == input.gim_sha256,
        "DXT source identity mismatch"
    );
    let png = base.join(&input.png);
    ensure!(
        sha256(&fs::read(&png)?) == input.png_sha256,
        "DXT draft identity mismatch"
    );
    let draft = authoring::png_read(&png)?;
    let old = graphics::decode(&original)?;
    ensure!(
        (draft.width, draft.height) == (old.width, old.height),
        "DXT draft geometry mismatch"
    );
    // Validate bounds and unchanged protected pixels before applying any color reduction.
    let mut candidate = graphics::prepare_dxt(&original, &draft.rgba, &input.allowed_rects)?;
    let mut fitted = draft.rgba.clone();
    if let Some(palette) = &input.rgb_palette {
        ensure!(
            (2..=256).contains(&palette.len()),
            "explicit RGB palette has invalid size"
        );
        for &[x, y, w, h] in &input.allowed_rects {
            for row in y..y + h {
                for col in x..x + w {
                    let pos = (row * draft.width + col) * 4;
                    if fitted[pos + 3] != 0 {
                        let nearest = palette
                            .iter()
                            .min_by_key(|p| {
                                (0..3)
                                    .map(|c| (i32::from(p[c]) - i32::from(fitted[pos + c])).pow(2))
                                    .sum::<i32>()
                            })
                            .unwrap();
                        fitted[pos..pos + 3].copy_from_slice(nearest);
                    }
                }
            }
        }
        candidate = graphics::prepare_dxt(&original, &fitted, &input.allowed_rects)?;
    }
    if let Some(levels) = input.alpha_levels {
        ensure!(
            (2..=256).contains(&levels),
            "DXT alpha levels must be 2..256"
        );
        for &[x, y, w, h] in &input.allowed_rects {
            for row in y..y + h {
                for col in x..x + w {
                    let pos = (row * draft.width + col) * 4 + 3;
                    let step = (usize::from(fitted[pos]) * (levels - 1) + 127) / 255;
                    fitted[pos] = ((step * 255 + (levels - 1) / 2) / (levels - 1)) as u8;
                }
            }
        }
        candidate = graphics::prepare_dxt(&original, &fitted, &input.allowed_rects)?;
    }
    let decoded = graphics::decode(&candidate)?;
    let mut squared_error = [0u64; 4];
    let mut max_error = [0u8; 4];
    for (a, b) in draft
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(decoded.rgba.as_chunks::<4>().0)
    {
        // Compare RGB after alpha blending onto black; separately measure alpha.
        for c in 0..4 {
            let av = if c == 3 {
                u16::from(a[c])
            } else {
                u16::from(a[c]) * u16::from(a[3]) / 255
            };
            let bv = if c == 3 {
                u16::from(b[c])
            } else {
                u16::from(b[c]) * u16::from(b[3]) / 255
            };
            let d = av.abs_diff(bv) as u8;
            max_error[c] = max_error[c].max(d);
            squared_error[c] += u64::from(d).pow(2);
        }
    }
    fs::create_dir_all(output)?;
    fs::write(output.join("candidate.gim"), &candidate)?;
    authoring::png_write(&output.join("decoded.png"), &decoded)?;
    let report = json!({
        "scope":"lossy DXT5 preparation; visual review and product adoption pending",
        "encoder":"texpresso 2.0.2 BC3 IterativeClusterFit; perceptual weights; alpha weighted; PSP block reorder",
        "rgb_palette":input.rgb_palette, "alpha_levels":input.alpha_levels, "color_reduction":"optional explicit RGB palette nearest Euclidean; optional uniform alpha levels",
        "manifest_sha256":sha256(&raw), "source_gim_sha256":input.gim_sha256,
        "draft_png_sha256":input.png_sha256, "candidate_gim_sha256":sha256(&candidate),
        "decoded_rgba_sha256":sha256(&decoded.rgba), "size":[old.width,old.height],
        "allowed_rects":input.allowed_rects, "protected_blocks_preserved":true,
        "premultiplied_rgb_and_alpha_max_error":max_error,
        "premultiplied_rgb_and_alpha_squared_error":squared_error,
        "runtime_performed":false
    });
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}
