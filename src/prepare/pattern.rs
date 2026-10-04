use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Region {
    rect: [usize; 4],
    colors: [[u8; 4]; 2],
}

#[derive(Deserialize)]
struct Guide {
    regions: Vec<Region>,
    minimum_observations: usize,
    prior_weight: f64,
}

fn field(
    w: usize,
    h: usize,
    anchors: &[Option<f64>],
    prior: &[f64],
    weight: f64,
) -> Result<(Vec<f64>, usize, f64)> {
    let mut values = prior.to_vec();
    for (v, a) in values.iter_mut().zip(anchors) {
        if let Some(a) = a {
            *v = *a;
        }
    }
    let mut residual = f64::INFINITY;
    let mut iterations = 0;
    while iterations < 4000 && residual > 0.00001 {
        residual = 0.0;
        for i in 0..values.len() {
            if anchors[i].is_some() {
                continue;
            }
            let (x, y) = (i % w, i / w);
            let neighbors = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then_some(i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then_some(i + w),
            ];
            let (mut sum, mut count) = (weight * prior[i], weight);
            for j in neighbors.into_iter().flatten() {
                sum += values[j];
                count += 1.0;
            }
            let next = sum / count;
            residual = residual.max((next - values[i]).abs());
            values[i] = next;
        }
        iterations += 1;
    }
    ensure!(residual <= 0.00001, "pattern alignment did not converge");
    Ok((values, iterations, residual))
}

pub(super) fn align(
    draft: &mut graphics::Image,
    source: &graphics::Image,
    palette: &[[u8; 4]],
    path: &Path,
) -> Result<Value> {
    let bytes = fs::read(path)?;
    let guide: Guide = serde_json::from_slice(&bytes)?;
    ensure!(
        guide.regions.len() >= 2,
        "pattern alignment needs repeated regions"
    );
    ensure!(
        (2..=guide.regions.len()).contains(&guide.minimum_observations),
        "insufficient pattern observations"
    );
    ensure!(
        guide.prior_weight.is_finite() && guide.prior_weight > 0.0 && guide.prior_weight <= 1.0,
        "pattern prior weight outside (0,1]"
    );
    ensure!(
        draft.width == source.width && draft.height == source.height,
        "pattern guide needs full-size source alignment"
    );
    let [_, _, w, h] = guide.regions[0].rect;
    ensure!(w > 0 && h > 0, "empty pattern region");
    let mut votes = vec![[0usize; 2]; w * h];
    let mut prior = vec![0.0; w * h];
    for (index, region) in guide.regions.iter().enumerate() {
        let [x, y, rw, rh] = region.rect;
        ensure!(
            rw == w
                && rh == h
                && x <= source.width
                && y <= source.height
                && w <= source.width - x
                && h <= source.height - y,
            "pattern region sizes or bounds differ"
        );
        ensure!(
            guide.regions[..index].iter().all(|other| {
                let [ox, oy, ow, oh] = other.rect;
                x >= ox + ow || ox >= x + w || y >= oy + oh || oy >= y + h
            }),
            "pattern observations must be disjoint"
        );
        ensure!(
            region.colors[0] != region.colors[1]
                && region
                    .colors
                    .iter()
                    .all(|c| c[3] == 255 && palette.contains(c)),
            "pattern colors must be distinct opaque original palette entries"
        );
        for yy in 0..h {
            for xx in 0..w {
                let local = yy * w + xx;
                let i = ((y + yy) * source.width + x + xx) * 4;
                let pixel: [u8; 4] = source.rgba[i..i + 4].try_into()?;
                if let Some(class) = region.colors.iter().position(|c| *c == pixel) {
                    votes[local][class] += 1;
                }
                let generated: [u8; 4] = draft.rgba[i..i + 4].try_into()?;
                if distance(generated, region.colors[1]) < distance(generated, region.colors[0]) {
                    prior[local] += 1.0 / guide.regions.len() as f64;
                }
            }
        }
    }
    let anchors: Vec<_> = votes
        .iter()
        .map(|v| {
            if v[0] >= guide.minimum_observations && v[1] == 0 {
                Some(0.0)
            } else if v[1] >= guide.minimum_observations && v[0] == 0 {
                Some(1.0)
            } else {
                None
            }
        })
        .collect();
    let count = anchors.iter().filter(|a| a.is_some()).count();
    ensure!(count > 0, "no agreed source pattern anchors");
    let (values, iterations, residual) = field(w, h, &anchors, &prior, guide.prior_weight)?;
    for region in &guide.regions {
        let [x, y, _, _] = region.rect;
        for yy in 0..h {
            for xx in 0..w {
                let i = ((y + yy) * draft.width + x + xx) * 4;
                let class = usize::from(values[yy * w + xx] >= 0.5);
                draft.rgba[i..i + 4].copy_from_slice(&region.colors[class]);
            }
        }
    }
    Ok(
        json!({"method":"two-color pattern alignment from unanimous repeated-source observations with generated-image prior", "guide_sha256":sha256(&bytes), "anchors":count, "conflicting_positions":votes.iter().filter(|v| v[0] > 0 && v[1] > 0).count(), "unobserved_positions":votes.iter().filter(|v| v[0] + v[1] == 0).count(), "iterations":iterations, "residual":residual, "prior_weight":guide.prior_weight}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_pattern_anchors_override_wrong_prior_with_a_smooth_transition() {
        let anchors = vec![Some(0.0), None, None, None, Some(1.0)];
        let (v, _, _) = field(5, 1, &anchors, &[1.0; 5], 0.02).unwrap();
        assert_eq!(v[0], 0.0);
        assert_eq!(v[4], 1.0);
        assert!(v[1] < 0.5);
        assert!(v[3] > 0.5);
        assert!(v.windows(2).all(|p| p[0] < p[1]));
    }
}
