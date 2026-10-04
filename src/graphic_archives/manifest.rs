//! Fixed source identities and graphic archive specification loading.
use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) archives: Vec<Archive>,
    #[serde(default)]
    pub(super) gim_archives: Vec<GimArchive>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GimArchive {
    pub(super) offset: u64,
    pub(super) size: usize,
    pub(super) sha256: String,
    pub(super) members: Vec<GimPatch>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GimPatch {
    pub(super) member: String,
    pub(super) member_sha256: String,
    pub(super) png: String,
    pub(super) png_sha256: String,
    pub(super) allowed_rects: Vec<[usize; 4]>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Archive {
    pub(super) offset: u64,
    pub(super) size: usize,
    pub(super) sha256: String,
    pub(super) member: String,
    pub(super) member_sha256: String,
    pub(super) snc_members: Vec<SncMember>,
    pub(super) cell_table_sha256: String,
    pub(super) patches: Vec<Patch>,
    #[serde(default)]
    pub(super) unmapped_regions: Vec<UnmappedRegion>,
    #[serde(default)]
    pub(super) prepared_dxt: Vec<PreparedDxt>,
    #[serde(default)]
    pub(super) shared_views: Vec<snc::SharedView>,
    #[serde(default)]
    pub(super) gim_members: Vec<GimPatch>,
    #[serde(default)]
    pub(super) snc_geometry: Vec<SncGeometry>,
    #[serde(default)]
    pub(super) snc_cell_remaps: Vec<SncCellRemaps>,
    // SNC members bound to another, unchanged SNT in the same ZIP.
    #[serde(default)]
    pub(super) other_snt_snc_members: Vec<OtherSntSnc>,
    #[serde(default)]
    pub(super) additional_snt_members: Vec<SntEdit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SntEdit {
    pub(super) member: String,
    pub(super) member_sha256: String,
    pub(super) snc_members: Vec<SncMember>,
    pub(super) cell_table_sha256: String,
    pub(super) patches: Vec<Patch>,
    #[serde(default)]
    pub(super) unmapped_regions: Vec<UnmappedRegion>,
    #[serde(default)]
    pub(super) prepared_dxt: Vec<PreparedDxt>,
    #[serde(default)]
    pub(super) shared_views: Vec<snc::SharedView>,
    #[serde(default)]
    pub(super) snc_geometry: Vec<SncGeometry>,
    #[serde(default)]
    pub(super) snc_cell_remaps: Vec<SncCellRemaps>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OtherSntSnc {
    pub(super) path: String,
    pub(super) sha256: String,
    pub(super) snt: String,
    pub(super) snt_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SncCellRemaps {
    pub(super) member: String,
    pub(super) draws: Vec<snc::CellRemap>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SncGeometry {
    pub(super) member: String,
    pub(super) draws: Vec<snc::DrawEdit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SncMember {
    pub(super) path: String,
    pub(super) sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UnmappedRegion {
    pub(super) slot: usize,
    pub(super) rect_xywh: [usize; 4],
    pub(super) png: String,
    pub(super) png_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreparedDxt {
    pub(super) slot: usize,
    pub(super) source_gim_sha256: String,
    pub(super) gim: String,
    pub(super) gim_sha256: String,
    pub(super) decoded_rgba_sha256: String,
    pub(super) allowed_rects: Vec<[usize; 4]>,
    pub(super) affected_cell_ids: Vec<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Patch {
    pub(super) cell_id: usize,
    pub(super) png: String,
    pub(super) png_sha256: String,
    // Cell-local rectangles; omitted means the entire validated writer cell.
    pub(super) allowed_rects: Option<Vec<[usize; 4]>>,
    // Write only the part of a cell that lies inside its texture; the PNG has that size.
    #[serde(default)]
    pub(super) clipped: bool,
}

/// Directory of graphic write specs: `archives/` and `gim-archives/` hold one archive per
/// file, applied in file-name order.
pub const BUILD_DIR: &str = "config/graphics-build";

/// Read every archive spec file; returns the manifest and per-file hashes.
pub(super) fn read_manifest(root: &Path) -> Result<Option<(Manifest, Value)>> {
    let dir = root.join(BUILD_DIR);
    if !dir.exists() {
        return Ok(None);
    }
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(&dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        ensure!(
            name == "archives" || name == "gim-archives",
            "unexpected {BUILD_DIR}/{name}"
        );
    }
    let mut manifest = Manifest {
        archives: Vec::new(),
        gim_archives: Vec::new(),
    };
    for kind in ["archives", "gim-archives"] {
        let sub = dir.join(kind);
        if !sub.exists() {
            continue;
        }
        let mut names: Vec<_> = fs::read_dir(&sub)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<_>>()?;
        names.sort();
        for name in names {
            ensure!(name.ends_with(".json"), "unexpected {kind}/{name}");
            let raw = fs::read(sub.join(&name))?;
            if kind == "archives" {
                manifest
                    .archives
                    .push(serde_json::from_slice(&raw).with_context(|| format!("{kind}/{name}"))?);
            } else {
                manifest
                    .gim_archives
                    .push(serde_json::from_slice(&raw).with_context(|| format!("{kind}/{name}"))?);
            }
            files.insert(format!("{kind}/{name}"), sha256(&raw));
        }
    }
    ensure!(!manifest.archives.is_empty(), "no graphic archives");
    Ok(Some((manifest, json!(files))))
}
