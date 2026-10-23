//! Anti-aliased bitmap fonts generated at build time (see
//! `build_support/fontgen.rs`). Pure — host-testable.

/// One rasterized glyph. Coordinates are relative to the pen position on the
/// baseline, y-down: the bitmap's top-left is at (pen + left, baseline + top).
#[derive(Debug)]
pub struct Glyph {
    pub ch: char,
    pub w: u16,
    pub h: u16,
    pub left: i16,
    pub top: i16,
    pub advance: u16,
    /// Byte offset of this glyph's 4-bit alpha rows in `Font::data`.
    pub offset: u32,
}

#[derive(Debug)]
pub struct Font {
    pub px: u16,
    pub ascent: i16,
    pub descent: i16,
    /// Sorted by `ch`.
    pub glyphs: &'static [Glyph],
    /// 4-bit alpha rows for all glyphs, in the asset pack.
    pub data: crate::assets::Blob,
}

impl Font {
    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        self.glyphs
            .binary_search_by_key(&ch, |g| g.ch)
            .ok()
            .map(|i| &self.glyphs[i])
    }

    pub fn advance(&self, ch: char) -> i32 {
        self.glyph(ch).map_or(0, |g| g.advance as i32)
    }

    pub fn text_width(&self, s: &str) -> i32 {
        s.chars().map(|c| self.advance(c)).sum()
    }

    /// Alpha (0-15) of pixel (x, y) within `g`'s bitmap.
    pub fn alpha(&self, g: &Glyph, x: usize, y: usize) -> u8 {
        let stride = (g.w as usize).div_ceil(2);
        let b = self.data.bytes()[g.offset as usize + y * stride + x / 2];
        if x % 2 == 0 {
            b >> 4
        } else {
            b & 0x0F
        }
    }

    /// Screen rectangle covered by `ch`'s bitmap with the pen at
    /// (x, baseline).
    pub fn glyph_rect(&self, ch: char, x: i32, baseline: i32) -> Option<crate::gfx::Rect> {
        let g = self.glyph(ch).filter(|g| g.w > 0 && g.h > 0)?;
        Some(crate::gfx::Rect::from_size(
            x + g.left as i32,
            baseline + g.top as i32,
            g.w as i32,
            g.h as i32,
        ))
    }

    /// Ink extent (top, bottom-exclusive) over `chars`, relative to the
    /// baseline, y-down.
    pub fn ink_rows(&self, chars: &str) -> (i32, i32) {
        chars
            .chars()
            .filter_map(|c| self.glyph(c))
            .filter(|g| g.h > 0)
            .fold((i32::MAX, i32::MIN), |(t, b), g| {
                (t.min(g.top as i32), b.max(g.top as i32 + g.h as i32))
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::fonts::CLOCK_BIG;

    #[test]
    fn glyphs_are_sorted_and_present() {
        assert!(CLOCK_BIG.glyphs.windows(2).all(|w| w[0].ch < w[1].ch));
        for c in "0123456789:".chars() {
            assert!(CLOCK_BIG.glyph(c).is_some(), "missing {c:?}");
        }
        assert!(CLOCK_BIG.glyph('x').is_none());
    }

    #[test]
    fn digit_zero_has_ink_above_baseline() {
        let g = CLOCK_BIG.glyph('0').unwrap();
        assert!(g.top < 0 && g.h > 100);
        let mid = (0..g.w as usize).map(|x| CLOCK_BIG.alpha(g, x, g.h as usize / 2)).max();
        assert_eq!(mid, Some(15));
    }
}
