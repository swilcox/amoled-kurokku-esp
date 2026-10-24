//! Build-time analog clock dials.
//!
//! Each dial is a full 480×480 RGB565 picture (bezel, markers, numerals),
//! drawn here with antialiased shapes and stored run-length encoded per row,
//! so a band of any dirty rectangle can be blitted without decoding the
//! rest. Hands are drawn at runtime (`src/clock/analog.rs`).
//!
//! Emits `$OUT_DIR/analog.rs` (included by `src/sprites.rs`).
//!
//! RLE layout: 480 little-endian u32 row offsets (bytes from the blob
//! start), then per row a sequence of (u16 length, u16 RGB565 color) runs
//! covering exactly 480 pixels.

use std::fmt::Write as _;
use std::path::Path;

use super::assetpack::Pack;

const N: usize = 480;
/// Dial center; pixel centers sit at +0.5, so this is the screen's middle.
const C: f32 = 240.0;

/// Classic dial's date window: center and size, in screen pixels.
pub const DATE_X: i32 = 360;
pub const DATE_Y: i32 = 240;
pub const DATE_W: i32 = 46;
pub const DATE_H: i32 = 34;

type Rgb = [f32; 3];

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t.clamp(0.0, 1.0))
}

fn over(px: &mut Rgb, c: Rgb, a: f32) {
    *px = mix(*px, c, a);
}

fn to565(c: Rgb) -> u16 {
    let q = |v: f32, max: f32| (v.clamp(0.0, 255.0) / 255.0 * max).round() as u16;
    q(c[0], 31.0) << 11 | q(c[1], 63.0) << 5 | q(c[2], 31.0)
}

/// Coverage of a radial bar at clock angle `theta` (radians clockwise from
/// 12): from radius `r0` to `r1`, `hw` half-width, for the pixel at (x, y).
fn bar(x: f32, y: f32, theta: f32, r0: f32, r1: f32, hw: f32) -> f32 {
    let (s, c) = theta.sin_cos();
    let (px, py) = (x - C, y - C);
    let u = px * s - py * c;
    let v = px * c + py * s;
    let d = (v.abs() - hw).max(r0 - u).max(u - r1);
    (0.5 - d).clamp(0.0, 1.0)
}

/// Coverage of the ring between radii `r0` and `r1`.
fn ring(r: f32, r0: f32, r1: f32) -> f32 {
    (0.5 - (r0 - r).max(r - r1)).clamp(0.0, 1.0)
}

/// Coverage of an axis-aligned rounded rectangle centered at (cx, cy).
fn round_rect(x: f32, y: f32, cx: f32, cy: f32, w: f32, h: f32, radius: f32) -> f32 {
    let qx = (x - cx).abs() - (w / 2.0 - radius);
    let qy = (y - cy).abs() - (h / 2.0 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let d = outside + qx.max(qy).min(0.0) - radius;
    (0.5 - d).clamp(0.0, 1.0)
}

/// Railway dial: plain disc, bold hour bars, minute ticks, graphite bezel.
fn railway(face: Rgb, marks: Rgb, bezel: Rgb) -> Vec<Rgb> {
    let mut img = vec![[0.0; 3]; N * N];
    for (i, px) in img.iter_mut().enumerate() {
        let (x, y) = ((i % N) as f32 + 0.5, (i / N) as f32 + 0.5);
        let r = ((x - C).powi(2) + (y - C).powi(2)).sqrt();
        if r > 238.0 {
            continue;
        }
        // Bezel with a soft highlight towards the top left.
        let light = 0.5 + 0.5 * ((x - C) * -0.7 + (y - C) * -0.7) / r.max(1.0);
        over(px, mix(bezel, [bezel[0] * 1.6, bezel[1] * 1.6, bezel[2] * 1.6], light), ring(r, 229.5, 237.0));
        over(px, face, (229.5 - r + 0.5).clamp(0.0, 1.0));
        if !(150.0..=224.0).contains(&r) {
            continue;
        }
        for k in 0..60 {
            let theta = k as f32 * std::f32::consts::TAU / 60.0;
            let a = if k % 5 == 0 { bar(x, y, theta, 160.0, 222.0, 6.5) } else { bar(x, y, theta, 204.0, 222.0, 2.2) };
            over(px, marks, a);
        }
    }
    img
}

/// Classic dial: charcoal sunburst, brass bezel, minute track, serif
/// numerals and a date window at 3 o'clock.
fn classic(numeral: &fontdue::Font, px_size: f32) -> Vec<Rgb> {
    const CREAM: Rgb = [226.0, 214.0, 188.0];
    const BRASS_DARK: Rgb = [86.0, 64.0, 30.0];
    const BRASS_LIGHT: Rgb = [196.0, 160.0, 92.0];
    let mut img = vec![[0.0; 3]; N * N];
    for (i, px) in img.iter_mut().enumerate() {
        let (x, y) = ((i % N) as f32 + 0.5, (i / N) as f32 + 0.5);
        let r = ((x - C).powi(2) + (y - C).powi(2)).sqrt();
        if r > 238.0 {
            continue;
        }
        let light = 0.5 + 0.5 * ((x - C) * -0.7 + (y - C) * -0.7) / r.max(1.0);
        over(px, mix(BRASS_DARK, BRASS_LIGHT, light), ring(r, 229.5, 237.0));
        // Dial: lighter in the middle, with a faint sunburst.
        let phi = (x - C).atan2(C - y);
        let burst = 1.0 + 0.06 * (phi * 90.0).sin();
        let base = mix([34.0, 36.0, 42.0], [12.0, 13.0, 16.0], r / 229.5);
        over(px, base.map(|v| v * burst), (229.5 - r + 0.5).clamp(0.0, 1.0));
        // Minute track.
        over(px, CREAM, ring(r, 225.4, 226.6) * 0.8);
        over(px, CREAM, ring(r, 211.6, 212.4) * 0.6);
        if (200.0..=227.0).contains(&r) {
            for k in 0..60 {
                let theta = k as f32 * std::f32::consts::TAU / 60.0;
                let a = if k % 5 == 0 { bar(x, y, theta, 205.0, 226.0, 2.0) } else { bar(x, y, theta, 212.0, 226.0, 0.8) };
                over(px, CREAM, a);
            }
        }
        // Date window: a light card in a brass frame.
        let (dx, dy, dw, dh) = (DATE_X as f32, DATE_Y as f32, DATE_W as f32, DATE_H as f32);
        over(px, BRASS_LIGHT, round_rect(x, y, dx, dy, dw + 4.0, dh + 4.0, 6.0));
        over(px, [236.0, 230.0, 214.0], round_rect(x, y, dx, dy, dw, dh, 4.0));
    }
    // Numerals, upright, centered on a circle.
    for n in 1..=12 {
        let theta = n as f32 * std::f32::consts::TAU / 12.0;
        let (cx, cy) = (C + 176.0 * theta.sin(), C - 176.0 * theta.cos());
        stamp_text(&mut img, numeral, px_size, &n.to_string(), cx, cy, CREAM);
    }
    img
}

/// Draw `s` with its ink box centered on (cx, cy).
fn stamp_text(img: &mut [Rgb], font: &fontdue::Font, px: f32, s: &str, cx: f32, cy: f32, color: Rgb) {
    let glyphs: Vec<_> = s.chars().map(|ch| font.rasterize(ch, px)).collect();
    // Pen positions and the ink box (y-down, relative to the baseline).
    let (mut pen, mut x0, mut x1, mut top, mut bot) = (0.0f32, f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    let mut placed = Vec::new();
    for (m, a) in &glyphs {
        let gx = pen + m.xmin as f32;
        let gy = -(m.ymin as f32 + m.height as f32);
        x0 = x0.min(gx);
        x1 = x1.max(gx + m.width as f32);
        top = top.min(gy);
        bot = bot.max(gy + m.height as f32);
        placed.push((gx, gy, m, a));
        pen += m.advance_width;
    }
    let (ox, oy) = ((cx - (x0 + x1) / 2.0).round(), (cy - (top + bot) / 2.0).round());
    for (gx, gy, m, a) in placed {
        for row in 0..m.height {
            for col in 0..m.width {
                let (x, y) = ((ox + gx) as i32 + col as i32, (oy + gy) as i32 + row as i32);
                if (0..N as i32).contains(&x) && (0..N as i32).contains(&y) {
                    over(&mut img[y as usize * N + x as usize], color, a[row * m.width + col] as f32 / 255.0);
                }
            }
        }
    }
}

fn rle(img: &[Rgb]) -> Vec<u8> {
    let px: Vec<u16> = img.iter().map(|&c| to565(c)).collect();
    let mut runs = Vec::new();
    let mut offsets = Vec::with_capacity(N);
    for row in px.chunks(N) {
        offsets.push((4 * N + runs.len()) as u32);
        let mut x = 0;
        while x < N {
            let c = row[x];
            let len = row[x..].iter().take_while(|&&p| p == c).count();
            runs.extend_from_slice(&(len as u16).to_le_bytes());
            runs.extend_from_slice(&c.to_le_bytes());
            x += len;
        }
    }
    offsets.iter().flat_map(|o| o.to_le_bytes()).chain(runs).collect()
}

pub fn generate(manifest_dir: &Path, out_dir: &Path, pack: &mut Pack, numeral_font: &str, numeral_px: f32) {
    let path = manifest_dir.join(numeral_font);
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings { scale: numeral_px, ..Default::default() })
        .unwrap_or_else(|e| panic!("parse {numeral_font}: {e}"));

    let railway_white = pack.add(&rle(&railway([246.0, 246.0, 243.0], [20.0, 20.0, 20.0], [44.0, 44.0, 48.0])));
    let railway_black = pack.add(&rle(&railway([6.0, 6.0, 6.0], [236.0, 236.0, 232.0], [44.0, 44.0, 48.0])));
    let classic = pack.add(&rle(&classic(&font, numeral_px)));

    let mut src = String::new();
    writeln!(src, "/// Dials, row-RLE (see `build_support/analog.rs`).").unwrap();
    writeln!(src, "pub static RAILWAY_WHITE: crate::assets::Blob = {railway_white};").unwrap();
    writeln!(src, "pub static RAILWAY_BLACK: crate::assets::Blob = {railway_black};").unwrap();
    writeln!(src, "pub static CLASSIC: crate::assets::Blob = {classic};").unwrap();
    writeln!(src, "/// Classic date window: center x, y and size.").unwrap();
    writeln!(src, "pub const DATE_WINDOW: (i32, i32, i32, i32) = ({DATE_X}, {DATE_Y}, {DATE_W}, {DATE_H});").unwrap();
    std::fs::write(out_dir.join("analog.rs"), src).unwrap();
}
