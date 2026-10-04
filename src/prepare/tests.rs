use super::*;

#[test]
fn backing_palette_excludes_letter_colors_and_rejects_non_source_colors() {
    let source = vec![
        [152, 234, 255, 255],
        [92, 194, 236, 255],
        [255, 255, 255, 255],
    ];
    let selected = restrict_palette(source.clone(), &source[..2]).unwrap();
    assert_eq!(palette_color([255; 4], &selected, false), source[0]);
    assert!(restrict_palette(source, &[[1, 2, 3, 255]]).is_err());
    assert!(restrict_palette(Vec::new(), &[[1, 2, 3, 255]]).is_err());
}

#[test]
fn backing_correction_uses_boundary_without_reintroducing_original_letters() {
    let mut source = graphics::Image {
        width: 9,
        height: 9,
        format: 3,
        order: 0,
        rgba: [40, 60, 80, 255].repeat(81),
    };
    for y in 2..7 {
        for x in 2..7 {
            source.rgba[(y * 9 + x) * 4..][..4].copy_from_slice(&[255, 0, 255, 255]);
        }
    }
    let mut draft = graphics::Image {
        width: 9,
        height: 9,
        format: 3,
        order: 0,
        rgba: [80, 100, 120, 255].repeat(81),
    };
    boundary::match_backing(&mut draft, &source, &[[2, 2, 5, 5]], &[]).unwrap();
    for y in 0..9 {
        for x in 0..9 {
            let expected = if (2..7).contains(&x) && (2..7).contains(&y) {
                [40, 60, 80, 255]
            } else {
                [80, 100, 120, 255]
            };
            assert_eq!(&draft.rgba[(y * 9 + x) * 4..][..4], &expected);
        }
    }
    assert!(boundary::match_backing(&mut draft, &source, &[[0, 0, 9, 9]], &[]).is_err());
}

#[test]
fn alpha_matching_does_not_trade_opaque_backing_for_a_closer_rgb() {
    let palette = [[250, 194, 1, 255], [219, 142, 3, 223], [0, 0, 0, 0]];
    let draft = [200, 140, 0, 255];
    assert_eq!(palette_color(draft, &palette, false), palette[1]);
    assert_eq!(palette_color(draft, &palette, true), palette[0]);
    assert_eq!(palette_color([200, 140, 0, 0], &palette, true), palette[2]);
    assert_eq!(
        palette_color([200, 140, 0, 220], &palette, true),
        palette[1]
    );
}

#[test]
fn source_rectangle_excludes_neighbor_pixels_and_rejects_invalid_extents() {
    let image = || graphics::Image {
        width: 3,
        height: 2,
        format: 3,
        order: 0,
        rgba: (1..=6).flat_map(|v| [v, 0, 0, 255]).collect(),
    };
    let selected = crop(image(), Some([1, 0, 2, 2])).unwrap();
    assert_eq!((selected.width, selected.height), (2, 2));
    assert_eq!(
        selected.rgba,
        [2, 0, 0, 255, 3, 0, 0, 255, 5, 0, 0, 255, 6, 0, 0, 255]
    );
    for rect in [
        [3, 0, 1, 1],
        [0, 2, 1, 1],
        [0, 0, 0, 1],
        [usize::MAX, 0, 2, 1],
    ] {
        assert!(crop(image(), Some(rect)).is_err());
    }
}

#[test]
fn color_limit_keeps_existing_colors_and_stable_frequency_ties() {
    let a = [10, 20, 30, 255];
    let b = [20, 30, 40, 255];
    let c = [30, 40, 50, 255];
    let mut first = [a, b, c, a].concat();
    let mut second = [c, b, a, a].concat();
    reduce_colors(&mut first, 2).unwrap();
    reduce_colors(&mut second, 2).unwrap();
    assert_eq!(first, [a, b, b, a].concat());
    assert_eq!(second, [b, b, a, a].concat());
    assert!(reduce_colors(&mut first, 1).is_err());
}
#[test]
fn faint_margin_noise_does_not_shrink_title() {
    let mut im = graphics::Image {
        width: 6,
        height: 4,
        format: 3,
        order: 0,
        rgba: vec![0; 6 * 4 * 4],
    };
    assert!(visible_bounds(&im).is_err());
    im.rgba[3] = 1;
    for y in 1..3 {
        for x in 2..5 {
            im.rgba[(y * 6 + x) * 4 + 3] = 255;
        }
    }
    assert_eq!(visible_bounds(&im).unwrap(), (2, 1, 3, 2));
}
#[test]
fn transparent_rgb_does_not_bias_palette_distance() {
    assert_eq!(distance([255, 0, 0, 0], [0, 255, 255, 0]), 0);
    assert!(distance([255, 255, 255, 255], [255, 255, 255, 128]) > 0);
}

#[test]
fn black_key_clears_ground_and_unblends_edges() {
    let mut img = graphics::Image {
        width: 3,
        height: 1,
        format: 3,
        order: 0,
        rgba: vec![2, 1, 0, 255, 60, 30, 0, 255, 200, 100, 50, 255],
    };
    key_black(&mut img, [8, 128]).unwrap();
    assert_eq!(&img.rgba[0..4], &[0, 0, 0, 0]);
    // Brightest channel 60 is 13/30 of the ramp: alpha drops and colour is restored.
    assert_eq!(img.rgba[7], 111);
    assert_eq!(&img.rgba[4..7], &[138, 69, 0]);
    assert_eq!(&img.rgba[8..12], &[200, 100, 50, 255]);
    assert!(key_black(&mut img, [9, 9]).is_err());
}

#[test]
fn specks_under_a_tenth_of_the_largest_piece_are_cleared() {
    // A 4x4 block (16 px) and a single pixel apart from it.
    let mut img = graphics::Image {
        width: 7,
        height: 4,
        format: 3,
        order: 0,
        rgba: vec![0; 7 * 4 * 4],
    };
    for y in 0..4 {
        for x in 0..4 {
            img.rgba[(y * 7 + x) * 4 + 3] = 255;
        }
    }
    img.rgba[(2 * 7 + 6) * 4 + 3] = 255;
    drop_specks(&mut img);
    assert_eq!(img.rgba[(2 * 7 + 6) * 4 + 3], 0);
    assert_eq!(img.rgba[(3 * 7 + 3) * 4 + 3], 255);
}
