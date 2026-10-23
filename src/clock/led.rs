//! "LED" face: red 7-segment digits with unlit "ghost" segments and a soft
//! glow, a blinking colon, small seconds below and the date above.

use super::{display_hour, Face, LocalTime, MONTHS, WEEKDAYS};
use crate::board::LCD_W;
use crate::color::rgb565;
use crate::font::Font;
use crate::fonts::{CLOCK_DATE, LED_BIG, LED_SMALL};
use crate::gfx::{Canvas, Rect, Scene, SCREEN};

const BG: u16 = rgb565(0, 0, 0);
const LIT: u16 = rgb565(255, 34, 22);
const GHOST: u16 = rgb565(30, 5, 4);
const DATE: u16 = rgb565(150, 22, 14);

/// Segments a..g as bits 0..6, per digit 0-9.
const DIGITS: [u8; 10] = [0x3F, 0x06, 0x5B, 0x4F, 0x66, 0x6D, 0x7D, 0x07, 0x7F, 0x6F];
const SEGS: [char; 7] = ['a', 'b', 'c', 'd', 'e', 'f', 'g'];

const DIGIT_GAP: i32 = 12;
const SMALL_GAP: i32 = 8;
const LINE_GAP: i32 = 30;

/// Draw one 7-segment digit cell with its pen at (x, baseline). `lit` is a
/// segment bitmask; unlit segments are drawn as ghosts.
fn draw_digit(c: &mut Canvas, font: &Font, x: i32, baseline: i32, lit: u8) {
    let segs = || SEGS.iter().enumerate().map(|(i, &ch)| (ch, lit & (1 << i) != 0));
    for (ch, on) in segs() {
        if !on {
            draw(c, font, ch, x, baseline, GHOST);
        }
    }
    for (ch, on) in segs() {
        if on {
            draw(c, font, ch.to_ascii_uppercase(), x, baseline, LIT);
        }
    }
    for (ch, on) in segs() {
        if on {
            draw(c, font, ch, x, baseline, LIT);
        }
    }
}

fn draw(c: &mut Canvas, font: &Font, ch: char, x: i32, baseline: i32, color: u16) {
    if let Some(g) = font.glyph(ch) {
        c.glyph(font, g, x, baseline, color);
    }
}

/// Everything a digit cell (with halos) can touch, pen at (x, baseline).
fn cell_rect(font: &Font, x: i32, baseline: i32) -> Rect {
    "ABCDEFG"
        .chars()
        .filter_map(|ch| font.glyph_rect(ch, x, baseline))
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Rect::new(x, baseline, x, baseline))
}

struct Layout {
    /// Pen x of the four big digits and the colon.
    digit_x: [i32; 4],
    colon_x: i32,
    big_base: i32,
    /// Pen x of the two seconds digits; the AM/PM label follows.
    secs_x: [i32; 2],
    ampm_x: i32,
    small_base: i32,
    date_base: i32,
}

impl Layout {
    fn new() -> Self {
        let (bw, sw) = (LED_BIG.advance('a'), LED_SMALL.advance('a'));
        let colon_w = LED_BIG.advance(':');
        let total = 4 * bw + 2 * DIGIT_GAP + colon_w;
        let x0 = (LCD_W as i32 - total) / 2;
        let digit_x = [x0, x0 + bw + DIGIT_GAP, x0 + 2 * bw + DIGIT_GAP + colon_w, x0 + 3 * bw + 2 * DIGIT_GAP + colon_w];

        let date_h = CLOCK_DATE.ink_rows("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789").1
            - CLOCK_DATE.ink_rows("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789").0;
        let block = date_h + LINE_GAP + LED_BIG.ascent as i32 + LINE_GAP + LED_SMALL.ascent as i32;
        let top = (SCREEN.height() - block) / 2;
        let date_base = top + date_h;
        let big_base = date_base + LINE_GAP + LED_BIG.ascent as i32;
        let small_base = big_base + LINE_GAP + LED_SMALL.ascent as i32;

        let secs_w = 2 * sw + SMALL_GAP;
        let sx = (LCD_W as i32 - secs_w) / 2;
        Self {
            digit_x,
            colon_x: x0 + 2 * bw + DIGIT_GAP,
            big_base,
            secs_x: [sx, sx + sw + SMALL_GAP],
            ampm_x: sx + secs_w + 2 * SMALL_GAP,
            small_base,
            date_base,
        }
    }
}

pub struct LedFace {
    format_24h: bool,
    layout: Layout,
    t: LocalTime,
    shown: Option<LocalTime>,
    off: (i32, i32),
}

impl LedFace {
    pub fn new(format_24h: bool) -> Self {
        Self { format_24h, layout: Layout::new(), t: LocalTime::default(), shown: None, off: (0, 0) }
    }

    /// Segment masks for HH:MM; a 12h hour below 10 blanks its tens digit
    /// (ghosts only).
    fn big(&self, t: &LocalTime) -> [u8; 4] {
        let (h, _) = display_hour(t.hour, self.format_24h);
        let tens = if self.format_24h || h >= 10 { DIGITS[(h / 10) as usize] } else { 0 };
        [tens, DIGITS[(h % 10) as usize], DIGITS[(t.minute / 10) as usize], DIGITS[(t.minute % 10) as usize]]
    }

    fn colon_on(t: &LocalTime) -> bool {
        t.second % 2 == 0
    }

    fn ampm(&self, t: &LocalTime) -> &'static str {
        match display_hour(t.hour, self.format_24h) {
            _ if self.format_24h => "",
            (_, true) => "PM",
            (_, false) => "AM",
        }
    }

    fn date_text(t: &LocalTime) -> String {
        let wd = WEEKDAYS.get(t.weekday as usize).copied().unwrap_or("");
        let mon = MONTHS.get(t.month.wrapping_sub(1) as usize).copied().unwrap_or("");
        format!("{wd}  {mon} {}", t.day)
    }

    fn date_rect(&self) -> Rect {
        let b = self.layout.date_base;
        let (t, bot) = CLOCK_DATE.ink_rows("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
        Rect::new(0, b + t - 4, SCREEN.x1, b + bot + 4)
    }

    fn secs_rect(&self) -> Rect {
        let l = &self.layout;
        let r = cell_rect(&LED_SMALL, l.secs_x[0], l.small_base).union(&cell_rect(&LED_SMALL, l.secs_x[1], l.small_base));
        Rect::new(0, r.y0, SCREEN.x1, r.y1)
    }
}

impl Face for LedFace {
    fn update(&mut self, _now_ms: u32, t: &LocalTime) -> Vec<Rect> {
        self.t = *t;
        let Some(prev) = self.shown.replace(*t) else {
            return vec![SCREEN];
        };
        let (dx, dy) = self.off;
        let l = &self.layout;
        let mut dirty = Vec::new();
        let (old, new) = (self.big(&prev), self.big(t));
        for i in 0..4 {
            if old[i] != new[i] {
                dirty.push(cell_rect(&LED_BIG, l.digit_x[i], l.big_base));
            }
        }
        if Self::colon_on(&prev) != Self::colon_on(t) {
            let r = [':', ';'].iter().filter_map(|&ch| LED_BIG.glyph_rect(ch, l.colon_x, l.big_base)).reduce(|a, b| a.union(&b));
            dirty.extend(r);
        }
        if prev.second != t.second || self.ampm(&prev) != self.ampm(t) {
            dirty.push(self.secs_rect());
        }
        if Self::date_text(&prev) != Self::date_text(t) {
            dirty.push(self.date_rect());
        }
        dirty.into_iter().map(|r| r.translate(dx, dy)).collect()
    }

    fn invalidate(&mut self) {
        self.shown = None;
    }

    fn set_offset(&mut self, dx: i32, dy: i32) {
        if self.off != (dx, dy) {
            self.off = (dx, dy);
            self.invalidate();
        }
    }
}

impl Scene for LedFace {
    fn draw(&self, c: &mut Canvas) {
        let l = &self.layout;
        let (dx, dy) = self.off;
        let t = &self.t;
        c.fill(c.area(), BG);
        let area = c.area();

        if area.intersect(&self.date_rect().translate(dx, dy)).is_some() {
            c.text_centered(&CLOCK_DATE, LCD_W as i32 / 2 + dx, l.date_base + dy, &Self::date_text(t), DATE);
        }

        for (i, lit) in self.big(t).into_iter().enumerate() {
            let (x, b) = (l.digit_x[i] + dx, l.big_base + dy);
            if area.intersect(&cell_rect(&LED_BIG, x, b)).is_some() {
                draw_digit(c, &LED_BIG, x, b, lit);
            }
        }
        let (cx, b) = (l.colon_x + dx, l.big_base + dy);
        if Self::colon_on(t) {
            draw(c, &LED_BIG, ';', cx, b, LIT);
            draw(c, &LED_BIG, ':', cx, b, LIT);
        } else {
            draw(c, &LED_BIG, ':', cx, b, GHOST);
        }

        if area.intersect(&self.secs_rect().translate(dx, dy)).is_some() {
            let sb = l.small_base + dy;
            for (i, d) in [t.second / 10, t.second % 10].into_iter().enumerate() {
                draw_digit(c, &LED_SMALL, l.secs_x[i] + dx, sb, DIGITS[d as usize]);
            }
            let ampm = self.ampm(t);
            if !ampm.is_empty() {
                c.text(&CLOCK_DATE, l.ampm_x + dx, sb, ampm, LIT);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::tests::check_incremental;

    fn at(hour: u8, minute: u8, second: u8) -> LocalTime {
        LocalTime { year: 2026, month: 10, day: 22, weekday: 4, hour, minute, second }
    }

    #[test]
    fn layout_fits_on_screen() {
        let l = Layout::new();
        let first = cell_rect(&LED_BIG, l.digit_x[0], l.big_base);
        let last = cell_rect(&LED_BIG, l.digit_x[3], l.big_base);
        assert!(first.x0 >= 0 && last.x1 < 480, "{first:?} {last:?}");
        let face = LedFace::new(true);
        assert!(face.date_rect().y0 >= 0 && face.secs_rect().y1 < 480);
    }

    #[test]
    fn incremental_matches_full_render() {
        let led = |h24| move || Box::new(LedFace::new(h24)) as Box<dyn Face>;
        check_incremental(led(true), at(12, 34, 56), at(12, 34, 57));
        check_incremental(led(true), at(12, 59, 59), at(13, 0, 0));
        check_incremental(led(false), at(9, 59, 59), at(10, 0, 0));
        check_incremental(led(false), at(11, 59, 59), at(12, 0, 0));
    }

    #[test]
    fn segment_table_matches_classic_shapes() {
        assert_eq!(DIGITS[8], 0x7F);
        assert_eq!(DIGITS[1], 0b0000110); // b, c
        assert_eq!(DIGITS[7], 0b0000111); // a, b, c
    }
}
