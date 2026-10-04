//! Standalone GIM member insertion into fixed-size archives.
use super::*;

pub(super) fn plan_gim_archive(
    root: &Path,
    iso: &mut fs::File,
    archive: GimArchive,
) -> Result<(ExpectedWrite, Value)> {
    ensure!(!archive.members.is_empty(), "empty GIM archive edit");
    let before = source::read_extent(iso, archive.offset, archive.size)?;
    ensure!(
        sha256(&before) == archive.sha256,
        "graphic ZIP identity mismatch"
    );
    let (after, receipts) = apply_gim_members(root, &before, before.clone(), archive.members)?;
    Ok((
        ExpectedWrite {
            offset: archive.offset,
            before,
            after,
        },
        json!({"offset":archive.offset,"gim_members":receipts}),
    ))
}

pub(super) fn apply_gim_members(
    root: &Path,
    before: &[u8],
    mut after: Vec<u8>,
    patches: Vec<GimPatch>,
) -> Result<(Vec<u8>, Vec<Value>)> {
    let mut original = zip::ZipArchive::new(Cursor::new(before))?;
    let mut names = BTreeSet::new();
    let mut outputs = Vec::new();
    for patch in patches {
        ensure!(
            names.insert(patch.member.clone()),
            "duplicate GIM member edit"
        );
        let source = member(&mut original, &patch.member)?;
        ensure!(
            sha256(&source) == patch.member_sha256,
            "graphic GIM identity mismatch"
        );
        let path = root.join(&patch.png);
        ensure!(
            sha256(&fs::read(&path)?) == patch.png_sha256,
            "graphic PNG identity mismatch"
        );
        let output = replace_gim(&source, &authoring::png_read(&path)?, &patch.allowed_rects)?;
        outputs.push((patch, output));
    }
    let mut receipts = Vec::new();
    for (patch, output) in outputs {
        let (next, receipt) = zip_patch::replace(&after, &patch.member, &output)?;
        after = next;
        receipts.push(json!({"member":patch.member,"source_gim_sha256":patch.member_sha256,"output_gim_sha256":sha256(&output),"png_sha256":patch.png_sha256,"allowed_rects":patch.allowed_rects,"zip":receipt}));
    }
    Ok((after, receipts))
}

pub(super) fn replace_gim(
    source: &[u8],
    replacement: &graphics::Image,
    allowed: &[[usize; 4]],
) -> Result<Vec<u8>> {
    let mut image = graphics::decode(source)?;
    let extent = [0, 0, image.width, image.height];
    replace_cell(&mut image, replacement, extent, Some(allowed))?;
    let encoded = graphics::encode(source, image.width, image.height, &image.rgba)?;
    ensure!(encoded.len() == source.len(), "GIM size changed");
    ensure!(
        graphics::decode(&encoded)?.rgba == image.rgba,
        "GIM pixel roundtrip mismatch"
    );
    Ok(encoded)
}
