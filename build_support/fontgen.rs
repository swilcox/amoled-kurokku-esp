//! Build-time glyph atlases: 4-bit alpha bitmaps, two pixels per byte (high
//! nibble first, rows padded to a whole byte).
//!
//! Glyphs come from TTFs (rasterized with `fontdue`) or from shapes drawn by
//! other build-support modules (e.g. 7-segment LED segments). The output is
//! `$OUT_DIR/fonts.rs` (one `Font` static per set, included by
//! `src/fonts.rs`); bitmap data goes into the asset pack.

use std::fmt::Write as _;
use std::path::Path;

pub struct FontSpec {
    /// Name of the generated `static`.
    pub name: &'static str,
    /// TTF path relative to the crate root.
    pub file: &'static str,
    pub px: f32,
    pub chars: &'static str,
}

/// One rasterized glyph with 8-bit coverage.
pub struct RawGlyph {
    pub ch: char,
    pub w: usize,
    pub h: usize,
    /// Bitmap left edge relative to the pen position.
    pub left: i32,
    /// Bitmap top relative to the baseline, y-down.
    pub top: i32,
    pub advance: i32,
    /// Row-major coverage, `w * h` bytes.
    pub alpha: Vec<u8>,
}

/// A complete glyph set ready to emit.
pub struct GlyphSet {
    pub name: String,
    pub px: u16,
    pub ascent: i32,
    pub descent: i32,
    pub glyphs: Vec<RawGlyph>,
}

/// Rasterize `spec`'s characters from its TTF.
pub fn from_ttf(manifest_dir: &Path, spec: &FontSpec) -> GlyphSet {
    let path = manifest_dir.join(spec.file);
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let settings = fontdue::FontSettings { scale: spec.px, ..Default::default() };
    let font = fontdue::Font::from_bytes(bytes, settings).unwrap_or_else(|e| panic!("parse {}: {e}", spec.file));
    let lm = font.horizontal_line_metrics(spec.px).expect("font has no horizontal metrics");

    let glyphs = spec
        .chars
        .chars()
        .map(|ch| {
            assert!(ch == ' ' || font.lookup_glyph_index(ch) != 0, "{} has no glyph for {ch:?}", spec.file);
            let (m, alpha) = font.rasterize(ch, spec.px);
            RawGlyph {
                ch,
                w: m.width,
                h: m.height,
                left: m.xmin,
                // fontdue's ymin is the bitmap bottom relative to the
                // baseline, y-up.
                top: -(m.ymin + m.height as i32),
                advance: m.advance_width.round() as i32,
                alpha,
            }
        })
        .collect();
    GlyphSet {
        name: spec.name.to_string(),
        px: spec.px.round() as u16,
        ascent: lm.ascent.round() as i32,
        descent: lm.descent.round() as i32,
        glyphs,
    }
}

/// Write `fonts.rs` and the `.a4` blobs for all sets.
pub fn emit(out_dir: &Path, pack: &mut super::assetpack::Pack, sets: Vec<GlyphSet>) {
    let mut src = String::new();
    for mut set in sets {
        set.glyphs.sort_by_key(|g| g.ch);
        set.glyphs.dedup_by_key(|g| g.ch);

        let mut data = Vec::new();
        let mut glyphs = String::new();
        for g in &set.glyphs {
            let offset = data.len();
            for row in g.alpha.chunks(g.w.max(1)).take(g.h) {
                for pair in row.chunks(2) {
                    let hi = quantize(pair[0]);
                    let lo = pair.get(1).copied().map_or(0, quantize);
                    data.push(hi << 4 | lo);
                }
            }
            writeln!(
                glyphs,
                "        Glyph {{ ch: {:?}, w: {}, h: {}, left: {}, top: {}, advance: {}, offset: {offset} }},",
                g.ch, g.w, g.h, g.left, g.top, g.advance,
            )
            .unwrap();
        }

        let blob = pack.add(&data);
        writeln!(
            src,
            "pub static {}: Font = Font {{\n    px: {},\n    ascent: {},\n    descent: {},\n    glyphs: &[\n{glyphs}    ],\n    data: {blob},\n}};\n",
            set.name, set.px, set.ascent, set.descent,
        )
        .unwrap();
    }
    std::fs::write(out_dir.join("fonts.rs"), src).unwrap();
}

fn quantize(a: u8) -> u8 {
    ((a as u16 * 15 + 127) / 255) as u8
}
