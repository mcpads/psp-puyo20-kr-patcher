//! Outlined display lettering drafts at exact cell size (RGBA, before palette fit).
//!
//! Spec: `{"font", "items": [{"id", "text", "size": [w,h], "output", "max_px", "min_px",
//! "fill", "outline", "outline_px"?, "outer"?, "outer_px"?, "shadow"?, "shadow_color"?,
//! "align"?, "margin"?, "tracking"?, "round_outline"?, "rasterizer"?, "text_rect"?, "font"?, "base": {"png", "origin"}?,
//! "keep_base"? | "row_interpolate"? | "erase_circle"? | "erase_color"? |
//! "row_extend_column"?}]}`. Items with `plates` build word plates instead (see [`plates`]). The largest size in `[min_px, max_px]` whose outlined ink fits
//! the text rectangle is chosen. With `base` the cell starts from that crop and only the
//! text rectangle is cleared (or refilled) before drawing; a base may be an earlier item's
//! output. Blending defaults to the original Pillow paste rules; `composite:
//! "source_over"` preserves translucent backings when drawing new lettering.
use crate::{authoring, graphics, sha256};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct Mask {
    pub w: usize,
    pub h: usize,
    pub a: Vec<u8>,
}

impl Mask {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            a: vec![0; w * h],
        }
    }
}

fn div255(x: u32) -> u8 {
    let t = x + 128;
    ((t + (t >> 8)) >> 8) as u8
}

/// Pillow `BLEND`: `out = in1 * (255 - m) + in2 * m`, rounded by DIV255.
pub(crate) fn blend(out: u8, ink: u8, m: u8) -> u8 {
    div255(u32::from(out) * (255 - u32::from(m)) + u32::from(ink) * u32::from(m))
}

pub(crate) struct Placed {
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    cov: Vec<u8>,
}

/// Glyph bitmaps relative to the ascender line at pen origin 0, plus the Pillow-style
/// bounding box (horizontal: 0..advance, vertical: ink extent).
pub(crate) fn layout(
    font: &fontdue::Font,
    px: f32,
    text: &str,
    tracking: f32,
    hint: Option<&[u8]>,
) -> Result<(Vec<Placed>, [i32; 4])> {
    // Auto-hinted coverage replaces fontdue's; advances stay fontdue's so layouts keep
    // their widths.
    let hinted = hint
        .map(|data| crate::hinting::Hinted::new(data, px))
        .transpose()?;
    let margin = (px.ceil() as i32) * 2;
    let canvas = (4 * margin) as usize;
    let ascent = font
        .horizontal_line_metrics(px)
        .context("font line metrics")?
        .ascent
        .ceil() as i32;
    let mut pen = 0f32;
    let mut glyphs = Vec::new();
    let (mut l, mut t, mut r, mut b) = (0i32, i32::MAX, 0i32, i32::MIN);
    let count = text.chars().count();
    for (i, c) in text.chars().enumerate() {
        ensure!(
            font.lookup_glyph_index(c) != 0 || c == ' ',
            "missing font glyph {c}"
        );
        let (m, cov) = font.rasterize(c, px);
        let (mut x, mut y) = (
            (pen + m.xmin as f32).round() as i32,
            ascent - (m.ymin + m.height as i32),
        );
        let (mut m, mut cov) = (m, cov);
        if let Some(h) = &hinted {
            match h.placed(c, margin, ascent + margin, (canvas, canvas))? {
                Some((hx, hy, hw, hh, hc)) => {
                    ensure!(
                        hx > 0 && hy > 0 && hx as usize + hw < canvas && hy as usize + hh < canvas,
                        "hinted glyph outside canvas {c}"
                    );
                    (x, y, m.width, m.height, cov) =
                        (hx - margin + pen.round() as i32, hy - margin, hw, hh, hc);
                }
                None => m.width = 0,
            }
        }
        if m.width > 0 && m.height > 0 {
            l = l.min(x);
            r = r.max(x + m.width as i32);
            t = t.min(y);
            b = b.max(y + m.height as i32);
            glyphs.push(Placed {
                x,
                y,
                w: m.width,
                h: m.height,
                cov,
            });
        }
        pen += m.advance_width;
        if i + 1 < count {
            pen += tracking;
        }
    }
    if glyphs.is_empty() {
        // Erase-only items: Pillow reports an empty box for empty text.
        (t, b) = (0, 0);
    }
    r = r.max(pen.round() as i32);
    Ok((glyphs, [l, t, r, b]))
}

pub(crate) fn draw_glyphs(mask: &mut Mask, glyphs: &[Placed], ox: i32, oy: i32) {
    for g in glyphs {
        for gy in 0..g.h {
            for gx in 0..g.w {
                let (x, y) = (ox + g.x + gx as i32, oy + g.y + gy as i32);
                if x < 0 || y < 0 || x as usize >= mask.w || y as usize >= mask.h {
                    continue;
                }
                let i = y as usize * mask.w + x as usize;
                mask.a[i] = blend(mask.a[i], 255, g.cov[gy * g.w + gx]);
            }
        }
    }
}

/// Repeated 3x3 maximum filter (Pillow `MaxFilter(3)`), edges clamped.
/// `round` alternates a plus-shaped step with the square one, an octagonal approximation of a
/// circular outline instead of the square corners of repeated 3x3 steps.
pub(crate) fn dilate_with(mask: &Mask, times: usize, round: bool) -> Mask {
    let mut cur = Mask {
        w: mask.w,
        h: mask.h,
        a: mask.a.clone(),
    };
    for step in 0..times {
        let plus = round && step % 2 == 0;
        let mut next = Mask::new(cur.w, cur.h);
        for y in 0..cur.h {
            for x in 0..cur.w {
                let mut v = 0;
                for yy in y.saturating_sub(1)..=(y + 1).min(cur.h - 1) {
                    for xx in x.saturating_sub(1)..=(x + 1).min(cur.w - 1) {
                        if plus && yy != y && xx != x {
                            continue;
                        }
                        v = v.max(cur.a[yy * cur.w + xx]);
                    }
                }
                next.a[y * cur.w + x] = v;
            }
        }
        cur = next;
    }
    cur
}

fn source_over(dst: &mut [u8], src: &[u8], coverage: u8) {
    let sa = u32::from(div255(u32::from(src[3]) * u32::from(coverage)));
    if sa == 0 {
        return;
    }
    let back = u32::from(dst[3]) * (255 - sa);
    let alpha = sa * 255 + back;
    for k in 0..3 {
        dst[k] =
            ((u32::from(src[k]) * sa * 255 + u32::from(dst[k]) * back + alpha / 2) / alpha) as u8;
    }
    dst[3] = div255(alpha);
}

fn paste_color(img: &mut graphics::Image, color: [u8; 4], mask: &Mask, over: bool) {
    for (px, m) in img.rgba.chunks_mut(4).zip(&mask.a) {
        if over {
            source_over(px, &color, *m);
            continue;
        }
        for (out, ink) in px.iter_mut().zip(color) {
            *out = blend(*out, ink, *m);
        }
    }
}

fn rgb(v: &Value) -> Result<[u8; 4]> {
    let a = v.as_array().context("color")?;
    ensure!(
        a.len() == 3 || a.len() == 4,
        "color needs 3 or 4 components"
    );
    let mut c = [255u8; 4];
    for (i, x) in a.iter().enumerate() {
        c[i] = u8::try_from(x.as_u64().context("color component")?)?;
    }
    Ok(c)
}

fn nums<const N: usize>(v: &Value) -> Result<[i64; N]> {
    let a = v.as_array().context("number list")?;
    ensure!(a.len() == N, "expected {N} numbers");
    let mut out = [0; N];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_i64().context("number")?;
    }
    Ok(out)
}

fn crop(src: &graphics::Image, x: usize, y: usize, w: usize, h: usize) -> Result<graphics::Image> {
    ensure!(
        x + w <= src.width && y + h <= src.height,
        "base crop outside image"
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

pub(crate) fn load_font(
    root: &Path,
    path: &str,
    cache: &mut BTreeMap<String, fontdue::Font>,
) -> Result<()> {
    if !cache.contains_key(path) {
        let font =
            fontdue::Font::from_bytes(fs::read(root.join(path))?, fontdue::FontSettings::default())
                .map_err(anyhow::Error::msg)?;
        cache.insert(path.to_string(), font);
    }
    Ok(())
}

fn draw(
    root: &Path,
    item: &Value,
    font: &fontdue::Font,
    hint: Option<&[u8]>,
    written: &BTreeMap<String, PathBuf>,
) -> Result<(graphics::Image, usize)> {
    let [cw, ch] = nums::<2>(&item["size"])?.map(|v| v as usize);
    let [tx, ty, w, h] = match item.get("text_rect") {
        Some(r) => nums::<4>(r)?.map(|v| v as usize),
        None => [0, 0, cw, ch],
    };
    let int = |k: &str, d: i64| item.get(k).and_then(Value::as_i64).unwrap_or(d);
    let margin = int("margin", 2) as i32;
    let o = int("outline_px", 2) as usize;
    let oo = int("outer_px", 0) as usize;
    let o_total = (o + oo) as i32;
    let [sdx, sdy] = match item.get("shadow") {
        Some(s) => nums::<2>(s)?.map(|v| v as i32),
        None => [0, 0],
    };
    let tracking = item.get("tracking").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let text = item["text"].as_str().context("text")?;
    let (max_px, min_px) = (int("max_px", 0), int("min_px", 0));
    let mut chosen = None;
    for px in (min_px..=max_px).rev() {
        let (glyphs, [l, t, r, b]) = layout(font, px as f32, text, tracking, hint)?;
        let (tw, th) = (r - l, b - t);
        if tw + 2 * o_total + sdx + 2 * margin <= w as i32
            && th + 2 * o_total + sdy + 2 * margin <= h as i32
        {
            chosen = Some((px as usize, glyphs, [l, t, r, b]));
            break;
        }
    }
    let Some((px, glyphs, [l, t, r, b])) = chosen else {
        bail!("text does not fit: {} {text}", item["id"]);
    };
    let (tw, th) = ((r - l) as f32, (b - t) as f32);
    let x = match item
        .get("align")
        .and_then(Value::as_str)
        .unwrap_or("center")
    {
        "center" => (w as f32 - tw - sdx as f32) / 2.0 - l as f32,
        "right" => (w as i32 - margin - o_total - sdx) as f32 - tw - l as f32,
        "left" => (margin + o_total - l) as f32,
        a => bail!("unknown align {a}"),
    };
    let y = (h as f32 - th - sdy as f32) / 2.0 - t as f32;
    let mut glyph = Mask::new(w, h);
    draw_glyphs(&mut glyph, &glyphs, x.round() as i32, y.round() as i32);
    let round = item
        .get("round_outline")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let edge = dilate_with(&glyph, o, round);
    let outer = (oo > 0).then(|| dilate_with(&edge, oo, round));
    let mut canvas = graphics::Image {
        width: cw,
        height: ch,
        format: 3,
        order: 0,
        rgba: vec![0; cw * ch * 4],
    };
    if let Some(base) = item.get("base") {
        let png = base["png"].as_str().context("base png")?;
        let path = written.get(png).cloned().unwrap_or_else(|| root.join(png));
        let [bx, by] = nums::<2>(&base["origin"])?.map(|v| v as usize);
        canvas = crop(&authoring::png_read(&path)?, bx, by, cw, ch)?;
        if let Some(patches) = item.get("base_patches") {
            for patch in patches.as_array().context("base patches")? {
                let png = root.join(patch["png"].as_str().context("base patch png")?);
                let [sx, sy, pw, ph] = nums::<4>(&patch["rect"])?.map(|n| n as usize);
                let [dx, dy] = nums::<2>(&patch["origin"])?.map(|n| n as usize);
                ensure!(
                    dx <= cw && dy <= ch && pw <= cw - dx && ph <= ch - dy,
                    "base patch outside canvas"
                );
                let flip_x = patch
                    .get("flip_x")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let patch = crop(&authoring::png_read(&png)?, sx, sy, pw, ph)?;
                for row in 0..ph {
                    let target = ((dy + row) * cw + dx) * 4;
                    for col in 0..pw {
                        let source_col = if flip_x { pw - 1 - col } else { col };
                        let source = (row * pw + source_col) * 4;
                        canvas.rgba[target + col * 4..target + col * 4 + 4]
                            .copy_from_slice(&patch.rgba[source..source + 4]);
                    }
                }
            }
        }
        let px_at = |c: &graphics::Image, x: usize, y: usize| -> [u8; 4] {
            c.rgba[(y * c.width + x) * 4..][..4].try_into().unwrap()
        };
        let set = |c: &mut graphics::Image, x: usize, y: usize, v: [u8; 4]| {
            c.rgba[(y * c.width + x) * 4..][..4].copy_from_slice(&v);
        };
        if item
            .get("keep_base")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
        } else if let Some(reference) = item.get("row_reference") {
            let reference_path = root.join(reference["png"].as_str().context("row reference png")?);
            let source = authoring::png_read(&reference_path)?;
            let column = reference["column"]
                .as_u64()
                .context("row reference column")? as usize;
            let source_y = reference["source_y"]
                .as_u64()
                .context("row reference source_y")? as usize;
            let [rx, ry, rw, rh] = match reference.get("rect") {
                Some(rect) => nums::<4>(rect)?.map(|n| n as usize),
                None => [tx, ty, w, h],
            };
            ensure!(
                column < source.width
                    && source_y <= source.height
                    && rh <= source.height - source_y,
                "row reference outside source"
            );
            ensure!(
                rx <= cw && ry <= ch && rw <= cw - rx && rh <= ch - ry,
                "row reference outside canvas"
            );
            for row in 0..rh {
                let color = px_at(&source, column, source_y + row);
                for xx in rx..rx + rw {
                    set(&mut canvas, xx, ry + row, color);
                }
            }
        } else if item
            .get("row_interpolate")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            for yy in ty..ty + h {
                let a = px_at(&canvas, tx - 1, yy);
                let b = px_at(&canvas, tx + w, yy);
                for i in 0..w {
                    let f = (i + 1) as f64 / (w + 1) as f64;
                    let v = std::array::from_fn(|k| {
                        (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * f)
                            .round_ties_even() as u8
                    });
                    set(&mut canvas, tx + i, yy, v);
                }
            }
        } else if let Some(circle) = item.get("erase_circle") {
            let [cx, cy] = nums::<2>(&circle["center"])?;
            let rad = circle["radius"].as_i64().context("radius")?;
            let color = rgb(&circle["color"])?;
            // Pillow ellipse over the inclusive box [c - r, c + r].
            for yy in (cy - rad).max(0)..=(cy + rad).min(ch as i64 - 1) {
                for xx in (cx - rad).max(0)..=(cx + rad).min(cw as i64 - 1) {
                    let (dx, dy) = ((xx - cx) as f64, (yy - cy) as f64);
                    let rr = rad as f64 + 0.5;
                    if dx * dx + dy * dy <= rr * rr {
                        set(&mut canvas, xx as usize, yy as usize, color);
                    }
                }
            }
        } else if let Some(c) = item.get("erase_color") {
            let color = rgb(c)?;
            for yy in ty..ty + h {
                for xx in tx..tx + w {
                    set(&mut canvas, xx, yy, color);
                }
            }
        } else if let Some(col) = item.get("row_extend_column").and_then(Value::as_u64) {
            for yy in ty..ty + h {
                let c = px_at(&canvas, col as usize, yy);
                for xx in tx..tx + w {
                    set(&mut canvas, xx, yy, c);
                }
            }
        } else {
            for yy in ty..ty + h {
                for xx in tx..tx + w {
                    set(&mut canvas, xx, yy, [0; 4]);
                }
            }
        }
    }
    let mut img = graphics::Image {
        width: w,
        height: h,
        format: 3,
        order: 0,
        rgba: vec![0; w * h * 4],
    };
    let over = match item.get("composite").and_then(Value::as_str) {
        None | Some("pillow_paste") => false,
        Some("source_over") => true,
        Some(other) => bail!("unknown composite mode {other}"),
    };
    if sdx != 0 || sdy != 0 {
        let src = outer.as_ref().unwrap_or(&edge);
        let mut shadow = Mask::new(w, h);
        for yy in 0..h as i32 {
            for xx in 0..w as i32 {
                let (sx, sy) = (xx - sdx, yy - sdy);
                if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                    shadow.a[yy as usize * w + xx as usize] = src.a[sy as usize * w + sx as usize];
                }
            }
        }
        let color = rgb(item.get("shadow_color").unwrap_or(&item["outline"]))?;
        paste_color(&mut img, color, &shadow, over);
    }
    if let Some(outer) = &outer {
        paste_color(&mut img, rgb(&item["outer"])?, outer, over);
    }
    paste_color(&mut img, rgb(&item["outline"])?, &edge, over);
    paste_color(&mut img, rgb(&item["fill"])?, &glyph, over);
    for yy in 0..h {
        for xx in 0..w {
            let s = &img.rgba[(yy * w + xx) * 4..][..4];
            let m = s[3];
            let d = ((ty + yy) * cw + tx + xx) * 4;
            if over {
                source_over(&mut canvas.rgba[d..d + 4], s, 255);
                continue;
            }
            for (out, ink) in canvas.rgba[d..d + 4].iter_mut().zip(s) {
                *out = blend(*out, *ink, m);
            }
        }
    }
    Ok((canvas, px))
}

fn adopt_entries(v: Option<&Value>) -> Vec<&Value> {
    match v {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(o @ Value::Object(_)) => vec![o],
        _ => Vec::new(),
    }
}

/// Palette-fit a draft into an adopted PNG: `{"png", "gim"?, "source_rect"?, "size"?,
/// "protect"?: {"base", "origin", "rects"?}}`. Protection keeps the original pixels outside
/// the rectangles (or outside the draft/base difference box when `rects` is absent).
pub(crate) fn adopt(root: &Path, draft: &Path, item: &Value, entry: &Value) -> Result<Value> {
    let png = entry["png"].as_str().context("adopt png")?;
    let gim = entry
        .get("gim")
        .or_else(|| item.get("gim"))
        .and_then(Value::as_str)
        .context("adopt gim")?;
    let source_rect = entry
        .get("source_rect")
        .map(nums::<4>)
        .transpose()?
        .map(|r| r.map(|v| v as usize));
    let [w, h] = match (entry.get("size"), source_rect) {
        (Some(s), _) => nums::<2>(s)?.map(|v| v as usize),
        (None, Some([_, _, w, h])) => [w, h],
        (None, None) => nums::<2>(&item["size"])?.map(|v| v as usize),
    };
    let protect = entry
        .get("protect")
        .map(|p| -> Result<crate::prepare::Protect> {
            Ok(crate::prepare::Protect {
                base: root.join(p["base"].as_str().context("protect base")?),
                origin: nums::<2>(&p["origin"])?.map(|v| v as usize),
                rects: match p.get("rects") {
                    Some(r) => r
                        .as_array()
                        .context("rects")?
                        .iter()
                        .map(|r| Ok(nums::<4>(r)?.map(|v| v as usize)))
                        .collect::<Result<_>>()?,
                    None => Vec::new(),
                },
            })
        })
        .transpose()?;
    let tmp = tempfile::tempdir()?;
    let out = tmp.path().join("region");
    let receipt = crate::prepare::region(
        draft,
        &root.join(gim),
        [w, h],
        crate::prepare::RegionOptions {
            trim_alpha: false,
            trim_padding: None,
            palette_colors: None,
            palette_rgba: Vec::new(),
            pattern_guide: None,
            match_alpha: entry
                .get("match_alpha")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            match_backing_boundary: false,
            left_align: false,
            source_rect,
            protect,
            key_black: None,
            drop_specks: false,
        },
        &out,
    )?;
    let dst = root.join(png);
    fs::create_dir_all(dst.parent().context("adopt dir")?)?;
    fs::copy(out.join("region.png"), &dst)?;
    Ok(
        json!({"png":png,"png_sha256":sha256(&fs::read(&dst)?),"allowed_rects":receipt.get("allowed_rects"),"gim":gim}),
    )
}

/// Render every item of a display spec. With `out_dir`, outputs keep their relative paths
/// under it and bases that name an earlier output read the redirected file. With `adopt`,
/// each item's `adopt` entries are palette-fitted into their adopted PNGs.
pub fn render(
    root: &Path,
    spec_path: &Path,
    out_dir: Option<&Path>,
    adopt_outputs: bool,
) -> Result<Value> {
    ensure!(
        !(adopt_outputs && out_dir.is_some()),
        "adoption writes spec drafts in place"
    );
    let raw = fs::read(root.join(spec_path))?;
    let spec: Value = serde_json::from_slice(&raw)?;
    let default_font = spec["font"].as_str().context("spec font")?.to_string();
    let mut fonts = BTreeMap::new();
    let mut written: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut receipts = Vec::new();
    let mut adopted = Vec::new();
    for item in spec["items"].as_array().context("items")? {
        let font_path = item
            .get("font")
            .and_then(Value::as_str)
            .unwrap_or(&default_font)
            .to_string();
        load_font(root, &font_path, &mut fonts)?;
        // `"rasterizer": "autohint"` (spec or item) snaps stems to pixels; see [`crate::hinting`].
        let hint_data = match item
            .get("rasterizer")
            .or_else(|| spec.get("rasterizer"))
            .and_then(Value::as_str)
        {
            None | Some("fontdue") => None,
            Some("autohint") => Some(fs::read(root.join(&font_path))?),
            Some(other) => bail!("unknown rasterizer {other}"),
        };
        let hint = hint_data.as_deref();
        let (img, px) = if item.get("plates").is_some() {
            plates::draw(root, item, &fonts[&font_path], hint, &written)?
        } else {
            draw(root, item, &fonts[&font_path], hint, &written)?
        };
        let output = item["output"].as_str().context("output")?;
        let dst = out_dir.map_or_else(|| root.join(output), |d| d.join(output));
        fs::create_dir_all(dst.parent().context("output dir")?)?;
        authoring::png_write(&dst, &img)?;
        written.insert(output.to_string(), dst.clone());
        receipts.push(json!({"id":item["id"],"text":item["text"],"font_px":px,"png":output,"composite":item.get("composite").and_then(Value::as_str).unwrap_or("pillow_paste"),"sha256":sha256(&fs::read(&dst)?)}));
        if adopt_outputs {
            for entry in adopt_entries(item.get("adopt")) {
                adopted.push(
                    adopt(root, &dst, item, entry)
                        .with_context(|| format!("adopt {}", item["id"]))?,
                );
            }
        }
    }
    Ok(
        json!({"spec_sha256":sha256(&raw),"method":"fontdue coverage; per-item composite mode; 3x3 max-filter outline","items":receipts,"adopted":adopted}),
    )
}

mod plates;

#[cfg(test)]
mod tests;
