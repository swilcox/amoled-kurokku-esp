//! "Modern" face: date above, big HH:MM, seconds (and AM/PM) below, on black.

use super::{display_hour, Face, LocalTime, MONTHS, WEEKDAYS};
use crate::board::LCD_W;
use crate::color::rgb565;
use crate::fonts::{CLOCK_BIG, CLOCK_DATE, CLOCK_SECS};
use crate::gfx::{Canvas, Rect, Scene, SCREEN};

const BG: u16 = rgb565(0, 0, 0);
const DIGIT: u16 = rgb565(235, 235, 235);
const DATE: u16 = rgb565(130, 130, 130);
const ACCENT: u16 = rgb565(255, 140, 0);

/// Vertical space between the date, the digits and the seconds line.
const GAP: i32 = 28;
/// Extra margin around redraw regions, for antialiasing and overshoot.
const PAD: i32 = 4;

struct Layout {
    cell_w: i32,
    colon_w: i32,
    /// Left edge of each digit cell, with the colon between cells 1 and 2.
    cell_x: [i32; 4],
    colon_x: i32,
    digit_baseline: i32,
    date_baseline: i32,
    secs_baseline: i32,
    digit_rows: (i32, i32),
    date_rect: Rect,
    secs_rect: Rect,
}

impl Layout {
    fn new() -> Self {
        let digits = "0123456789";
        let cell_w = digits.chars().map(|c| CLOCK_BIG.advance(c)).max().unwrap_or(0);
        let colon_w = CLOCK_BIG.advance(':');
        let x0 = (LCD_W as i32 - (4 * cell_w + colon_w)) / 2;
        let cell_x = [x0, x0 + cell_w, x0 + 2 * cell_w + colon_w, x0 + 3 * cell_w + colon_w];

        // Center the three lines vertically using their ink heights.
        let (dt, db) = CLOCK_DATE.ink_rows("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
        let (gt, gb) = CLOCK_BIG.ink_rows(digits);
        let (st, sb) = CLOCK_SECS.ink_rows("0123456789APM");
        let block = (db - dt) + GAP + (gb - gt) + GAP + (sb - st);
        let top = (SCREEN.height() - block) / 2;

        let date_baseline = top - dt;
        let digit_baseline = date_baseline + db + GAP - gt;
        let secs_baseline = digit_baseline + gb + GAP - st;

        let band = |baseline: i32, (t, b): (i32, i32)| {
            Rect::new(0, baseline + t - PAD, SCREEN.x1, baseline + b - 1 + PAD)
        };
        Self {
            cell_w,
            colon_w,
            cell_x,
            colon_x: x0 + 2 * cell_w,
            digit_baseline,
            date_baseline,
            secs_baseline,
            digit_rows: (gt, gb),
            date_rect: band(date_baseline, (dt, db)),
            secs_rect: band(secs_baseline, (st, sb)),
        }
    }

    fn cell_rect(&self, i: usize, shift: i32) -> Rect {
        let (t, b) = self.digit_rows;
        Rect::new(
            self.cell_x[i] + shift - PAD,
            self.digit_baseline + t - PAD,
            self.cell_x[i] + shift + self.cell_w - 1 + PAD,
            self.digit_baseline + b - 1 + PAD,
        )
    }

    /// The full-width band holding the digits.
    fn digit_band(&self) -> Rect {
        let (t, b) = self.digit_rows;
        Rect::new(0, self.digit_baseline + t - PAD, SCREEN.x1, self.digit_baseline + b - 1 + PAD)
    }
}

pub struct ModernFace {
    format_24h: bool,
    layout: Layout,
    t: LocalTime,
    shown: Option<LocalTime>,
}

impl ModernFace {
    pub fn new(format_24h: bool) -> Self {
        Self { format_24h, layout: Layout::new(), t: LocalTime::default(), shown: None }
    }

    pub fn format_24h(&self) -> bool {
        self.format_24h
    }

    pub fn set_format_24h(&mut self, on: bool) {
        if on != self.format_24h {
            self.format_24h = on;
            self.invalidate();
        }
    }

    /// The four HH:MM characters; a 12h hour below 10 has no leading digit.
    fn digits(&self, t: &LocalTime) -> [Option<char>; 4] {
        let (h, _) = display_hour(t.hour, self.format_24h);
        let d = |n: u8| char::from(b'0' + n);
        let tens = if self.format_24h || h >= 10 { Some(d(h / 10)) } else { None };
        [tens, Some(d(h % 10)), Some(d(t.minute / 10)), Some(d(t.minute % 10))]
    }

    /// Horizontal offset of the digit row: a 12h hour below 10 has no
    /// leading digit, so shift H:MM left by half a cell to keep it centered.
    fn shift(&self, t: &LocalTime) -> i32 {
        if self.digits(t)[0].is_none() {
            -self.layout.cell_w / 2
        } else {
            0
        }
    }

    fn secs_text(&self, t: &LocalTime) -> String {
        let (_, pm) = display_hour(t.hour, self.format_24h);
        match (self.format_24h, pm) {
            (true, _) => format!("{:02}", t.second),
            (false, false) => format!("{:02} AM", t.second),
            (false, true) => format!("{:02} PM", t.second),
        }
    }

    fn date_text(t: &LocalTime) -> String {
        let wd = WEEKDAYS.get(t.weekday as usize).copied().unwrap_or("");
        let mon = MONTHS.get(t.month.wrapping_sub(1) as usize).copied().unwrap_or("");
        format!("{wd}  {mon} {}", t.day)
    }
}

impl Face for ModernFace {
    fn update(&mut self, t: &LocalTime) -> Vec<Rect> {
        self.t = *t;
        let Some(prev) = self.shown.replace(*t) else {
            return vec![SCREEN];
        };
        let mut dirty = Vec::new();
        let (old, new) = (self.digits(&prev), self.digits(t));
        let shift = self.shift(t);
        if shift != self.shift(&prev) {
            dirty.push(self.layout.digit_band());
        } else {
            for i in 0..4 {
                if old[i] != new[i] {
                    dirty.push(self.layout.cell_rect(i, shift));
                }
            }
        }
        if self.secs_text(&prev) != self.secs_text(t) {
            dirty.push(self.layout.secs_rect);
        }
        if Self::date_text(&prev) != Self::date_text(t) {
            dirty.push(self.layout.date_rect);
        }
        dirty
    }

    fn invalidate(&mut self) {
        self.shown = None;
    }
}

impl Scene for ModernFace {
    fn draw(&self, c: &mut Canvas) {
        let l = &self.layout;
        let cx = LCD_W as i32 / 2;
        c.fill(c.area(), BG);

        if c.area().intersect(&l.date_rect).is_some() {
            c.text_centered(&CLOCK_DATE, cx, l.date_baseline, &Self::date_text(&self.t), DATE);
        }

        let shift = self.shift(&self.t);
        for (i, ch) in self.digits(&self.t).into_iter().enumerate() {
            if let Some(g) = ch.and_then(|ch| CLOCK_BIG.glyph(ch)) {
                let pen = l.cell_x[i] + shift + (l.cell_w - g.advance as i32) / 2;
                c.glyph(&CLOCK_BIG, g, pen, l.digit_baseline, DIGIT);
            }
        }
        if let Some(g) = CLOCK_BIG.glyph(':') {
            let pen = l.colon_x + shift + (l.colon_w - g.advance as i32) / 2;
            c.glyph(&CLOCK_BIG, g, pen, l.digit_baseline, DIGIT);
        }

        if c.area().intersect(&l.secs_rect).is_some() {
            c.text_centered(&CLOCK_SECS, cx, l.secs_baseline, &self.secs_text(&self.t), ACCENT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::{MemPanel, Renderer};

    fn at(hour: u8, minute: u8, second: u8) -> LocalTime {
        LocalTime { year: 2026, month: 10, day: 22, weekday: 4, hour, minute, second }
    }

    fn full(face_24h: bool, t: &LocalTime) -> Vec<u16> {
        let mut face = ModernFace::new(face_24h);
        let mut panel = MemPanel::default();
        let rects = face.update(t);
        Renderer::new(480 * 32).render(&mut panel, &face, &rects).unwrap();
        panel.pixels
    }

    /// Rendering `from` in full and then only the dirty regions for `to`
    /// must give exactly the same pixels as rendering `to` in full.
    fn check_incremental(face_24h: bool, from: LocalTime, to: LocalTime) {
        let mut face = ModernFace::new(face_24h);
        let mut panel = MemPanel::default();
        let mut r = Renderer::new(480 * 32);
        let rects = face.update(&from);
        r.render(&mut panel, &face, &rects).unwrap();
        let rects = face.update(&to);
        r.render(&mut panel, &face, &rects).unwrap();
        assert!(panel.pixels == full(face_24h, &to), "{from:?} -> {to:?}");
    }

    #[test]
    fn layout_fits_on_screen() {
        let l = Layout::new();
        assert!(l.cell_x[0] >= 0 && l.cell_x[3] + l.cell_w <= LCD_W as i32);
        assert!(l.date_rect.y0 >= 0 && l.secs_rect.y1 < 480);
        assert!(l.date_rect.y1 < l.digit_band().y0 && l.digit_band().y1 < l.secs_rect.y0);
    }

    #[test]
    fn incremental_matches_full_render() {
        check_incremental(true, at(12, 34, 56), at(12, 34, 57));
        check_incremental(true, at(12, 59, 59), at(13, 0, 0));
        check_incremental(false, at(9, 59, 59), at(10, 0, 0));
        check_incremental(false, at(11, 59, 59), at(12, 0, 0));
        check_incremental(false, at(12, 59, 59), at(13, 0, 0));
        check_incremental(false, at(9, 41, 0), at(9, 42, 0));
        let mut next_day = at(0, 0, 0);
        next_day.day = 23;
        next_day.weekday = 5;
        check_incremental(true, at(23, 59, 59), next_day);
    }

    #[test]
    fn only_seconds_change_within_a_minute() {
        let mut face = ModernFace::new(true);
        face.update(&at(12, 34, 56));
        assert_eq!(face.update(&at(12, 34, 57)), vec![face.layout.secs_rect]);
        assert!(face.update(&at(12, 34, 57)).is_empty());
    }
}
