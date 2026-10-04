use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "PSP Puyo 20th fixed-source development tooling")]
pub(super) struct Cli {
    #[arg(long, default_value = ".")]
    pub(super) project: PathBuf,
    #[command(subcommand)]
    pub(super) command: Command,
}
#[derive(Subcommand)]
pub(super) enum Command {
    /// Rebuild all adopted Korean assets and the ISO from source; no work/ inputs required.
    Build {
        source: PathBuf,
        #[arg(long)]
        ppsspp_root: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Audit the JP/EN population against a verified Korean build, read-only.
    AuditScope {
        source: PathBuf,
        #[arg(long)]
        product: PathBuf,
        #[arg(long)]
        inventory: PathBuf,
        #[arg(long)]
        graphics: PathBuf,
        #[arg(long)]
        build_report: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Analysis bridge: stdin begins at an oo_disk_image index header.
    ArchiveIndex,
    /// Decrypt the fixed Japanese EBOOT and verify ELF addresses against original RAM.
    ExtractExecutable {
        source: PathBuf,
        #[arg(long)]
        ppsspp_root: PathBuf,
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Extract pinned story scripts and candidate MTX references for context review.
    ExtractStoryContext {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect pinned original PSP code with retro-typed-isa Allegrex semantics.
    InspectTextCode {
        snapshot: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Extract all 96 pinned FNT/MTX pairs without guessing unknown token operands.
    ExtractText {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Analysis bridge: stdin is font length (u32 LE), FNT bytes, then MTX bytes.
    TextPair,
    /// Inspect stored named SNC draws; does not evaluate animation tracks.
    SncDraws {
        #[arg(long)]
        snc: PathBuf,
    },
    /// Box resampling for a draft region, preserving the source GIM color format.
    PrepareRegion {
        /// Restrict input to x,y,width,height before alpha trimming and fitting.
        #[arg(long, value_delimiter = ',', num_args = 4)]
        source_rect: Option<Vec<usize>>,
        /// Place the trimmed image at the left two-pixel margin.
        #[arg(long)]
        left_align: bool,
        /// Restrict a draft to its most frequent prepared colors.
        #[arg(long)]
        palette_colors: Option<usize>,
        /// Restrict matching to these original RGBA colors; repeat for each color.
        #[arg(long, num_args = 4, conflicts_with = "palette_colors")]
        palette_rgba: Vec<u8>,
        /// JSON guide of repeated two-color source regions for generated pattern alignment.
        #[arg(long, requires = "protect_base", conflicts_with_all = ["trim_alpha", "match_backing_boundary", "palette_colors", "palette_rgba"])]
        pattern_guide: Option<PathBuf>,
        /// Match the closest original palette alpha before matching RGB.
        #[arg(long, conflicts_with = "palette_colors")]
        match_alpha: bool,
        /// Correct a text-free opaque backing from the protected original boundary.
        #[arg(long, requires = "protect_base", conflicts_with = "palette_colors")]
        match_backing_boundary: bool,
        /// Trim margins below alpha 16 and fit proportionally with two-pixel padding.
        #[arg(long)]
        trim_alpha: bool,
        /// Padding after alpha trimming (default 2); match the source sprite's native margin.
        #[arg(long, requires = "trim_alpha")]
        trim_padding: Option<usize>,
        /// Key a black-ground draft: brightest channel <= LOW is transparent, ramping to HIGH.
        #[arg(long, value_delimiter = ',', num_args = 2)]
        key_black: Option<Vec<u8>>,
        /// With --key-black, clear visible pieces under a tenth of the largest piece.
        #[arg(long, requires = "key_black")]
        drop_specks: bool,
        /// Keep this original image outside the edited rectangles (cropped at --protect-origin).
        #[arg(long)]
        protect_base: Option<PathBuf>,
        #[arg(long, value_delimiter = ',', num_args = 2, requires = "protect_base")]
        protect_origin: Option<Vec<usize>>,
        /// Edited x,y,width,height; repeatable. Without it the draft/base difference box is used.
        #[arg(long, value_delimiter = ',', num_args = 4, action = clap::ArgAction::Append, requires = "protect_base")]
        allowed_rect: Vec<usize>,
        #[arg(long)]
        image: PathBuf,
        #[arg(long)]
        gim: PathBuf,
        #[arg(long)]
        width: usize,
        #[arg(long)]
        height: usize,
        #[arg(long)]
        output: PathBuf,
    },
    /// Render outlined display lettering drafts from a spec (RGBA, before palette fit).
    RenderDisplay {
        spec: PathBuf,
        /// Write outputs under this directory instead of their spec paths.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Palette-fit each item's `adopt` entries into their adopted PNGs.
        #[arg(long)]
        adopt: bool,
    },
    /// Render 1-bit fixed-font words into standalone GIM texture rectangles from a spec.
    RenderGimRects {
        spec: PathBuf,
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Copy each member's rendered texture to its `adopt.png`.
        #[arg(long)]
        adopt: bool,
    },
    /// Apply reviewed wording to translation JSON without touching source controls.
    EditTranslation {
        /// JSON list of {file, path, old, new}; "/" marks source line breaks.
        fixes: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Split one translation file into an index and per-group batch files; updates the config.
    SplitTranslation {
        config: PathBuf,
        /// JSON Pointer of the object holding "translation" (empty for the config root).
        #[arg(long, default_value = "")]
        pointer: String,
    },
    /// Check hash pins in config/ against repository files; update pins of the named files.
    Pins {
        #[arg(long, num_args = 1..)]
        update: Vec<PathBuf>,
        /// Add missing pins beside every config reference to these files.
        #[arg(long, num_args = 1..)]
        adopt: Vec<PathBuf>,
    },
    /// Render pinned text into isolated SNC cells using a fixed font and palette.
    PrepareLabels {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Merge prepared standalone GIM pixels inside pinned rectangles.
    PrepareGim {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Prepare a lossy PSP DXT5 candidate with explicit 4x4 block boundaries.
    PrepareDxt {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Render styled multiline text while preserving graphics outside text rectangles.
    PreparePanels {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Compose {
        #[arg(long)]
        inputs: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    ReinsertMenu {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        preview_report: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Inspect stored SNC cell rectangles; optionally require isolated edit cells.
    SncCells {
        #[arg(long)]
        snt: PathBuf,
        #[arg(long)]
        snc: PathBuf,
        #[arg(long, value_delimiter = ',')]
        cells: Option<Vec<usize>>,
    },
    Graphics {
        #[arg(value_parser=["decode", "encode", "snt-table"])]
        operation: String,
    },
    VerifySource {
        source: PathBuf,
    },
    InspectMenu {
        source: PathBuf,
    },
    /// Export the verified menu SNT and its original GIM/PNG textures.
    ExtractMenu {
        source: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    BuildMenu {
        source: PathBuf,
        /// Replace the original FNT copy-length addition with subtraction in a plain ELF.
        #[arg(long, requires = "ppsspp_root")]
        fix_font_copy: bool,
        #[arg(long, requires = "fix_font_copy")]
        ppsspp_root: Option<PathBuf>,
        #[arg(long)]
        descriptions: bool,
        /// Grow the general story font and relocate its unchanged MTX within their joint span.
        #[arg(long, requires = "fix_font_copy")]
        expand_story_font: bool,
        /// Insert all 26 aligned opening dialogue drafts using the expanded story font.
        #[arg(long, requires = "expand_story_font")]
        opening: bool,
        /// Insert all adopted story groups from config/story-build.json.
        #[arg(long, requires = "fix_font_copy", conflicts_with_all = ["opening", "expand_story_font"])]
        story: bool,
        /// Empty /PSP_GAME/INSDIR and truncate the image at the install data it held.
        #[arg(long)]
        remove_install_data: bool,
        #[arg(long)]
        snt_report: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}
