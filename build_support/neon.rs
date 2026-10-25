//! Build-time neon sign for the boot splash: クロック in glowing tubes.
//!
//! The sign is stored as one 8-bit intensity per pixel; at runtime it goes
//! through `RAMP` (black → deep magenta → pink → white-hot), scaled by the
//! sign's brightness, so the tubes can flicker on without more artwork.
//!
//! Emits `$OUT_DIR/neon.rs` (included by `src/sprites.rs`).

use std::fmt::Write as _;
use std::path::Path;

use super::assetpack::Pack;
use super::segments::blur;

pub const SIGN_W: usize = 460;
pub const SIGN_H: usize = 190;
/// Width the lettering's ink is scaled to.
const INK_W: f32 = 390.0;
const TEXT: &str = "クロック";

type Rgb = [f32; 3];

/// Tube color for intensity `i` (0-1): the halo is magenta, the tubes pink,
/// the hottest middle of each tube near white.
fn ramp(i: f32) -> Rgb {
    const STOPS: [(f32, Rgb); 6] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.08, [34.0, 0.0, 26.0]),
        (0.3, [150.0, 8.0, 104.0]),
        (0.6, [255.0, 40.0, 170.0]),
        (0.85, [255.0, 140.0, 215.0]),
        (1.0, [255.0, 238.0, 250.0]),
    ];
    for w in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if i <= b {
            let t = ((i - a) / (b - a)).clamp(0.0, 1.0);
            return [0, 1, 2].map(|k| ca[k] + (cb[k] - ca[k]) * t);
        }
    }
    STOPS[5].1
}

/// Coverage of the lettering, its ink centered in the sign.
fn lettering(font: &fontdue::Font) -> Vec<f32> {
    let layout = |px: f32| {
        let mut pen = 0.0f32;
        let (mut x0, mut x1, mut top, mut bot) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        let mut glyphs = Vec::new();
        for ch in TEXT.chars() {
            let (m, a) = font.rasterize(ch, px);
            let (gx, gy) = (pen + m.xmin as f32, -(m.ymin as f32 + m.height as f32));
            (x0, x1) = (x0.min(gx), x1.max(gx + m.width as f32));
            (top, bot) = (top.min(gy), bot.max(gy + m.height as f32));
            pen += m.advance_width;
            glyphs.push((gx, gy, m, a));
        }
        (glyphs, x0, x1, top, bot)
    };
    let (.., x0, x1, _, _) = layout(100.0);
    let (glyphs, x0b, x1b, top, bot) = layout(100.0 * INK_W / (x1 - x0));
    let ox = ((SIGN_W as f32 - (x1b - x0b)) / 2.0 - x0b).round() as i32;
    let oy = ((SIGN_H as f32 - (bot - top)) / 2.0 - top).round() as i32;
    let mut cov = vec![0f32; SIGN_W * SIGN_H];
    for (gx, gy, m, a) in glyphs {
        for row in 0..m.height {
            for col in 0..m.width {
                let (x, y) = (ox + gx as i32 + col as i32, oy + gy as i32 + row as i32);
                if (0..SIGN_W as i32).contains(&x) && (0..SIGN_H as i32).contains(&y) {
                    let p = &mut cov[y as usize * SIGN_W + x as usize];
                    *p = p.max(a[row * m.width + col] as f32 / 255.0);
                }
            }
        }
    }
    cov
}

/// Intensity per pixel: tubes with a hot core, plus a near and a wide halo.
fn sign(cov: &[f32]) -> Vec<u8> {
    let mut near = cov.to_vec();
    blur(&mut near, SIGN_W, SIGN_H, 3);
    let mut wide = cov.to_vec();
    blur(&mut wide, SIGN_W, SIGN_H, 13);
    cov.iter()
        .zip(&near)
        .zip(&wide)
        .map(|((&c, &n), &w)| {
            let core = c * (0.8 + 0.2 * ((n - 0.4) / 0.35).clamp(0.0, 1.0));
            let halo = (1.1 * n + 2.4 * w).min(1.0) * 0.62;
            (core.max(halo) * 255.0).round() as u8
        })
        .collect()
}

pub fn generate(manifest_dir: &Path, out_dir: &Path, pack: &mut Pack, font_file: &str) {
    let path = manifest_dir.join(font_file);
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
        .unwrap_or_else(|e| panic!("parse {font_file}: {e}"));

    let sign = pack.add(&sign(&lettering(&font)));
    let ramp: Vec<String> = (0..256)
        .map(|i| {
            let [r, g, b] = ramp(i as f32 / 255.0).map(|v| v.round().clamp(0.0, 255.0) as u32);
            format!("0x{:06X}", r << 16 | g << 8 | b)
        })
        .collect();

    let mut src = String::new();
    writeln!(src, "pub const SIGN_W: usize = {SIGN_W};\npub const SIGN_H: usize = {SIGN_H};").unwrap();
    writeln!(src, "/// クロック, 8-bit intensity per pixel, row-major.\npub static SIGN: crate::assets::Blob = {sign};").unwrap();
    writeln!(src, "/// Color per intensity at full brightness, 0xRRGGBB.\npub static RAMP: [u32; 256] = [{}];", ramp.join(", ")).unwrap();
    std::fs::write(out_dir.join("neon.rs"), src).unwrap();
}
