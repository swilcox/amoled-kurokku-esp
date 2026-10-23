//! Strip renderer. Pure — host-testable.
//!
//! There is no framebuffer: a full 480x480 RGB565 frame (460 KB) doesn't fit
//! in RAM. Instead a `Scene` knows how to draw itself into any `Canvas` (a
//! window onto the screen), and the `Renderer` walks each dirty rectangle in
//! horizontal bands that fit one small buffer, pushing each band to the
//! `Panel` as it's finished.

use anyhow::Result;

use crate::board::{LCD_H, LCD_W};
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

/// A window onto the screen backed by a row-major buffer of native-order
/// RGB565. Drawing outside `area` is clipped.
pub struct Canvas<'a> {
    area: Rect,
    buf: &'a mut [u16],
}

impl<'a> Canvas<'a> {
    pub fn new(area: Rect, buf: &'a mut [u16]) -> Self {
        debug_assert!(buf.len() >= (area.width() * area.height()) as usize);
        Self { area, buf }
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    fn index(&self, x: i32, y: i32) -> usize {
        ((y - self.area.y0) * self.area.width() + (x - self.area.x0)) as usize
    }

    pub fn fill(&mut self, r: Rect, color: u16) {
        let Some(r) = r.intersect(&self.area) else { return };
        for y in r.y0..=r.y1 {
            let i = self.index(r.x0, y);
            self.buf[i..i + r.width() as usize].fill(color);
        }
    }

    /// Draw one glyph with its pen position at (x, baseline).
    pub fn glyph(&mut self, font: &Font, g: &Glyph, x: i32, baseline: i32, color: u16) {
        let gx = x + g.left as i32;
        let gy = baseline + g.top as i32;
        let Some(r) = Rect::from_size(gx, gy, g.w as i32, g.h as i32).intersect(&self.area) else {
            return;
        };
        for y in r.y0..=r.y1 {
            let row = self.index(r.x0, y);
            for (i, x) in (r.x0..=r.x1).enumerate() {
                let a = font.alpha(g, (x - gx) as usize, (y - gy) as usize);
                if a != 0 {
                    let p = &mut self.buf[row + i];
                    *p = blend565(*p, color, a);
                }
            }
        }
    }

    /// Draw `s` with its pen starting at (x, baseline). Returns the pen x
    /// after the last character.
    pub fn text(&mut self, font: &Font, x: i32, baseline: i32, s: &str, color: u16) -> i32 {
        let mut pen = x;
        for c in s.chars() {
            if let Some(g) = font.glyph(c) {
                self.glyph(font, g, pen, baseline, color);
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
    fn draw(&mut self, area: Rect, pixels: &[u16]) -> Result<()>;
}

pub struct Renderer {
    buf: Vec<u16>,
}

impl Renderer {
    /// `pixels` is the band buffer size; it must hold at least two full rows.
    pub fn new(pixels: usize) -> Self {
        assert!(pixels >= 2 * LCD_W as usize);
        Self { buf: vec![0; pixels] }
    }

    /// Redraw `rects` from `scene` onto `panel`.
    pub fn render(&mut self, panel: &mut impl Panel, scene: &impl Scene, rects: &[Rect]) -> Result<()> {
        for r in rects.iter().filter_map(Rect::aligned) {
            let rows = ((self.buf.len() / r.width() as usize) & !1) as i32;
            let mut y = r.y0;
            while y <= r.y1 {
                let band = Rect::new(r.x0, y, r.x1, (y + rows - 1).min(r.y1));
                let buf = &mut self.buf[..(band.width() * band.height()) as usize];
                scene.draw(&mut Canvas::new(band, buf));
                for p in buf.iter_mut() {
                    *p = crate::color::to_wire(*p);
                }
                panel.draw(band, buf)?;
                y = band.y1 + 1;
            }
        }
        Ok(())
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
    fn renderer_bands_reassemble_exactly() {
        let mut panel = MemPanel::default();
        Renderer::new(480 * 7).render(&mut panel, &Checker, &[SCREEN]).unwrap();
        for (i, &p) in panel.pixels.iter().enumerate() {
            let (x, y) = (i % 480, i / 480);
            assert_eq!(p, ((x ^ y) & 0xFFFF) as u16, "pixel ({x}, {y})");
        }
    }
}
