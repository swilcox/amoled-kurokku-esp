//! Build-time 7-segment LED glyphs.
//!
//! Produces a glyph set where 'a'..='g' are the seven segments of one digit
//! cell (standard lettering: a top, b upper right, c lower right, d bottom,
//! e lower left, f upper left, g middle), 'A'..='G' are blurred halos of the
//! same segments, ':' is the colon's two dots and ';' their halo. All glyphs
//! share one origin — the cell's bottom-left on the baseline — so a digit is
//! drawn by placing every segment glyph at the same pen position.

use super::fontgen::{GlyphSet, RawGlyph};

pub struct SegSpec {
    pub name: &'static str,
    /// Digit cell width and height, px.
    pub w: f32,
    pub h: f32,
    /// Segment thickness.
    pub t: f32,
    /// Gap between neighbouring segments.
    pub gap: f32,
    /// Horizontal lean per px of height (italic).
    pub slant: f32,
    /// Halo blur radius, and its peak opacity (0-1).
    pub glow_r: usize,
    pub glow_peak: f32,
    /// Colon cell width.
    pub colon_w: f32,
}

type Pt = (f32, f32);

/// Hexagonal bar from `p0` to `p1` (axis-aligned), `t` thick, pointed ends.
fn bar(p0: Pt, p1: Pt, t: f32) -> Vec<Pt> {
    let hw = t / 2.0;
    if (p0.1 - p1.1).abs() < f32::EPSILON {
        let (y, xa, xb) = (p0.1, p0.0, p1.0);
        vec![(xa, y), (xa + hw, y - hw), (xb - hw, y - hw), (xb, y), (xb - hw, y + hw), (xa + hw, y + hw)]
    } else {
        let (x, ya, yb) = (p0.0, p0.1, p1.1);
        vec![(x, ya), (x + hw, ya + hw), (x + hw, yb - hw), (x, yb), (x - hw, yb - hw), (x - hw, ya + hw)]
    }
}

fn inside_convex(poly: &[Pt], x: f32, y: f32) -> bool {
    let n = poly.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        let cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        if cross != 0.0 {
            if sign != 0.0 && cross.signum() != sign {
                return false;
            }
            sign = cross.signum();
        }
    }
    true
}

/// 8-bit coverage of `inside` over the integer-aligned box `(x0, y0, w, h)`,
/// 4x4 supersampled.
fn rasterize(x0: i32, y0: i32, w: usize, h: usize, inside: impl Fn(f32, f32) -> bool) -> Vec<u8> {
    const SS: usize = 4;
    let mut out = vec![0u8; w * h];
    for py in 0..h {
        for px in 0..w {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = x0 as f32 + px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = y0 as f32 + py as f32 + (sy as f32 + 0.5) / SS as f32;
                    hits += inside(x, y) as usize;
                }
            }
            out[py * w + px] = (hits * 255 / (SS * SS)) as u8;
        }
    }
    out
}

/// Three-pass box blur (≈ gaussian) of a `w x h` coverage map, in place.
pub fn blur(a: &mut [f32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0f32; a.len()];
    for _ in 0..3 {
        for y in 0..h {
            for x in 0..w {
                let (lo, hi) = (x.saturating_sub(r), (x + r).min(w - 1));
                tmp[y * w + x] = (lo..=hi).map(|i| a[y * w + i]).sum::<f32>() / (2 * r + 1) as f32;
            }
        }
        for y in 0..h {
            for x in 0..w {
                let (lo, hi) = (y.saturating_sub(r), (y + r).min(h - 1));
                a[y * w + x] = (lo..=hi).map(|i| tmp[i * w + x]).sum::<f32>() / (2 * r + 1) as f32;
            }
        }
    }
}

/// Rasterize a shape (given by its bounding box and inside test, in cell
/// coordinates with y down from the cell top) as a glyph, plus its halo.
fn shape_glyphs(
    spec: &SegSpec,
    ch: char,
    halo: char,
    bbox: (f32, f32, f32, f32),
    inside: impl Fn(f32, f32) -> bool,
) -> [RawGlyph; 2] {
    let x0 = bbox.0.floor() as i32 - 1;
    let y0 = bbox.1.floor() as i32 - 1;
    let w = (bbox.2.ceil() as i32 + 1 - x0) as usize;
    let h = (bbox.3.ceil() as i32 + 1 - y0) as usize;
    let alpha = rasterize(x0, y0, w, h, &inside);
    let top = |y: i32| y - spec.h as i32;
    let solid = RawGlyph { ch, w, h, left: x0, top: top(y0), advance: spec.w as i32, alpha: alpha.clone() };

    let pad = 3 * spec.glow_r;
    let (gw, gh) = (w + 2 * pad, h + 2 * pad);
    let mut f = vec![0f32; gw * gh];
    for y in 0..h {
        for x in 0..w {
            f[(y + pad) * gw + x + pad] = alpha[y * w + x] as f32 / 255.0;
        }
    }
    blur(&mut f, gw, gh, spec.glow_r);
    let peak = f.iter().cloned().fold(0.0, f32::max).max(1e-6);
    let glow = f.iter().map(|v| (v / peak * spec.glow_peak * 255.0).round() as u8).collect();
    let halo = RawGlyph {
        ch: halo,
        w: gw,
        h: gh,
        left: x0 - pad as i32,
        top: top(y0 - pad as i32),
        advance: spec.w as i32,
        alpha: glow,
    };
    [solid, halo]
}

pub fn led_set(spec: &SegSpec) -> GlyphSet {
    let (w, h, t, g) = (spec.w, spec.h, spec.t, spec.gap);
    let hw = t / 2.0;
    let mid = h / 2.0;
    // Bars before shearing, in cell coordinates (y down from the top).
    let bars: [(char, Pt, Pt); 7] = [
        ('a', (hw + g, hw), (w - hw - g, hw)),
        ('b', (w - hw, hw + g), (w - hw, mid - g)),
        ('c', (w - hw, mid + g), (w - hw, h - hw - g)),
        ('d', (hw + g, h - hw), (w - hw - g, h - hw)),
        ('e', (hw, mid + g), (hw, h - hw - g)),
        ('f', (hw, hw + g), (hw, mid - g)),
        ('g', (hw + g, mid), (w - hw - g, mid)),
    ];
    // Lean: shift x right the higher the point is.
    let shear = |(x, y): Pt| (x + (h - y) * spec.slant, y);
    let unshear = |x: f32, y: f32| (x - (h - y) * spec.slant, y);

    let mut glyphs = Vec::new();
    for (ch, p0, p1) in bars {
        let poly = bar(p0, p1, t);
        let sheared: Vec<Pt> = poly.iter().map(|&p| shear(p)).collect();
        let bbox = sheared.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |b, &(x, y)| {
            (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y))
        });
        let halo = ch.to_ascii_uppercase();
        glyphs.extend(shape_glyphs(spec, ch, halo, bbox, |x, y| {
            let (ux, uy) = unshear(x, y);
            inside_convex(&poly, ux, uy)
        }));
    }

    // Colon: two round dots in a narrower cell, at 1/3 and 2/3 height.
    let r = t * 0.6;
    let cx = spec.colon_w / 2.0;
    let dots = [h * 0.33, h * 0.67].map(|cy| shear((cx, cy)));
    let bbox = (
        dots[0].0.min(dots[1].0) - r,
        dots[0].1 - r,
        dots[0].0.max(dots[1].0) + r,
        dots[1].1 + r,
    );
    let [mut colon, mut colon_halo] = shape_glyphs(spec, ':', ';', bbox, |x, y| {
        dots.iter().any(|&(dx, dy)| (x - dx).powi(2) + (y - dy).powi(2) <= r * r)
    });
    colon.advance = spec.colon_w as i32;
    colon_halo.advance = spec.colon_w as i32;
    glyphs.push(colon);
    glyphs.push(colon_halo);

    GlyphSet { name: spec.name.to_string(), px: h as u16, ascent: h as i32, descent: 0, glyphs }
}
