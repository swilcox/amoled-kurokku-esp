//! Strip renderer. Pure — host-testable.
//!
//! There is no framebuffer: a full 480x480 RGB565 frame (460 KB) doesn't fit
//! in RAM. Instead a `Scene` knows how to draw itself into any `Canvas` (a
//! window onto the screen), and the `Renderer` walks each dirty rectangle in
//! horizontal bands that fit one small buffer, pushing each band to the
//! `Panel` as it's finished.

use anyhow::Result;

use crate::board::{LCD_H, LCD_W};
use crate::color::to_wire;
use crate::font::{Font, Glyph};

/// Inclusive rectangle in screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

pub const SCREEN: Rect = Rect::new(0, 0, LCD_W as i32 - 1, LCD_H as i32 - 1);

impl Rect {
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub const fn from_size(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self::new(x, y, x + w - 1, y + h - 1)
    }

    pub const fn width(&self) -> i32 {
        self.x1 - self.x0 + 1
    }

    pub const fn height(&self) -> i32 {
        self.y1 - self.y0 + 1
    }

    pub const fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }

    /// Smallest rectangle containing both.
    pub fn union(&self, o: &Rect) -> Rect {
        Rect::new(self.x0.min(o.x0), self.y0.min(o.y0), self.x1.max(o.x1), self.y1.max(o.y1))
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let r = Rect::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1));
        (r.x0 <= r.x1 && r.y0 <= r.y1).then_some(r)
    }

    /// Clip to the screen and grow to the controller's alignment: even
    /// start, odd end on both axes.
    pub fn aligned(&self) -> Option<Rect> {
        let r = self.intersect(&SCREEN)?;
        Some(Rect::new(r.x0 & !1, r.y0 & !1, r.x1 | 1, r.y1 | 1))
    }
}

/// Blend `fg` over `bg` (both RGB565) with 4-bit alpha (0-15).
pub fn blend565(bg: u16, fg: u16, a: u8) -> u16 {
    match a {
        0 => bg,
        15.. => fg,
        _ => {
            let a = a as u32;
            let mix = |shift: u32, mask: u32| {
                let b = (bg as u32 >> shift) & mask;
                let f = (fg as u32 >> shift) & mask;
                ((b * (15 - a) + f * a + 7) / 15) << shift
            };
            (mix(11, 0x1F) | mix(5, 0x3F) | mix(0, 0x1F)) as u16
        }
    }
}

/// Saturating per-channel add of two RGB565 colors.
pub fn add565(a: u16, b: u16) -> u16 {
    let ch = |shift: u32, mask: u16| ((a >> shift & mask) + (b >> shift & mask)).min(mask) << shift;
    ch(11, 0x1F) | ch(5, 0x3F) | ch(0, 0x1F)
}

/// A window onto the screen backed by a row-major buffer of pixels in wire
/// (MSB-first) order, so a finished band can go to the panel as-is. Colors
/// passed in are ordinary RGB565. Drawing is clipped to `area` and to an
/// optional clip rectangle.
pub struct Canvas<'a> {
    area: Rect,
    /// `area` ∩ the clip rectangle; None = nothing is visible.
    clip: Option<Rect>,
    buf: &'a mut [u16],
}

/// Blend table for one foreground color against the most recently seen
/// background pixel (antialiased edges mostly sit on a uniform background).
struct EdgeLut {
    color: u16,
    bg: Option<u16>,
    lut: [u16; 16],
}

impl EdgeLut {
    fn new(color: u16) -> Self {
        Self { color, bg: None, lut: [0; 16] }
    }

    /// Wire-order result of blending `self.color` over wire pixel `dst`.
    fn blend(&mut self, dst: u16, a: u8) -> u16 {
        if self.bg != Some(dst) {
            let bg = to_wire(dst);
            for (k, e) in self.lut.iter_mut().enumerate() {
                *e = to_wire(blend565(bg, self.color, k as u8));
            }
            self.bg = Some(dst);
        }
        self.lut[a as usize]
    }
}

impl<'a> Canvas<'a> {
    pub fn new(area: Rect, buf: &'a mut [u16]) -> Self {
        debug_assert!(buf.len() >= (area.width() * area.height()) as usize);
        Self { area, clip: Some(area), buf }
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    /// Restrict drawing to `r` (within the canvas area) until `reset_clip`.
    pub fn set_clip(&mut self, r: Rect) {
        self.clip = r.intersect(&self.area);
    }

    pub fn reset_clip(&mut self) {
        self.clip = Some(self.area);
    }

    fn visible(&self, r: Rect) -> Option<Rect> {
        r.intersect(&self.clip?)
    }

    fn index(&self, x: i32, y: i32) -> usize {
        ((y - self.area.y0) * self.area.width() + (x - self.area.x0)) as usize
    }

    pub fn fill(&mut self, r: Rect, color: u16) {
        let Some(r) = self.visible(r) else { return };
        let wire = to_wire(color);
        for y in r.y0..=r.y1 {
            let i = self.index(r.x0, y);
            self.buf[i..i + r.width() as usize].fill(wire);
        }
    }

    /// Fill `r` with a vertical gradient from `top` to `bottom`, optionally
    /// with antialiased rounded corners of `radius` on the top and/or bottom
    /// edge.
    pub fn fill_round_rect(&mut self, r: Rect, radius: i32, top: u16, bottom: u16, round_top: bool, round_bottom: bool) {
        let Some(vis) = self.visible(r) else { return };
        let radius = radius.min(r.width() / 2).min(r.height() / 2).max(0);
        let rf = radius as f32;
        for y in vis.y0..=vis.y1 {
            let k = if r.height() > 1 { ((y - r.y0) * 15 / (r.height() - 1)) as u8 } else { 0 };
            let color = blend565(top, bottom, k);
            let wire = to_wire(color);
            let start = self.index(vis.x0, y);
            let row = &mut self.buf[start..start + vis.width() as usize];
            // Vertical distance into a rounded corner, if this row has one.
            let cy = if round_top && y < r.y0 + radius {
                Some(r.y0 as f32 + rf)
            } else if round_bottom && y > r.y1 - radius {
                Some((r.y1 + 1) as f32 - rf)
            } else {
                None
            };
            let Some(cy) = cy else {
                row.fill(wire);
                continue;
            };
            let mut lut = EdgeLut::new(color);
            for (i, dst) in row.iter_mut().enumerate() {
                let x = vis.x0 + i as i32;
                let cx = if x < r.x0 + radius {
                    r.x0 as f32 + rf
                } else if x > r.x1 - radius {
                    (r.x1 + 1) as f32 - rf
                } else {
                    *dst = wire;
                    continue;
                };
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let a = ((rf - d + 0.5).clamp(0.0, 1.0) * 15.0).round() as u8;
                match a {
                    0 => {}
                    15 => *dst = wire,
                    _ => *dst = lut.blend(*dst, a),
                }
            }
        }
    }

    /// Draw one glyph with its pen position at (x, baseline).
    ///
    /// Hot path for scrolling text. Most glyph pixels are fully transparent
    /// or fully opaque; only antialiased edges blend, through `EdgeLut`.
    pub fn glyph(&mut self, font: &Font, g: &Glyph, x: i32, baseline: i32, color: u16) {
        self.glyph_scaled(font, g, x, baseline, color, 0, 1.0);
    }

    /// Like `glyph`, but squashed vertically by `scale` (0-1] towards the
    /// row `pivot_y` — e.g. a flip-clock flap folding over its hinge.
    pub fn glyph_scaled(&mut self, font: &Font, g: &Glyph, x: i32, baseline: i32, color: u16, pivot_y: i32, scale: f32) {
        let gx = x + g.left as i32;
        let gy = baseline + g.top as i32;
        let scaled = scale < 0.999;
        let (top, bottom) = if scaled {
            let map = |y: i32| pivot_y as f32 + (y - pivot_y) as f32 * scale;
            (map(gy).floor() as i32, map(gy + g.h as i32).ceil() as i32)
        } else {
            (gy, gy + g.h as i32)
        };
        let Some(r) = self.visible(Rect::new(gx, top, gx + g.w as i32 - 1, bottom - 1)) else {
            return;
        };
        let stride = (g.w as usize).div_ceil(2);
        let data = &font.data[g.offset as usize..];
        let fg = to_wire(color);
        let mut lut = EdgeLut::new(color);
        let (sx0, w) = ((r.x0 - gx) as usize, r.width() as usize);
        for y in r.y0..=r.y1 {
            let sy = if scaled {
                (pivot_y as f32 + (y as f32 + 0.5 - pivot_y as f32) / scale).floor() as i32 - gy
            } else {
                y - gy
            };
            if sy < 0 || sy >= g.h as i32 {
                continue;
            }
            let src = &data[sy as usize * stride..][..stride];
            let start = self.index(r.x0, y);
            for (i, dst) in self.buf[start..start + w].iter_mut().enumerate() {
                let sx = sx0 + i;
                let b = src[sx / 2];
                let a = if sx % 2 == 0 { b >> 4 } else { b & 0x0F };
                match a {
                    0 => {}
                    15 => *dst = fg,
                    _ => *dst = lut.blend(*dst, a),
                }
            }
        }
    }

    /// Blit a `w x h` sprite at (x, y): `bg` is RGB565 little-endian; each
    /// glow layer is a 4-bit intensity map (rows padded to a byte) scaled by
    /// a 0-15 weight. The strongest layer's level picks a `palette` color
    /// that is added (saturating) onto the background.
    pub fn blit_glow(&mut self, x: i32, y: i32, w: usize, h: usize, bg: &[u8], layers: &[(&[u8], u8)], palette: &[u16; 16]) {
        let Some(r) = self.visible(Rect::from_size(x, y, w as i32, h as i32)) else { return };
        let stride = w.div_ceil(2);
        for py in r.y0..=r.y1 {
            let sy = (py - y) as usize;
            let start = self.index(r.x0, py);
            for (i, dst) in self.buf[start..start + r.width() as usize].iter_mut().enumerate() {
                let sx = (r.x0 - x) as usize + i;
                let k = 2 * (sy * w + sx);
                let base = u16::from_le_bytes([bg[k], bg[k + 1]]);
                let level = layers
                    .iter()
                    .map(|(g, weight)| {
                        let b = g[sy * stride + sx / 2];
                        let q = if sx % 2 == 0 { b >> 4 } else { b & 0x0F };
                        (q as u16 * *weight as u16 / 15) as u8
                    })
                    .max()
                    .unwrap_or(0);
                *dst = to_wire(if level == 0 { base } else { add565(base, palette[level as usize]) });
            }
        }
    }

    /// Draw `s` with its pen starting at (x, baseline). Returns the pen x
    /// after the last character drawn (drawing stops past the canvas).
    pub fn text(&mut self, font: &Font, x: i32, baseline: i32, s: &str, color: u16) -> i32 {
        let mut pen = x;
        for c in s.chars() {
            if let Some(g) = font.glyph(c) {
                // Skip glyphs wholly left of this canvas; stop once past it.
                // Keeps long scrolling messages cheap per band.
                if pen + g.left as i32 > self.area.x1 {
                    break;
                }
                if pen + g.left as i32 + g.w as i32 > self.area.x0 {
                    self.glyph(font, g, pen, baseline, color);
                }
                pen += g.advance as i32;
            }
        }
        pen
    }

    /// Draw `s` horizontally centered on `cx`.
    pub fn text_centered(&mut self, font: &Font, cx: i32, baseline: i32, s: &str, color: u16) {
        self.text(font, cx - font.text_width(s) / 2, baseline, s, color);
    }
}

/// Something that can paint any region of the screen on demand.
pub trait Scene {
    /// Paint everything that intersects `canvas.area()`, background included.
    fn draw(&self, canvas: &mut Canvas);
}

/// A display that accepts rectangles of pixels in wire (MSB-first) order.
pub trait Panel {
    /// Start sending `pixels` to `area`. May return while the transfer is
    /// still in flight: the caller must leave `pixels` untouched until the
    /// next `draw` or `flush` returns.
    fn draw(&mut self, area: Rect, pixels: &[u16]) -> Result<()>;

    /// Wait for any transfer still in flight.
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Renders scenes in bands, alternating between two buffers so the next
/// band is drawn while the previous one is still being sent.
pub struct Renderer {
    bufs: [Vec<u16>; 2],
}

impl Renderer {
    /// `pixels` is the size of each of the two band buffers; it must hold at
    /// least two full rows.
    pub fn new(pixels: usize) -> Self {
        assert!(pixels >= 2 * LCD_W as usize);
        Self { bufs: [vec![0; pixels], vec![0; pixels]] }
    }

    /// Redraw `rects` from `scene` onto `panel`.
    pub fn render<S: Scene + ?Sized>(&mut self, panel: &mut impl Panel, scene: &S, rects: &[Rect]) -> Result<()> {
        let mut which = 0;
        let result = (|| {
            for r in rects.iter().filter_map(Rect::aligned) {
                let rows = ((self.bufs[0].len() / r.width() as usize) & !1) as i32;
                let mut y = r.y0;
                while y <= r.y1 {
                    let band = Rect::new(r.x0, y, r.x1, (y + rows - 1).min(r.y1));
                    // Safe to overwrite: the panel's last `draw` returned only
                    // after the transfer from this buffer (two bands ago) ended.
                    let buf = &mut self.bufs[which][..(band.width() * band.height()) as usize];
                    scene.draw(&mut Canvas::new(band, buf));
                    panel.draw(band, buf)?;
                    which ^= 1;
                    y = band.y1 + 1;
                }
            }
            Ok(())
        })();
        // Never return with DMA still reading our buffers.
        panel.flush()?;
        result
    }
}

/// An in-memory full-screen panel, for host previews and tests.
pub struct MemPanel {
    /// Native-order RGB565, row-major.
    pub pixels: Vec<u16>,
}

impl Default for MemPanel {
    fn default() -> Self {
        Self { pixels: vec![0; LCD_W as usize * LCD_H as usize] }
    }
}

impl MemPanel {
    /// Expand to 8-bit RGB triples.
    pub fn to_rgb8(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|&p| {
                let (r, g, b) = ((p >> 11) & 0x1F, (p >> 5) & 0x3F, p & 0x1F);
                [(r << 3 | r >> 2) as u8, (g << 2 | g >> 4) as u8, (b << 3 | b >> 2) as u8]
            })
            .collect()
    }
}

impl Panel for MemPanel {
    fn draw(&mut self, area: Rect, pixels: &[u16]) -> Result<()> {
        let w = area.width() as usize;
        for (i, row) in pixels.chunks(w).take(area.height() as usize).enumerate() {
            let start = (area.y0 as usize + i) * LCD_W as usize + area.x0 as usize;
            for (dst, &src) in self.pixels[start..start + w].iter_mut().zip(row) {
                *dst = src.swap_bytes();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_grows_to_even_odd_and_clips() {
        assert_eq!(Rect::new(3, 5, 10, 12).aligned(), Some(Rect::new(2, 4, 11, 13)));
        assert_eq!(Rect::new(-5, 470, 600, 900).aligned(), Some(Rect::new(0, 470, 479, 479)));
        assert_eq!(Rect::new(500, 0, 600, 10).aligned(), None);
    }

    #[test]
    fn add_saturates_per_channel() {
        assert_eq!(add565(0xF800, 0xF800), 0xF800);
        assert_eq!(add565(0x0001, 0x0002), 0x0003);
        assert_eq!(add565(0x07E0, 0x0020), 0x07E0);
    }

    #[test]
    fn blend_endpoints_and_midpoint() {
        assert_eq!(blend565(0x0000, 0xFFFF, 0), 0x0000);
        assert_eq!(blend565(0x0000, 0xFFFF, 15), 0xFFFF);
        let mid = blend565(0x0000, 0xF800, 8);
        assert!((15..=17).contains(&(mid >> 11)));
    }

    struct Checker;
    impl Scene for Checker {
        fn draw(&self, c: &mut Canvas) {
            let a = c.area();
            for y in a.y0..=a.y1 {
                for x in a.x0..=a.x1 {
                    c.fill(Rect::new(x, y, x, y), ((x ^ y) & 0xFFFF) as u16);
                }
            }
        }
    }

    #[test]
    fn fast_glyph_path_matches_reference_blend() {
        use crate::fonts::TEXT_L;
        let (bg, fg) = (0x1234, 0xFD20);
        let g = TEXT_L.glyph('&').unwrap();
        let area = Rect::from_size(0, 0, g.w as i32 + 4, g.h as i32 + 4);
        let mut buf = vec![0u16; (area.width() * area.height()) as usize];
        let mut c = Canvas::new(area, &mut buf);
        c.fill(area, bg);
        c.glyph(&TEXT_L, g, 2 - g.left as i32, 2 - g.top as i32, fg);
        for y in 0..g.h as usize {
            for x in 0..g.w as usize {
                let got = buf[(y + 2) * area.width() as usize + x + 2].swap_bytes();
                assert_eq!(got, blend565(bg, fg, TEXT_L.alpha(g, x, y)), "({x}, {y})");
            }
        }
    }

    #[test]
    fn renderer_bands_reassemble_exactly() {
        let mut panel = MemPanel::default();
        Renderer::new(480 * 7).render(&mut panel, &Checker, &[SCREEN]).unwrap();
        for (i, &p) in panel.pixels.iter().enumerate() {
            let (x, y) = (i % 480, i / 480);
            assert_eq!(p, ((x ^ y) & 0xFFFF) as u16, "pixel ({x}, {y})");
        }
    }
}
