//! Fixed-font Korean words inside consumer rectangles of standalone GIM textures.
//!
//! Spec: `{"font", "members": [{"member", "source_png", "output", "rects": [{"rect": [x,y,w,h],
//! "text", "background": "clear"|"row_extend", "source_column"?, "fill_hint"?,
//! "outline_hint"?, "bold"?, "font_px"?, "font"?, "align"?, "indent"?, "pad_*"?}]}]}`.
//! Text is rendered 1-bit with a one-pixel outline. Colours are chosen among colours already
//! in the original rectangle (fill from the brightest 40%, outline from the darkest 30%,
//! or nearest to a hint), so they are in the GIM palette; the build re-checks it.
use crate::{
    authoring,
    display_text::{Mask, draw_glyphs, layout, load_font},
    graphics, sha256,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

type Rgba = [u8; 4];

fn lum(c: &Rgba) -> f64 {
    0.299 * f64::from(c[0]) + 0.587 * f64::from(c[1]) + 0.114 * f64::from(c[2])
}

/// Colours of a rectangle in first-seen order with counts.
fn colors(img: &graphics::Image, [x, y, w, h]: [usize; 4]) -> Vec<(Rgba, usize)> {
    let mut order: Vec<(Rgba, usize)> = Vec::new();
    let mut index = BTreeMap::new();
    for yy in y..y + h {
        for xx in x..x + w {
            let c: Rgba = img.rgba[(yy * img.width + xx) * 4..][..4]
                .try_into()
                .unwrap();
            let i = *index.entry(c).or_insert_with(|| {
                order.push((c, 0));
                order.len() - 1
            });
            order[i].1 += 1;
        }
    }
    order
}

fn first_max(items: &[(Rgba, usize)]) -> Option<Rgba> {
    let mut best: Option<(Rgba, usize)> = None;
    for &(c, n) in items {
        if best.is_none_or(|b| n > b.1) {
            best = Some((c, n));
        }
    }
    best.map(|b| b.0)
}

fn pick(cols: &[(Rgba, usize)], hint: Option<[u8; 3]>, dark: bool) -> Result<Rgba> {
    let opaque: Vec<_> = cols.iter().filter(|(c, _)| c[3] == 255).copied().collect();
    ensure!(!opaque.is_empty(), "no opaque colours in rectangle");
    if let Some(h) = hint {
        let d = |c: &Rgba| {
            (0..3)
                .map(|k| (i32::from(c[k]) - i32::from(h[k])).pow(2))
                .sum::<i32>()
        };
        let mut best = opaque[0].0;
        for (c, _) in &opaque {
            if d(c) < d(&best) {
                best = *c;
            }
        }
        return Ok(best);
    }
    let mut pixels: Vec<Rgba> = opaque
        .iter()
        .flat_map(|(c, n)| std::iter::repeat_n(*c, *n))
        .collect();
    pixels.sort_by(|a, b| lum(a).total_cmp(&lum(b)));
    let band = if dark {
        &pixels[..(pixels.len() * 3 / 10).max(1)]
    } else {
        &pixels[pixels.len() * 6 / 10..]
    };
    let mut counts: Vec<(Rgba, usize)> = Vec::new();
    for c in band {
        match counts.iter_mut().find(|(k, _)| k == c) {
            Some(e) => e.1 += 1,
            None => counts.push((*c, 1)),
        }
    }
    first_max(&counts).context("empty colour band")
}

fn hint(v: Option<&Value>) -> Result<Option<[u8; 3]>> {
    v.map(|v| {
        let a = v.as_array().context("hint")?;
        ensure!(a.len() == 3, "hint needs rgb");
        Ok(std::array::from_fn(|k| a[k].as_u64().unwrap_or(0) as u8))
    })
    .transpose()
}

fn render_rect(
    out: &mut graphics::Image,
    src: &graphics::Image,
    r: &Value,
    font: &fontdue::Font,
) -> Result<[usize; 4]> {
    let rect: Vec<usize> = r["rect"]
        .as_array()
        .context("rect")?
        .iter()
        .map(|v| v.as_u64().unwrap_or(0) as usize)
        .collect();
    ensure!(rect.len() == 4, "rect needs x,y,w,h");
    let [x, y, w, h] = [rect[0], rect[1], rect[2], rect[3]];
    ensure!(
        x + w <= src.width && y + h <= src.height,
        "rect outside texture"
    );
    let cols = colors(src, [x, y, w, h]);
    let fill = pick(&cols, hint(r.get("fill_hint"))?, false)?;
    let outline = pick(&cols, hint(r.get("outline_hint"))?, true)?;
    let num = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
    let set = |img: &mut graphics::Image, xx: usize, yy: usize, c: Rgba| {
        img.rgba[(yy * img.width + xx) * 4..][..4].copy_from_slice(&c);
    };
    match r["background"].as_str().context("background")? {
        "row_extend" => {
            let sx = r["source_column"].as_u64().context("source_column")? as usize;
            for yy in y..y + h {
                let c: Rgba = src.rgba[(yy * src.width + sx) * 4..][..4]
                    .try_into()
                    .unwrap();
                for xx in x..x + w {
                    set(out, xx, yy, c);
                }
            }
        }
        "clear" => {
            let clear: Vec<_> = cols.iter().filter(|(c, _)| c[3] == 0).copied().collect();
            let bg = first_max(&clear).context("no transparent colour in rectangle")?;
            for yy in y + num("pad_top")..y + h - num("pad_bottom") {
                for xx in x + num("pad_left")..x + w - num("pad_right") {
                    set(out, xx, yy, bg);
                }
            }
        }
        b => bail!("unknown background {b}"),
    }
    let px = r.get("font_px").and_then(Value::as_u64).unwrap_or(12) as f32;
    let text = r["text"].as_str().context("text")?;
    let (glyphs, [_, t, _, b]) = layout(font, px, text, 0.0, None)?;
    let advance: f32 = text
        .chars()
        .map(|c| font.metrics(c, px).advance_width)
        .sum();
    let ox = match r.get("align").and_then(Value::as_str).unwrap_or("center") {
        "center" => (w as f32 - advance) / 2.0,
        "left" => (1 + num("pad_left") + num("indent")) as f32,
        a => bail!("unknown align {a}"),
    };
    let oy = (h as f32 - (b - t) as f32) / 2.0 - t as f32;
    let mut mask = Mask::new(w, h);
    draw_glyphs(
        &mut mask,
        &glyphs,
        ox.round_ties_even() as i32,
        oy.round_ties_even() as i32,
    );
    let mut ink: BTreeSet<(i64, i64)> = BTreeSet::new();
    for j in 0..h {
        for i in 0..w {
            if mask.a[j * w + i] >= 128 {
                ink.insert((i as i64, j as i64));
            }
        }
    }
    if r.get("bold").and_then(Value::as_bool).unwrap_or(false) {
        let shifted: Vec<_> = ink.iter().map(|&(i, j)| (i + 1, j)).collect();
        ink.extend(shifted);
    }
    ensure!(!ink.is_empty(), "empty text");
    let (w, h) = (w as i64, h as i64);
    ensure!(
        ink.iter()
            .all(|&(i, j)| i >= 1 && j >= 1 && i <= w - 2 && j <= h - 2),
        "text exceeds rect with outline: {text} {rect:?}"
    );
    for &(i, j) in &ink {
        for di in -1..=1 {
            for dj in -1..=1 {
                if !ink.contains(&(i + di, j + dj)) {
                    set(
                        out,
                        (x as i64 + i + di) as usize,
                        (y as i64 + j + dj) as usize,
                        outline,
                    );
                }
            }
        }
    }
    for &(i, j) in &ink {
        set(out, x + i as usize, y + j as usize, fill);
    }
    Ok([x, y, w as usize, h as usize])
}

/// Render every member of a spec. With `out_dir`, outputs keep their relative paths under it.
pub fn render(root: &Path, spec_path: &Path, out_dir: Option<&Path>, adopt: bool) -> Result<Value> {
    ensure!(
        !(adopt && out_dir.is_some()),
        "adoption writes spec outputs in place"
    );
    let raw = fs::read(root.join(spec_path))?;
    let spec: Value = serde_json::from_slice(&raw)?;
    let default_font = spec["font"].as_str().context("spec font")?.to_string();
    let mut fonts = BTreeMap::new();
    let mut receipts = Vec::new();
    for m in spec["members"].as_array().context("members")? {
        let src = authoring::png_read(&root.join(m["source_png"].as_str().context("source_png")?))?;
        let mut out = graphics::Image {
            width: src.width,
            height: src.height,
            format: 3,
            order: 0,
            rgba: src.rgba.clone(),
        };
        let mut rects = Vec::new();
        for r in m["rects"].as_array().context("rects")? {
            let font_path = r
                .get("font")
                .and_then(Value::as_str)
                .unwrap_or(&default_font)
                .to_string();
            load_font(root, &font_path, &mut fonts)?;
            rects.push(render_rect(&mut out, &src, r, &fonts[&font_path])?);
        }
        let output = m["output"].as_str().context("output")?;
        let dst = out_dir.map_or_else(|| root.join(output), |d| d.join(output));
        fs::create_dir_all(dst.parent().context("output dir")?)?;
        authoring::png_write(&dst, &out)?;
        let mut record = json!({"member":m["member"],"png":output,"png_sha256":sha256(&fs::read(&dst)?),"allowed_rects":rects});
        // The rendered texture already uses original colours; adoption copies it.
        if adopt && let Some(target) = m.get("adopt").and_then(|a| a["png"].as_str()) {
            fs::copy(&dst, root.join(target))?;
            record["adopted"] =
                json!({"png":target,"png_sha256":sha256(&fs::read(root.join(target))?)});
        }
        receipts.push(record);
    }
    Ok(
        json!({"spec_sha256":sha256(&raw),"method":"fontdue 1-bit (coverage >= 128); one-pixel outline; colours from the original rectangle","members":receipts}),
    )
}

#[cfg(test)]
mod tests;
