//! Build configured story glyphs and remap their MTX within a verified archive span.
use crate::{
    archive_index::{Index, Relocation},
    graphics, sha256, source, text,
    write_plan::ExpectedWrite,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};
mod glyphs;

pub struct Plan {
    pub write: ExpectedWrite,
    pub receipt: Value,
    changes: Vec<Relocation>,
    game_offset: u64,
    game_size: u64,
}

impl Plan {
    pub fn relocate_index(&self, original: &[u8]) -> Result<Vec<u8>> {
        let index = Index::parse(original)?;
        let start = self
            .write
            .offset
            .checked_sub(self.game_offset)
            .context("GAME joint start")?;
        let end = start + self.write.before.len() as u64;
        let overlaps: Vec<_> = index
            .records
            .iter()
            .filter(|r| {
                u64::from(r.offset) < end && u64::from(r.offset) + u64::from(r.size_bytes) > start
            })
            .collect();
        ensure!(
            overlaps.len() == self.changes.len()
                && overlaps.iter().all(|r| self
                    .changes
                    .iter()
                    .any(|c| c.record_offset == r.record_offset)),
            "joint font span contains another indexed asset"
        );
        crate::archive_index::relocate(original, &self.changes, self.game_size)
    }
}

pub fn plan(
    root: &Path,
    iso: &mut fs::File,
    config_path: &Path,
    translation: Option<&Path>,
) -> Result<Plan> {
    let raw = fs::read(root.join(config_path))?;
    let config: Value = serde_json::from_slice(&raw)?;
    let compact = match config["glyph_policy"].as_str().unwrap_or("preserve_source") {
        "preserve_source" => false,
        "translation_only" => true,
        _ => anyhow::bail!("unknown story glyph policy"),
    };
    ensure!(
        !compact || translation.is_some(),
        "compact font requires text remapping"
    );
    let layout: Value = serde_json::from_slice(&fs::read(root.join("config/menu-build.json"))?)?;
    let game_offset = config["game_offset"].as_u64().context("GAME offset")?;
    let game_size = config["game_size"].as_u64().context("GAME size")?;
    ensure!(
        layout["game_iso_offset"] == game_offset && layout["game_size"] == game_size,
        "story GAME layout drift"
    );
    let catalog: text::Catalog =
        serde_json::from_slice(&fs::read(root.join("config/text-resources.json"))?)?;
    let profile: source::SourceProfile =
        serde_json::from_slice(&fs::read(root.join("config/source.json"))?)?;
    ensure!(
        catalog.source_sha256 == profile.sha256,
        "story source drift"
    );
    let pair = catalog
        .pairs
        .iter()
        .find(|p| config["pair"] == p.name)
        .context("story pair")?;
    let offset = config["joint_offset"].as_u64().context("joint offset")?;
    let length = usize::try_from(config["joint_size"].as_u64().context("joint size")?)?;
    ensure!(
        offset >= game_offset
            && offset
                .checked_add(length as u64)
                .context("joint overflow")?
                <= game_offset + game_size
            && offset.is_multiple_of(2048)
            && length.is_multiple_of(2048),
        "joint GAME range"
    );
    let before = source::read_extent(iso, offset, length)?;
    let font_start = usize::try_from(
        pair.font
            .offset
            .checked_sub(offset)
            .context("FNT placement")?,
    )?;
    let text_start = usize::try_from(
        pair.text
            .offset
            .checked_sub(offset)
            .context("MTX placement")?,
    )?;
    ensure!(
        font_start == 0 && pair.font.size <= text_start,
        "source pair ordering"
    );
    let fnt = graphics::bytes(&before, font_start, pair.font.size)?;
    let mtx = graphics::bytes(&before, text_start, pair.text.size)?;
    ensure!(
        sha256(fnt) == pair.font.sha256 && sha256(mtx) == pair.text.sha256,
        "joint source assets mismatch"
    );
    let mut preserved = Vec::new();
    let mut spans = vec![
        (0, pair.font.size),
        (text_start, text_start + pair.text.size),
    ];
    if let Some(assets) = config.get("preserved_assets") {
        for asset in assets.as_array().context("preserved assets")? {
            let record: crate::archive_index::Record =
                serde_json::from_value(asset["record"].clone())?;
            ensure!(
                record.member == u32::MAX,
                "preserved asset is not standalone"
            );
            let at = usize::try_from(
                (game_offset + u64::from(record.offset))
                    .checked_sub(offset)
                    .context("preserved asset placement")?,
            )?;
            let data = graphics::bytes(&before, at, record.size_bytes as usize)?;
            ensure!(
                asset["sha256"] == sha256(data),
                "preserved asset identity mismatch"
            );
            spans.push((at, at + data.len()));
            preserved.push((record, data));
        }
    }
    spans.sort_unstable();
    let mut end = 0;
    for (start, next) in spans {
        ensure!(
            start >= end && before[end..start].iter().all(|b| *b == 0),
            "source span overlap or nonzero padding"
        );
        end = next;
    }
    ensure!(
        before[end..].iter().all(|b| *b == 0),
        "nonzero trailing source padding"
    );
    let derived = translation
        .map(|path| crate::story_text::characters(root, path))
        .transpose()?;
    let built = glyphs::build(root, &config, fnt, compact, derived.as_deref())?;
    let new_fnt = &built.bytes;
    let (new_mtx, text_receipt) = if let Some(path) = translation {
        crate::story_text::build(root, path, &pair.name, fnt, new_fnt, mtx, compact)?
    } else {
        (mtx.to_vec(), Value::Null)
    };
    ensure!(new_mtx.len() == mtx.len(), "story text size changed");
    let new_text_start = new_fnt.len().div_ceil(2048) * 2048;
    ensure!(
        new_text_start + mtx.len().div_ceil(2048) * 2048 <= length,
        "joint font/text capacity exceeded"
    );
    let mut after = vec![0; length];
    after[..new_fnt.len()].copy_from_slice(new_fnt);
    after[new_text_start..new_text_start + mtx.len()].copy_from_slice(&new_mtx);
    let source_records = config["records"].as_array().context("source records")?;
    ensure!(source_records.len() == 2, "expected font/text records");
    let mut changes = Vec::new();
    for (i, (start, size)) in [(0, new_fnt.len()), (new_text_start, mtx.len())]
        .into_iter()
        .enumerate()
    {
        let r = &source_records[i];
        let extent = if i == 0 { &pair.font } else { &pair.text };
        ensure!(
            r["offset"].as_u64().context("record offset")? + game_offset == extent.offset
                && r["size_bytes"] == extent.size
                && r["member"] == u32::MAX,
            "story record identity"
        );
        changes.push(Relocation {
            record_offset: usize::try_from(
                r["record_offset"].as_u64().context("record location")?,
            )?,
            hash_path: serde_json::from_value(r["hash_path"].clone())?,
            before_offset: u32::try_from(extent.offset - game_offset)?,
            before_size: u32::try_from(extent.size)?,
            offset: u32::try_from(offset - game_offset + start as u64)?,
            size: u32::try_from(size)?,
        });
    }
    let mut next = new_text_start + mtx.len().div_ceil(2048) * 2048;
    let mut preserved_receipts = Vec::new();
    for (record, data) in preserved {
        let rounded = data.len().div_ceil(2048) * 2048;
        ensure!(
            next + rounded <= length,
            "preserved asset capacity exceeded"
        );
        after[next..next + data.len()].copy_from_slice(data);
        let new_offset = u32::try_from(offset - game_offset + next as u64)?;
        changes.push(Relocation {
            record_offset: record.record_offset,
            hash_path: record.hash_path.clone(),
            before_offset: record.offset,
            before_size: record.size_bytes,
            offset: new_offset,
            size: record.size_bytes,
        });
        preserved_receipts.push(json!({"record_offset":record.record_offset,"size":data.len(),"sha256":sha256(data),"before_offset":record.offset,"offset":new_offset,"bytes_preserved":after[next..next+data.len()] == *data}));
        next += rounded;
    }
    let receipt = json!({"scope":"configured story glyphs and optional prose translation; requires coupled verified EBOOT plan", "pair":pair.name,
        "glyph_policy":if compact {"translation_only"} else {"preserve_source"},"retained_source_glyphs":built.retained_source_glyphs,
        "preserved_assets":preserved_receipts,
        "preserved_glyphs":built.preserved_glyphs,
        "opening":text_receipt,
        "config_sha256":sha256(&raw),"characters_sha256":built.characters_sha256,"font_sha256":config["font_sha256"],
        "source_glyphs":built.source_glyphs,"added_glyphs":built.added_glyphs,"glyphs":built.glyphs,"height":built.height,
        "source_glyph_table_and_rasters_preserved":!compact,"mtx_bytes_preserved":new_mtx == mtx,"joint_original_sha256":sha256(&before),
        "font_sha256_output":sha256(new_fnt),"text_sha256_output":sha256(&new_mtx),"relocations":changes});
    Ok(Plan {
        write: ExpectedWrite {
            offset,
            before,
            after,
        },
        receipt,
        changes,
        game_offset,
        game_size,
    })
}
