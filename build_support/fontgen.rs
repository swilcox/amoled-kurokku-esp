//! Build-time font rasterizer: TTF -> 4-bit alpha glyph atlases.
//!
//! For each `FontSpec`, every requested character is rasterized with
//! `fontdue` at a fixed pixel size and packed two pixels per byte (high
//! nibble first, rows padded to a whole byte). The output is
//! `$OUT_DIR/fonts.rs` (one `Font` static per spec, included by
//! `src/fonts.rs`) plus one `.a4` data blob per font.

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

pub fn generate(manifest_dir: &Path, out_dir: &Path, specs: &[FontSpec]) {
    let mut src = String::new();
    for spec in specs {
        let path = manifest_dir.join(spec.file);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let settings = fontdue::FontSettings { scale: spec.px, ..Default::default() };
        let font = fontdue::Font::from_bytes(bytes, settings)
            .unwrap_or_else(|e| panic!("parse {}: {e}", spec.file));
        let lm = font.horizontal_line_metrics(spec.px).expect("font has no horizontal metrics");

        let mut chars: Vec<char> = spec.chars.chars().collect();
        chars.sort_unstable();
        chars.dedup();

        let mut data = Vec::new();
        let mut glyphs = String::new();
        for ch in chars {
            assert!(
                ch == ' ' || font.lookup_glyph_index(ch) != 0,
                "{} has no glyph for {ch:?}",
                spec.file
            );
            let (m, alpha) = font.rasterize(ch, spec.px);
            let offset = data.len();
            for row in alpha.chunks(m.width.max(1)).take(m.height) {
                for pair in row.chunks(2) {
                    let hi = quantize(pair[0]);
                    let lo = pair.get(1).copied().map_or(0, quantize);
                    data.push(hi << 4 | lo);
                }
            }
            // fontdue's ymin is the bitmap bottom relative to the baseline,
            // y-up. `top` is the bitmap top relative to the baseline, y-down.
            let top = -(m.ymin + m.height as i32);
            writeln!(
                glyphs,
                "        Glyph {{ ch: {ch:?}, w: {}, h: {}, left: {}, top: {top}, advance: {}, offset: {offset} }},",
                m.width,
                m.height,
                m.xmin,
                m.advance_width.round() as i32,
            )
            .unwrap();
        }

        let blob = format!("{}.a4", spec.name.to_lowercase());
        std::fs::write(out_dir.join(&blob), &data).unwrap();
        writeln!(
            src,
            "pub static {}: Font = Font {{\n    px: {},\n    ascent: {},\n    descent: {},\n    glyphs: &[\n{glyphs}    ],\n    data: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{blob}\")),\n}};\n",
            spec.name,
            spec.px.round() as u16,
            lm.ascent.round() as i32,
            lm.descent.round() as i32,
        )
        .unwrap();
    }
    std::fs::write(out_dir.join("fonts.rs"), src).unwrap();
}

fn quantize(a: u8) -> u8 {
    ((a as u16 * 15 + 127) / 255) as u8
}
