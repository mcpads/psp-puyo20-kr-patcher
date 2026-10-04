//! CLI dispatch; product writes are owned by the library build pipeline.
mod args;
mod bridges;
use anyhow::{Context, Result};
use args::{Cli, Command};
use clap::Parser;
use puyo20_tool::{
    menu,
    source::{self, SourceProfile},
};
use serde_json::json;
use std::fs;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Build {
            source,
            ppsspp_root,
            output,
        } => {
            let report = menu::build_current(&cli.project, source, ppsspp_root, output)?;
            println!(
                "{}",
                json!({"output":output,"sha256":report["output_sha256"],"size_bytes":report["size_bytes"],"writes":report["writes"].as_array().context("writes")?.len()})
            );
        }
        Command::AuditScope {
            source,
            product,
            inventory,
            graphics,
            build_report,
            output,
        } => {
            println!(
                "{}",
                puyo20_tool::scope::audit(
                    &cli.project,
                    source,
                    product,
                    inventory,
                    graphics,
                    build_report,
                    output
                )?
            );
        }
        Command::ExtractExecutable {
            source,
            ppsspp_root,
            snapshot,
            output,
        } => {
            let report = puyo20_tool::executable::extract(
                &cli.project,
                source,
                ppsspp_root,
                snapshot,
                output,
            )?;
            println!(
                "ELF extracted; {} relocation entries, {} verified RAM correspondences",
                report["relocation_entry_count"],
                report["proofs"].as_array().context("proofs")?.len()
            );
        }
        Command::ExtractStoryContext { source, output } => {
            let report = puyo20_tool::story::extract(&cli.project, source, output)?;
            println!(
                "{}",
                json!({"route_archives":report["totals"],
                "general_scripts":report["general"]["scripts"].as_array().context("general scripts")?.len(),
                "general_calls":report["general"]["calls"]})
            );
        }
        Command::InspectTextCode { snapshot, output } => {
            let report = puyo20_tool::code::inspect(&cli.project, snapshot, output)?;
            println!(
                "{} addressed Allegrex instructions verified; {} unresolved words",
                report["decoded_instruction_count"], report["unresolved_word_count"]
            );
        }
        Command::ExtractText { source, output } => {
            let report = puyo20_tool::text::extract(&cli.project, source, output)?;
            println!("{}", report["totals"]);
        }
        Command::ArchiveIndex => bridges::archive_index()?,
        Command::TextPair => bridges::text_pair()?,
        Command::SncDraws { snc } => {
            println!("{}", puyo20_tool::snc::named_draws(&fs::read(snc)?)?);
        }
        Command::SncCells { snt, snc, cells } => {
            println!(
                "{}",
                serde_json::to_string(&puyo20_tool::snc::inspect(
                    &fs::read(snt)?,
                    &fs::read(snc)?,
                    cells.as_deref()
                )?)?
            );
        }
        Command::Graphics { operation } => bridges::graphics(operation)?,
        Command::PrepareRegion {
            source_rect,
            left_align,
            palette_colors,
            palette_rgba,
            pattern_guide,
            match_alpha,
            match_backing_boundary,
            trim_alpha,
            trim_padding,
            key_black,
            drop_specks,
            protect_base,
            protect_origin,
            allowed_rect,
            image,
            gim,
            width,
            height,
            output,
        } => {
            let protect = protect_base
                .as_ref()
                .map(|base| puyo20_tool::prepare::Protect {
                    base: base.clone(),
                    origin: protect_origin.as_ref().map_or([0, 0], |o| [o[0], o[1]]),
                    rects: allowed_rect
                        .chunks(4)
                        .map(|r| [r[0], r[1], r[2], r[3]])
                        .collect(),
                });
            println!(
                "{}",
                puyo20_tool::prepare::region(
                    image,
                    gim,
                    [*width, *height],
                    puyo20_tool::prepare::RegionOptions {
                        trim_alpha: *trim_alpha,
                        trim_padding: *trim_padding,
                        palette_colors: *palette_colors,
                        palette_rgba: palette_rgba.as_chunks::<4>().0.to_vec(),
                        pattern_guide: pattern_guide.clone(),
                        match_alpha: *match_alpha,
                        match_backing_boundary: *match_backing_boundary,
                        left_align: *left_align,
                        source_rect: source_rect.as_ref().map(|v| [v[0], v[1], v[2], v[3]]),
                        protect,
                        key_black: key_black.as_ref().map(|v| [v[0], v[1]]),
                        drop_specks: *drop_specks,
                    },
                    output
                )?
            );
        }
        Command::RenderDisplay {
            spec,
            out_dir,
            adopt,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&puyo20_tool::display_text::render(
                    &cli.project,
                    spec,
                    out_dir.as_deref(),
                    *adopt
                )?)?
            );
        }
        Command::RenderGimRects {
            spec,
            out_dir,
            adopt,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&puyo20_tool::gim_text::render(
                    &cli.project,
                    spec,
                    out_dir.as_deref(),
                    *adopt
                )?)?
            );
        }
        Command::EditTranslation { fixes, dry_run } => {
            let fixes: Vec<puyo20_tool::translation_edit::Fix> =
                serde_json::from_slice(&fs::read(fixes)?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&puyo20_tool::translation_edit::run(
                    &cli.project,
                    &fixes,
                    *dry_run
                )?)?
            );
        }
        Command::SplitTranslation { config, pointer } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&puyo20_tool::translation_batches::split(
                    &cli.project,
                    config,
                    pointer
                )?)?
            );
        }
        Command::Pins { update, adopt } => {
            if !adopt.is_empty() {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&puyo20_tool::pins::adopt(&cli.project, adopt)?)?
                );
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&puyo20_tool::pins::run(&cli.project, update)?)?
            );
        }
        Command::PrepareDxt { manifest, output } => {
            println!("{}", puyo20_tool::prepare::dxt(manifest, output)?);
        }
        Command::PrepareGim { manifest, output } => {
            println!(
                "{}",
                puyo20_tool::graphic_archives::prepare_gim(&cli.project, manifest, output)?
            );
        }
        Command::PrepareLabels { manifest, output } => {
            println!(
                "{}",
                puyo20_tool::label_graphics::prepare(&cli.project, manifest, output)?
            );
        }
        Command::PreparePanels { manifest, output } => {
            println!(
                "{}",
                puyo20_tool::panel_graphics::prepare(&cli.project, manifest, output)?
            );
        }
        Command::Compose { inputs, output } => {
            println!(
                "{}",
                puyo20_tool::authoring::compose(&cli.project, inputs, output)?
            );
        }
        Command::ReinsertMenu {
            source,
            preview_report,
            output,
        } => {
            println!(
                "{}",
                puyo20_tool::authoring::reinsert(&cli.project, source, preview_report, output)?
            );
        }
        Command::VerifySource { source } => {
            let profile: SourceProfile =
                serde_json::from_slice(&fs::read(cli.project.join("config/source.json"))?)?;
            source::verify(source, &profile)?;
            println!(
                "{}",
                json!({"sha256":profile.sha256,"size_bytes":profile.size_bytes,"verified":true})
            );
        }
        Command::InspectMenu { source } => println!(
            "{}",
            serde_json::to_string_pretty(&menu::inspect(&cli.project, source)?)?
        ),
        Command::ExtractMenu { source, output } => {
            println!("{}", menu::extract(&cli.project, source, output)?)
        }
        Command::BuildMenu {
            source,
            snt_report,
            output,
            descriptions,
            expand_story_font,
            opening,
            story,
            fix_font_copy,
            ppsspp_root,
            remove_install_data,
        } => {
            let report = menu::build(
                &cli.project,
                source,
                menu::BuildOptions {
                    snt_report,
                    output,
                    descriptions: *descriptions,
                    expand_story_font: *expand_story_font,
                    opening: *opening,
                    story: *story,
                    fix_font_copy: *fix_font_copy,
                    ppsspp_root: ppsspp_root.as_deref(),
                    remove_install_data: *remove_install_data,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}
