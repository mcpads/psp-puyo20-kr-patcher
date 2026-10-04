//! Word plates stretched from an original plate crop, each holding one text run.
//!
//! Item keys: `"plates": {"template": {"png", "rect": [x,y,w,h]}, "caps": [left,right],
//! "fill_column", "texts": [..], "px", "min_px"?, "gap"?, "padding"?, "top",
//! "text_band": [y0,y1], "area"?: [x0,x1]}` plus the item `fill` color and optional `base`
//! with `clear_rect`. Each plate keeps the template's left and right caps (border and shadow)
//! and repeats one text-free column between them, so a vertical gradient stays seamless. The
//! largest size in `[min_px, px]` at which every plate fits the area is used; the plate row
//! is centred in the area and each text is centred on its plate's face.
use super::{Mask, crop, draw_glyphs, layout, nums, paste_color, rgb};
use crate::{authoring, graphics};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(super) fn draw(
    root: &Path,
    item: &Value,
    font: &fontdue::Font,
    hint: Option<&[u8]>,
    written: &BTreeMap<String, PathBuf>,
) -> Result<(graphics::Image, usize)> {
    let [cw, ch] = nums::<2>(&item["size"])?.map(|v| v as usize);
    let spec = &item["plates"];
    let read = |png: &str| {
        let path = written.get(png).cloned().unwrap_or_else(|| root.join(png));
        authoring::png_read(&path)
    };
    let mut canvas = match item.get("base") {
        Some(base) => {
            let [bx, by] = nums::<2>(&base["origin"])?.map(|v| v as usize);
            let mut c = crop(
                &read(base["png"].as_str().context("base png")?)?,
                bx,
                by,
                cw,
                ch,
            )?;
            if let Some(r) = item.get("clear_rect") {
                let [x, y, w, h] = nums::<4>(r)?.map(|v| v as usize);
                ensure!(x + w <= cw && y + h <= ch, "clear_rect outside cell");
                for yy in y..y + h {
                    c.rgba[(yy * cw + x) * 4..(yy * cw + x + w) * 4].fill(0);
                }
            }
            c
        }
        None => graphics::Image {
            width: cw,
            height: ch,
            format: 3,
            order: 0,
            rgba: vec![0; cw * ch * 4],
        },
    };
    let [tx, tyy, tw, th] = nums::<4>(&spec["template"]["rect"])?.map(|v| v as usize);
    let template = crop(
        &read(spec["template"]["png"].as_str().context("template png")?)?,
        tx,
        tyy,
        tw,
        th,
    )?;
    let [left, right] = nums::<2>(&spec["caps"])?.map(|v| v as usize);
    let fill_column = spec["fill_column"].as_u64().context("fill_column")? as usize;
    ensure!(
        left + right < tw && (left..tw - right).contains(&fill_column),
        "plate caps and fill column must fit the template"
    );
    let int = |k: &str, d: i64| spec.get(k).and_then(Value::as_i64).unwrap_or(d);
    let (gap, padding, top) = (int("gap", 1), int("padding", 4), int("top", 0));
    let [band0, band1] = nums::<2>(&spec["text_band"])?;
    let [area0, area1] = match spec.get("area") {
        Some(a) => nums::<2>(a)?,
        None => [0, cw as i64],
    };
    ensure!(
        top >= 0 && top as usize + th <= ch && area0 >= 0 && area1 <= cw as i64,
        "plate row outside cell"
    );
    let texts: Vec<&str> = spec["texts"]
        .as_array()
        .context("texts")?
        .iter()
        .map(|t| t.as_str().context("plate text"))
        .collect::<Result<_>>()?;
    ensure!(!texts.is_empty(), "no plate texts");
    let (max_px, min_px) = (int("px", 0), int("min_px", int("px", 0)));
    let mut chosen = None;
    for px in (min_px..=max_px).rev() {
        let runs = texts
            .iter()
            .map(|t| layout(font, px as f32, t, 0.0, hint))
            .collect::<Result<Vec<_>>>()?;
        let widths: Vec<usize> = runs
            .iter()
            .map(|(_, [l, _, r, _])| {
                ((r - l) as usize + 2 * padding as usize).max(left + right + 1)
            })
            .collect();
        let total = widths.iter().sum::<usize>() as i64 + gap * (texts.len() as i64 - 1);
        if total <= area1 - area0 {
            chosen = Some((px as usize, runs, widths, total));
            break;
        }
    }
    let Some((px, runs, widths, total)) = chosen else {
        bail!("plates do not fit: {}", item["id"]);
    };
    let mut x = area0 + (area1 - area0 - total) / 2;
    let fill = rgb(&item["fill"])?;
    for ((glyphs, [l, t, r, b]), width) in runs.iter().zip(&widths) {
        for j in 0..*width {
            let src = if j < left {
                j
            } else if j >= width - right {
                tw - (width - j)
            } else {
                fill_column
            };
            for row in 0..th {
                let s = &template.rgba[(row * tw + src) * 4..][..4];
                let d = ((top as usize + row) * cw + x as usize + j) * 4;
                canvas.rgba[d..d + 4].copy_from_slice(s);
            }
        }
        // Centre the text on the face, which excludes the right cap's shadow column.
        let face = (*width - right + left) as f32 / 2.0;
        let gx = x as f32 + face - (r - l) as f32 / 2.0 - *l as f32;
        let gy = top as f32 + (band0 + band1) as f32 / 2.0 - (b - t) as f32 / 2.0 - *t as f32;
        let mut mask = Mask::new(cw, ch);
        draw_glyphs(&mut mask, glyphs, gx.round() as i32, gy.round() as i32);
        paste_color(&mut canvas, fill, &mask, false);
        x += *width as i64 + gap;
    }
    Ok((canvas, px))
}
