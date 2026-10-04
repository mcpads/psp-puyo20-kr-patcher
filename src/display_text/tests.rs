use super::*;

#[test]
#[ignore = "requires assets/fonts/poc/Galmuri11.ttf"]
fn background_patch_and_reference_column_preserve_gradient_and_reject_bad_bounds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let mut reference = graphics::Image {
        width: 32,
        height: 32,
        format: 3,
        order: 0,
        rgba: vec![0; 32 * 32 * 4],
    };
    for y in 0..32 {
        for x in 0..32 {
            reference.rgba[(y * 32 + x) * 4..][..4].copy_from_slice(&[
                y as u8 * 7,
                20 + x as u8,
                30,
                255,
            ]);
        }
    }
    let path = dir.path().join("reference.png");
    authoring::png_write(&path, &reference).unwrap();
    let mut fonts = BTreeMap::new();
    load_font(root, "assets/fonts/poc/Galmuri11.ttf", &mut fonts)
        .expect("required test input assets/fonts/poc/Galmuri11.ttf is unavailable");
    let font = &fonts["assets/fonts/poc/Galmuri11.ttf"];
    let mut item = json!({"id":"reference", "text":"가", "size":[32,32], "text_rect":[0,16,32,16],
        "max_px":10, "min_px":10, "fill":[255,255,255], "outline":[0,0,0], "outline_px":0,
        "base":{"png":path,"origin":[0,0]},
        "base_patches":[{"png":path,"rect":[0,8,8,8],"origin":[0,0]}],
        "row_reference":{"png":path,"column":3,"source_y":2,"rect":[10,0,6,8]}});
    let (image, _) = draw(root, &item, font, None, &BTreeMap::new()).unwrap();
    assert_eq!(&image.rgba[..4], &[56, 20, 30, 255]);
    assert_eq!(&image.rgba[10 * 4..14 * 4], &[14, 23, 30, 255].repeat(4));
    assert_eq!(&image.rgba[20 * 4..21 * 4], &[0, 40, 30, 255]);
    item["base_patches"][0]["flip_x"] = json!(true);
    let (mirrored, _) = draw(root, &item, font, None, &BTreeMap::new()).unwrap();
    assert_eq!(&mirrored.rgba[..4], &[56, 27, 30, 255]);
    assert_eq!(&mirrored.rgba[7 * 4..8 * 4], &[56, 20, 30, 255]);
    assert_eq!(&mirrored.rgba[8 * 4..9 * 4], &image.rgba[8 * 4..9 * 4]);
    item["row_reference"]["source_y"] = json!(30);
    assert!(draw(root, &item, font, None, &BTreeMap::new()).is_err());
    item["row_reference"]["source_y"] = json!(2);
    item["base_patches"][0]["origin"] = json!([30, 0]);
    assert!(draw(root, &item, font, None, &BTreeMap::new()).is_err());
}

#[test]
fn source_over_preserves_translucent_backing_and_straight_color() {
    let mut backing = [0, 0, 0, 181];
    source_over(&mut backing, &[255, 255, 255, 255], 128);
    assert_eq!(backing, [150, 150, 150, 218]);
    let mut clear = [0, 0, 0, 0];
    source_over(&mut clear, &[255, 255, 255, 255], 128);
    assert_eq!(clear, [255, 255, 255, 128]);
    source_over(&mut clear, &[10, 20, 30, 0], 255);
    assert_eq!(clear, [255, 255, 255, 128]);
    source_over(&mut clear, &[10, 20, 30, 255], 255);
    assert_eq!(clear, [10, 20, 30, 255]);
}

#[test]
fn blend_matches_pillow_rounding() {
    assert_eq!(blend(0, 255, 255), 255);
    assert_eq!(blend(0, 255, 128), 128);
    assert_eq!(blend(200, 0, 0), 200);
}

#[test]
fn dilate_grows_one_pixel_per_pass() {
    let mut m = Mask::new(5, 5);
    m.a[12] = 255;
    assert_eq!(
        dilate_with(&m, 1, false)
            .a
            .iter()
            .filter(|v| **v == 255)
            .count(),
        9
    );
    assert_eq!(
        dilate_with(&m, 2, false)
            .a
            .iter()
            .filter(|v| **v == 255)
            .count(),
        25
    );
}

#[test]
#[ignore = "requires assets/fonts/poc/Galmuri11.ttf"]
fn largest_fitting_size_is_chosen_and_overflow_fails() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut fonts = BTreeMap::new();
    load_font(root, "assets/fonts/poc/Galmuri11.ttf", &mut fonts)
        .expect("required test input assets/fonts/poc/Galmuri11.ttf is unavailable");
    let font = &fonts["assets/fonts/poc/Galmuri11.ttf"];
    let item = json!({"id": "t", "text": "남은", "size": [40, 20], "max_px": 12, "min_px": 8,
        "fill": [255, 255, 255], "outline": [0, 0, 0], "outline_px": 1, "margin": 0});
    let (img, px) = draw(root, &item, font, None, &BTreeMap::new()).unwrap();
    assert_eq!(px, 12);
    assert!(img.rgba.chunks(4).any(|p| p == [255, 255, 255, 255]));
    let tight = json!({"id": "t", "text": "아주 긴 글자", "size": [20, 12], "max_px": 12, "min_px": 11,
        "fill": [255, 255, 255], "outline": [0, 0, 0]});
    assert!(draw(root, &tight, font, None, &BTreeMap::new()).is_err());
}

#[test]
#[ignore = "requires assets/fonts/poc/Galmuri11.ttf"]
fn plates_stretch_template_caps_and_shrink_to_fit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    // 8x6 plate: dark 1px border, grey face; the fill column is plain grey.
    let mut t = graphics::Image {
        width: 8,
        height: 6,
        format: 3,
        order: 0,
        rgba: vec![0; 8 * 6 * 4],
    };
    for y in 0..6 {
        for x in 0..8 {
            let edge = x == 0 || y == 0 || x == 7 || y == 5;
            let c: [u8; 4] = if edge {
                [20, 20, 20, 255]
            } else {
                [200, 200, 200, 255]
            };
            t.rgba[(y * 8 + x) * 4..][..4].copy_from_slice(&c);
        }
    }
    let png = dir.path().join("t.png");
    authoring::png_write(&png, &t).unwrap();
    let mut fonts = BTreeMap::new();
    load_font(root, "assets/fonts/poc/Galmuri11.ttf", &mut fonts)
        .expect("required test input assets/fonts/poc/Galmuri11.ttf is unavailable");
    let font = &fonts["assets/fonts/poc/Galmuri11.ttf"];
    let item = |w: usize| {
        json!({"id": "p", "text": "가 1", "size": [w, 20], "fill": [0, 0, 0],
            "plates": {"template": {"png": png.to_str().unwrap(), "rect": [0, 0, 8, 6]},
                "caps": [1, 1], "fill_column": 3, "texts": ["가", "1"], "px": 12, "min_px": 6,
                "gap": 1, "padding": 1, "top": 2, "text_band": [0, 6]}})
    };
    let (img, px) = plates::draw(root, &item(40), font, None, &BTreeMap::new()).unwrap();
    assert_eq!(px, 12);
    // Two separate plates: a transparent gap column lies between opaque borders on row 2.
    let row: Vec<u8> = (0..40).map(|x| img.rgba[(2 * 40 + x) * 4 + 3]).collect();
    let starts = row.windows(2).filter(|w| w[0] == 0 && w[1] == 255).count();
    assert_eq!(starts, 2);
    let (_, small) = plates::draw(root, &item(22), font, None, &BTreeMap::new()).unwrap();
    assert!(small < 12);
    assert!(plates::draw(root, &item(8), font, None, &BTreeMap::new()).is_err());
}

#[test]
fn round_outline_trims_square_corners() {
    let mut m = Mask::new(9, 9);
    m.a[4 * 9 + 4] = 255;
    let round = dilate_with(&m, 2, true);
    // Plus then square: a 5x5 block without its four corners.
    assert_eq!(round.a.iter().filter(|v| **v == 255).count(), 21);
    assert_eq!(round.a[2 * 9 + 2], 0);
}

#[test]
#[ignore = "requires assets/fonts/private/MaplestoryLight.ttf"]
fn autohint_snaps_stems_to_whole_pixels() {
    // A vertical stem rendered hinted has more fully covered pixels than unhinted.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(root.join("assets/fonts/private/MaplestoryLight.ttf"))
        .expect("required test input assets/fonts/private/MaplestoryLight.ttf is unavailable");
    let font =
        fontdue::Font::from_bytes(data.as_slice(), fontdue::FontSettings::default()).unwrap();
    let solid = |hint| {
        let (glyphs, _) = layout(&font, 11.0, "바람이기분", 0.0, hint).unwrap();
        glyphs
            .iter()
            .map(|g| g.cov.iter().filter(|&&a| a > 200).count())
            .sum::<usize>()
    };
    assert!(solid(Some(data.as_slice())) > solid(None));
}
