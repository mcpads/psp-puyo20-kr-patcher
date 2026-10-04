//! Auto-hinted glyph cells: skrifa's port of the FreeType auto-hinter (light target) and
//! zeno coverage rasterization. Fonts without TrueType instructions render with blurred
//! stems unhinted; the auto-hinter aligns them to the pixel grid like desktop previews.
use anyhow::{Context, Result, ensure};
use skrifa::{
    MetadataProvider,
    instance::{LocationRef, Size},
    outline::{
        DrawSettings, Engine, HintingInstance, HintingOptions, OutlineGlyphCollection, OutlinePen,
        SmoothMode, Target,
    },
    raw::FontRef,
};
use zeno::{Command, Format, Mask, Transform, Vector};

struct Path(Vec<Command>);

/// Ink bounding box `(x, y, w, h)` and its row-major coverage.
pub(crate) type Placed = (i32, i32, usize, usize, Vec<u8>);

// Font units point up; the cell's rows point down.
impl OutlinePen for Path {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(Command::MoveTo(Vector::new(x, -y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(Command::LineTo(Vector::new(x, -y)));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0
            .push(Command::QuadTo(Vector::new(cx, -cy), Vector::new(x, -y)));
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.0.push(Command::CurveTo(
            Vector::new(c0x, -c0y),
            Vector::new(c1x, -c1y),
            Vector::new(x, -y),
        ));
    }
    fn close(&mut self) {
        self.0.push(Command::Close);
    }
}

pub(crate) struct Hinted<'a> {
    font: FontRef<'a>,
    outlines: OutlineGlyphCollection<'a>,
    instance: HintingInstance,
}

impl<'a> Hinted<'a> {
    pub(crate) fn new(data: &'a [u8], px: f32) -> Result<Self> {
        let font = FontRef::new(data).map_err(|e| anyhow::anyhow!("font: {e}"))?;
        let outlines = font.outline_glyphs();
        let options = HintingOptions {
            engine: Engine::Auto(None),
            target: Target::Smooth {
                mode: SmoothMode::Light,
                symmetric_rendering: false,
                preserve_linear_metrics: true,
            },
        };
        let instance =
            HintingInstance::new(&outlines, Size::new(px), LocationRef::default(), options)
                .map_err(|e| anyhow::anyhow!("hinting: {e}"))?;
        Ok(Self {
            font,
            outlines,
            instance,
        })
    }

    /// Coverage of `c` drawn with the pen at (`x`, `baseline`) in a `w`x`h` canvas, as its
    /// ink bounding box `(x, y, w, h, coverage)`; `None` for blank glyphs.
    pub(crate) fn placed(
        &self,
        c: char,
        x: i32,
        baseline: i32,
        (w, h): (usize, usize),
    ) -> Result<Option<Placed>> {
        let id = self.font.charmap().map(c).context("missing font glyph")?;
        let glyph = self.outlines.get(id).context("missing outline")?;
        let mut path = Path(Vec::new());
        glyph
            .draw(DrawSettings::hinted(&self.instance, false), &mut path)
            .map_err(|e| anyhow::anyhow!("draw {c}: {e}"))?;
        if path.0.is_empty() {
            return Ok(None);
        }
        let mut buf = vec![0u8; w * h];
        Mask::new(&path.0)
            .size(w as u32, h as u32)
            .format(Format::Alpha)
            .transform(Some(Transform::translation(x as f32, baseline as f32)))
            .render_into(&mut buf, None);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for (i, &a) in buf.iter().enumerate() {
            if a > 0 {
                let (px, py) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(px), y0.min(py), x1.max(px + 1), y1.max(py + 1));
            }
        }
        if x1 == 0 {
            return Ok(None);
        }
        let cov = (y0..y1)
            .flat_map(|yy| buf[yy * w + x0..yy * w + x1].to_vec())
            .collect();
        Ok(Some((x0 as i32, y0 as i32, x1 - x0, y1 - y0, cov)))
    }

    /// 14x14 coverage with the pen origin at (`x_shift`, `baseline`).
    pub(crate) fn cell(&self, c: char, x_shift: i32, baseline: i32) -> Result<[u8; 196]> {
        let id = self.font.charmap().map(c).context("missing font glyph")?;
        let glyph = self.outlines.get(id).context("missing outline")?;
        let mut path = Path(Vec::new());
        glyph
            .draw(DrawSettings::hinted(&self.instance, false), &mut path)
            .map_err(|e| anyhow::anyhow!("draw {c}: {e}"))?;
        // Render with a margin so ink outside the cell is detected, not clipped.
        const M: usize = 8;
        let mut big = vec![0u8; (14 + 2 * M) * (14 + 2 * M)];
        Mask::new(&path.0)
            .size((14 + 2 * M) as u32, (14 + 2 * M) as u32)
            .format(Format::Alpha)
            .transform(Some(Transform::translation(
                (x_shift + M as i32) as f32,
                (baseline + M as i32) as f32,
            )))
            .render_into(&mut big, None);
        let mut cell = [0u8; 196];
        for (i, &a) in big.iter().enumerate() {
            let (x, y) = (i % (14 + 2 * M), i / (14 + 2 * M));
            let inside = (M..M + 14).contains(&x) && (M..M + 14).contains(&y);
            if inside {
                cell[(y - M) * 14 + x - M] = a;
            } else {
                ensure!(a == 0, "glyph bounds {c}");
            }
        }
        Ok(cell)
    }
}
