//! Build-time nixie tube art.
//!
//! Emits `$OUT_DIR/nixie.rs` (included by `src/sprites.rs`) plus data blobs:
//! - one tube background (RGB565): dark glass with reflections, the anode
//!   mesh, all ten unlit cathodes stacked behind each other, and the base;
//! - per digit, a 4-bit glow intensity map. At runtime it is mapped through
//!   `PALETTE` (orange → white-hot) and added onto the background, so a lit
//!   digit costs half a byte per pixel;
//! - the same pair (background + glow) for one neon colon dot.
//!
//! Glow intensities are stored as sqrt(I) so the 16 levels fall mostly in
//! the dim halo, where banding would otherwise show.

use std::fmt::Write as _;
use std::path::Path;

use super::segments::blur;

pub const TUBE_W: usize = 98;
pub const TUBE_H: usize = 224;
pub const DOT_W: usize = 22;
pub const DOT_H: usize = 22;

type Rgb = [f32; 3];

/// Coverage of a rounded rectangle with separate top/bottom corner radii.
fn round_rect(w: usize, h: usize, (x0, y0, x1, y1): (f32, f32, f32, f32), rt: f32, rb: f32) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = if py < (y0 + y1) / 2.0 { rt } else { rb };
            let cx = px.clamp(x0 + r, x1 - r);
            let cy = py.clamp(y0 + r, y1 - r);
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let inside = px >= x0 && px <= x1 && py >= y0 && py <= y1;
            out[y * w + x] = if inside { (r - d + 0.5).clamp(0.0, 1.0) } else { 0.0 };
        }
    }
    out
}

fn add(px: &mut Rgb, c: Rgb, k: f32) {
    for i in 0..3 {
        px[i] += c[i] * k;
    }
}

fn to565(c: Rgb) -> u16 {
    let q = |v: f32, max: f32| (v.clamp(0.0, 255.0) / 255.0 * max).round() as u16;
    q(c[0], 31.0) << 11 | q(c[1], 63.0) << 5 | q(c[2], 31.0)
}

/// Glow color for linear intensity `i` (0-1).
fn ramp(i: f32) -> Rgb {
    const STOPS: [(f32, Rgb); 6] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.06, [45.0, 8.0, 0.0]),
        (0.25, [175.0, 48.0, 4.0]),
        (0.55, [255.0, 108.0, 18.0]),
        (0.8, [255.0, 165.0, 60.0]),
        (1.0, [255.0, 228.0, 165.0]),
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

/// Horizontal squeeze for the cathodes: nixie numerals are tall and narrow.
const CONDENSE: f32 = 0.62;

/// Rasterize digit `d`, condensed horizontally, as a coverage map centered
/// in the tube's cathode area.
fn digit_mask(font: &fontdue::Font, px: f32, d: char) -> Vec<f32> {
    let (m, src) = font.rasterize(d, px);
    // Box-filter resample each row to CONDENSE of its width.
    let w = (m.width as f32 * CONDENSE).ceil() as usize;
    let mut a = vec![0u8; w * m.height];
    for y in 0..m.height {
        for x in 0..w {
            let (x0, x1) = (x as f32 / CONDENSE, (x + 1) as f32 / CONDENSE);
            let (mut sum, mut n) = (0f32, 0f32);
            let mut sx = x0.floor() as usize;
            while (sx as f32) < x1 && sx < m.width {
                let cover = ((sx + 1) as f32).min(x1) - (sx as f32).max(x0);
                sum += src[y * m.width + sx] as f32 * cover;
                n += cover;
                sx += 1;
            }
            a[y * w + x] = if n > 0.0 { (sum / n).round() as u8 } else { 0 };
        }
    }
    let m = fontdue::Metrics { width: w, ..m };
    let mut out = vec![0f32; TUBE_W * TUBE_H];
    // Cathode area: below the dome, above the base.
    let (area_top, area_bot) = (44.0, 200.0);
    let ox = (TUBE_W as i32 - m.width as i32) / 2;
    let oy = ((area_top + area_bot) / 2.0) as i32 - m.height as i32 / 2;
    for y in 0..m.height {
        for x in 0..m.width {
            let (tx, ty) = (ox + x as i32, oy + y as i32);
            if tx >= 0 && ty >= 0 && (tx as usize) < TUBE_W && (ty as usize) < TUBE_H {
                out[ty as usize * TUBE_W + tx as usize] = a[y * m.width + x] as f32 / 255.0;
            }
        }
    }
    out
}

/// Pack linear intensities (0-1) as 4-bit sqrt-encoded nibbles, rows padded.
fn pack_glow(i: &[f32], w: usize) -> Vec<u8> {
    let q = |v: f32| (v.clamp(0.0, 1.0).sqrt() * 15.0).round() as u8;
    i.chunks(w)
        .flat_map(|row| row.chunks(2).map(|p| q(p[0]) << 4 | p.get(1).map_or(0, |&v| q(v))).collect::<Vec<_>>())
        .collect()
}

/// A lit cathode: bright core plus near and far halos.
fn glow_of(mask: &[f32], w: usize, h: usize) -> Vec<f32> {
    let (mut near, mut far) = (mask.to_vec(), mask.to_vec());
    blur(&mut near, w, h, 2);
    blur(&mut far, w, h, 7);
    let peak = |v: &[f32]| v.iter().cloned().fold(1e-6, f32::max);
    let (pn, pf) = (peak(&near), peak(&far));
    (0..mask.len()).map(|i| (mask[i] + near[i] / pn * 0.55 + far[i] / pf * 0.35).min(1.0)).collect()
}

fn tube_background(cathodes: &[f32]) -> Vec<u16> {
    let (w, h) = (TUBE_W, TUBE_H);
    let glass = round_rect(w, h, (3.0, 2.0, w as f32 - 3.0, 214.0), 45.0, 10.0);
    let base = round_rect(w, h, (6.0, 204.0, w as f32 - 6.0, h as f32 - 1.0), 4.0, 8.0);
    let mut px = vec![[0f32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let g = glass[i];
            let p = &mut px[i];
            // Glass body, a touch lighter towards the top.
            let lift = 1.0 - y as f32 / h as f32;
            add(p, [12.0 + 6.0 * lift, 10.0 + 4.0 * lift, 9.0 + 3.0 * lift], g);
            // Reflections: a soft band near the left edge, a faint one right.
            let fx = x as f32;
            let left = (-((fx - 14.0) / 4.0).powi(2)).exp();
            let right = (-((fx - (w as f32 - 12.0)) / 2.5).powi(2)).exp();
            add(p, [255.0, 245.0, 235.0], g * (0.07 * left + 0.035 * right) * (0.4 + 0.6 * lift));
            // Anode mesh: a fine triangular lattice in front of the cathodes.
            let (u, v) = (fx, y as f32);
            let s = 9.0;
            let line = |t: f32| {
                let d = (t / s - (t / s).round()).abs() * s;
                (1.0 - d / 0.7).max(0.0)
            };
            let mesh = line(v).max(line(v * 0.5 + u * 0.866)).max(line(v * 0.5 - u * 0.866));
            let inset = if y > 30 && y < 204 && x > 8 && x < w - 8 { 1.0 } else { 0.0 };
            add(p, [34.0, 28.0, 23.0], g * inset * mesh * 0.3);
            // Unlit cathodes: dull metal wire.
            add(p, [62.0, 44.0, 32.0], g * cathodes[i] * 0.3);
            // Plastic base with a highlight along its top edge.
            let b = base[i];
            for k in 0..3 {
                p[k] = p[k] * (1.0 - b) + [20.0, 18.0, 17.0][k] * b;
            }
            if y == 205 {
                add(p, [70.0, 64.0, 58.0], b * 0.6);
            }
        }
    }
    px.into_iter().map(to565).collect()
}

fn dot_art() -> (Vec<u16>, Vec<f32>) {
    let (w, h) = (DOT_W, DOT_H);
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let disc = |r: f32| -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                (r - ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0)
            })
            .collect()
    };
    let (bulb, core) = (disc(7.5), disc(3.2));
    let bg = bulb.iter().map(|&b| to565([14.0 * b, 11.0 * b, 10.0 * b])).collect();
    (bg, glow_of(&core, w, h))
}

pub fn generate(manifest_dir: &Path, out_dir: &Path, digit_font: &str, digit_px: f32) {
    let path = manifest_dir.join(digit_font);
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings { scale: digit_px, ..Default::default() })
        .unwrap_or_else(|e| panic!("parse {digit_font}: {e}"));

    let masks: Vec<Vec<f32>> = ('0'..='9').map(|d| digit_mask(&font, digit_px, d)).collect();
    let stacked: Vec<f32> = (0..TUBE_W * TUBE_H).map(|i| masks.iter().map(|m| m[i]).fold(0.0, f32::max)).collect();

    let write565 = |name: &str, px: &[u16]| {
        let bytes: Vec<u8> = px.iter().flat_map(|p| p.to_le_bytes()).collect();
        std::fs::write(out_dir.join(name), bytes).unwrap();
    };
    write565("nixie_tube.bin", &tube_background(&stacked));
    for (d, m) in masks.iter().enumerate() {
        std::fs::write(out_dir.join(format!("nixie_glow{d}.a4")), pack_glow(&glow_of(m, TUBE_W, TUBE_H), TUBE_W)).unwrap();
    }
    let (dot_bg, dot_glow) = dot_art();
    write565("nixie_dot.bin", &dot_bg);
    std::fs::write(out_dir.join("nixie_dot_glow.a4"), pack_glow(&dot_glow, DOT_W)).unwrap();

    let palette: Vec<String> = (0..16).map(|q| format!("0x{:04X}", to565(ramp((q as f32 / 15.0).powi(2))))).collect();
    let inc = |f: &str| format!("include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{f}\"))");
    let mut src = String::new();
    writeln!(src, "pub const TUBE_W: usize = {TUBE_W};\npub const TUBE_H: usize = {TUBE_H};").unwrap();
    writeln!(src, "pub const DOT_W: usize = {DOT_W};\npub const DOT_H: usize = {DOT_H};").unwrap();
    writeln!(src, "/// Tube background, RGB565 little-endian, row-major.\npub static TUBE: &[u8] = {};", inc("nixie_tube.bin")).unwrap();
    let glows: Vec<String> = (0..10).map(|d| inc(&format!("nixie_glow{d}.a4"))).collect();
    writeln!(src, "/// Glow intensity per digit, 4-bit, sqrt-encoded.\npub static GLOW: [&[u8]; 10] = [{}];", glows.join(", ")).unwrap();
    writeln!(src, "pub static DOT: &[u8] = {};\npub static DOT_GLOW: &[u8] = {};", inc("nixie_dot.bin"), inc("nixie_dot_glow.a4")).unwrap();
    writeln!(src, "/// Glow color per 4-bit level, RGB565.\npub static PALETTE: [u16; 16] = [{}];", palette.join(", ")).unwrap();
    std::fs::write(out_dir.join("nixie.rs"), src).unwrap();
}
