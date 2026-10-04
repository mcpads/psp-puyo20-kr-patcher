//! Authoring-time GIM rectangle composition and preparation receipts.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GimPreparation {
    gim: String,
    gim_sha256: String,
    png: String,
    png_sha256: String,
    allowed_rects: Vec<[usize; 4]>,
    #[serde(default)]
    source_origins: Option<Vec<[usize; 2]>>,
}

pub fn prepare_gim(root: &Path, manifest: &Path, output: &Path) -> Result<Value> {
    ensure!(!output.exists(), "GIM preparation output exists");
    let raw = fs::read(manifest)?;
    let m: GimPreparation = serde_json::from_slice(&raw)?;
    let source = fs::read(root.join(&m.gim))?;
    ensure!(
        sha256(&source) == m.gim_sha256,
        "preparation GIM identity mismatch"
    );
    let path = root.join(&m.png);
    ensure!(
        sha256(&fs::read(&path)?) == m.png_sha256,
        "preparation PNG identity mismatch"
    );
    let mut image = graphics::decode(&source)?;
    let draft = authoring::png_read(&path)?;
    merge_rectangles(
        &mut image,
        &draft,
        &m.allowed_rects,
        m.source_origins.as_deref(),
    )?;
    if image.format != 10 {
        replace_gim(&source, &image, &m.allowed_rects)?;
    }

    fs::create_dir_all(output)?;
    authoring::png_write(&output.join("image.png"), &image)?;
    let receipt = json!({"manifest_sha256":sha256(&raw),"gim_sha256":m.gim_sha256,"input_png_sha256":m.png_sha256,"allowed_rects":m.allowed_rects,"source_origins":m.source_origins,"encoding_status":if image.format == 10 {"uncompressed draft; DXT5 encoding and insertion not performed"} else {"lossless GIM roundtrip verified"},"output_png_sha256":sha256(&fs::read(output.join("image.png"))?),"output_rgba_sha256":sha256(&image.rgba)});
    fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}

pub(crate) fn merge_rectangles(
    image: &mut graphics::Image,
    draft: &graphics::Image,
    rects: &[[usize; 4]],
    origins: Option<&[[usize; 2]]>,
) -> Result<()> {
    ensure!(
        origins.is_some() || (image.width == draft.width && image.height == draft.height),
        "GIM preparation size mismatch"
    );
    // Reuse the insertion checks to validate dimensions, masks and overlap before copying.
    let extent = [0, 0, image.width, image.height];
    let original = graphics::Image {
        rgba: image.rgba.clone(),
        ..*image
    };
    replace_cell(image, &original, extent, Some(rects))?;
    ensure!(
        draft.rgba.len() == draft.width * draft.height * 4,
        "draft pixel length mismatch"
    );
    let defaults: Vec<_> = rects.iter().map(|r| [r[0], r[1]]).collect();
    let origins = origins.unwrap_or(&defaults);
    ensure!(origins.len() == rects.len(), "source origin count mismatch");
    for (&[sx, sy], &[_, _, w, h]) in origins.iter().zip(rects) {
        ensure!(
            sx.checked_add(w).is_some_and(|v| v <= draft.width)
                && sy.checked_add(h).is_some_and(|v| v <= draft.height),
            "source rectangle outside draft"
        );
    }
    for (&[sx, sy], &[x, y, w, h]) in origins.iter().zip(rects) {
        for row in 0..h {
            let start = ((y + row) * image.width + x) * 4;
            let input = ((sy + row) * draft.width + sx) * 4;
            image.rgba[start..start + w * 4].copy_from_slice(&draft.rgba[input..input + w * 4]);
        }
    }
    Ok(())
}
