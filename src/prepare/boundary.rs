use super::*;

/// Correct a generated, text-free opaque backing from unchanged source pixels around it.
/// Source pixels inside the mask are deliberately never used as color constraints.
pub(super) fn match_backing(
    draft: &mut graphics::Image,
    source: &graphics::Image,
    rects: &[[usize; 4]],
    palette: &[[u8; 4]],
) -> Result<Value> {
    let (w, h) = (draft.width, draft.height);
    let mut mask = vec![false; w * h];
    for &[x, y, rw, rh] in rects {
        ensure!(x + rw <= w && y + rh <= h, "backing mask outside image");
        for yy in y..y + rh {
            mask[yy * w + x..yy * w + x + rw].fill(true);
        }
    }
    let neighbors = |i: usize| {
        let (x, y) = (i % w, i / w);
        [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then_some(i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < h).then_some(i + w),
        ]
    };
    let mut correction = vec![[0.0f64; 3]; w * h];
    let mut anchors = 0;
    for (i, &inside) in mask.iter().enumerate() {
        if inside {
            ensure!(
                source.rgba[i * 4 + 3] == 255,
                "backing source must be opaque inside mask"
            );
        } else if source.rgba[i * 4 + 3] == 255 && draft.rgba[i * 4 + 3] >= 240 {
            for (k, value) in correction[i].iter_mut().enumerate() {
                *value = f64::from(source.rgba[i * 4 + k]) - f64::from(draft.rgba[i * 4 + k]);
            }
            if neighbors(i).into_iter().flatten().any(|j| mask[j]) {
                anchors += 1;
            }
        }
    }
    ensure!(
        anchors > 0,
        "no opaque unchanged source boundary for backing correction"
    );
    let mut iterations = 0;
    let mut residual = f64::INFINITY;
    while iterations < 4000 && residual > 0.001 {
        residual = 0.0;
        for (i, &inside) in mask.iter().enumerate() {
            if !inside {
                continue;
            }
            let mut sum = [0.0; 3];
            let mut count = 0;
            for j in neighbors(i).into_iter().flatten() {
                if mask[j] || (source.rgba[j * 4 + 3] == 255 && draft.rgba[j * 4 + 3] >= 240) {
                    for (s, c) in sum.iter_mut().zip(correction[j]) {
                        *s += c;
                    }
                    count += 1;
                }
            }
            ensure!(count > 0, "isolated backing correction pixel");
            for (k, s) in sum.into_iter().enumerate() {
                let next = s / f64::from(count);
                residual = residual.max((next - correction[i][k]).abs());
                correction[i][k] = next;
            }
        }
        iterations += 1;
    }
    ensure!(residual <= 0.001, "backing correction did not converge");
    for (i, &inside) in mask.iter().enumerate() {
        if !inside {
            continue;
        }
        let mut pixel = [0, 0, 0, 255];
        for k in 0..3 {
            pixel[k] = (f64::from(draft.rgba[i * 4 + k]) + correction[i][k])
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        let color = palette_color(pixel, palette, true);
        ensure!(color[3] == 255, "palette lacks opaque backing color");
        draft.rgba[i * 4..i * 4 + 4].copy_from_slice(&color);
    }
    Ok(
        json!({"method":"harmonic RGB correction from opaque outside-mask source boundary; original interior colors unused","anchors":anchors,"iterations":iterations,"residual":residual}),
    )
}
