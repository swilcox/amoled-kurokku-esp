//! "Nixie" face: four glowing tubes for HH:MM with neon colon dots, the date
//! above and seconds below in amber. Changing digits crossfade the way a
//! cathode's glow decays.

use super::{display_hour, Face, LocalTime, MONTHS, WEEKDAYS};
use crate::board::LCD_W;
use crate::color::rgb565;
use crate::fonts::{CLOCK_DATE, CLOCK_SECS};
use crate::gfx::{Canvas, Rect, Scene, SCREEN};
use crate::sprites::nixie::{DOT, DOT_GLOW, DOT_H, DOT_W, GLOW, PALETTE, TUBE, TUBE_H, TUBE_W};

const BG: u16 = rgb565(0, 0, 0);
const DATE: u16 = rgb565(160, 72, 18);
const SECS: u16 = rgb565(235, 110, 25);

const PAIR_GAP: i32 = 6;
const COLON_W: i32 = 30;
const LINE_GAP: i32 = 22;
const FADE_MS: u32 = 200;

struct Fade {
    start_ms: u32,
    from: Option<u8>,
}

struct Tube {
    x: i32,
    digit: Option<u8>,
    fade: Option<Fade>,
}

pub struct NixieFace {
    format_24h: bool,
    tubes: [Tube; 4],
    tube_y: i32,
    colon_x: i32,
    date_base: i32,
    secs_base: i32,
    date_rect: Rect,
    secs_rect: Rect,
    t: LocalTime,
    now_ms: u32,
    shown: Option<LocalTime>,
    /// Whether the tubes have shown a time yet (so changes should fade).
    primed: bool,
    off: (i32, i32),
}

impl NixieFace {
    pub fn new(format_24h: bool) -> Self {
        let tw = TUBE_W as i32;
        let total = 4 * tw + 2 * PAIR_GAP + COLON_W;
        let x0 = (LCD_W as i32 - total) / 2;
        let xs = [x0, x0 + tw + PAIR_GAP, x0 + 2 * tw + PAIR_GAP + COLON_W, x0 + 3 * tw + 2 * PAIR_GAP + COLON_W];

        let caps = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let (dt, db) = CLOCK_DATE.ink_rows(caps);
        let (st, sb) = CLOCK_SECS.ink_rows("0123456789APM");
        let block = (db - dt) + LINE_GAP + TUBE_H as i32 + LINE_GAP + (sb - st);
        let top = (SCREEN.height() - block) / 2;
        let date_base = top - dt;
        let tube_y = date_base + db + LINE_GAP;
        let secs_base = tube_y + TUBE_H as i32 + LINE_GAP - st;
        let band = |base: i32, (t, b): (i32, i32)| Rect::new(0, base + t - 4, SCREEN.x1, base + b + 3);
        Self {
            format_24h,
            tubes: xs.map(|x| Tube { x, digit: None, fade: None }),
            tube_y,
            colon_x: x0 + 2 * tw + PAIR_GAP,
            date_base,
            secs_base,
            date_rect: band(date_base, (dt, db)),
            secs_rect: band(secs_base, (st, sb)),
            t: LocalTime::default(),
            now_ms: 0,
            shown: None,
            primed: false,
            off: (0, 0),
        }
    }

    /// Lit digit per tube; a 12h hour below 10 leaves the first tube dark.
    fn digits(&self, t: &LocalTime) -> [Option<u8>; 4] {
        let (h, _) = display_hour(t.hour, self.format_24h);
        let tens = (self.format_24h || h >= 10).then_some(h / 10);
        [tens, Some(h % 10), Some(t.minute / 10), Some(t.minute % 10)]
    }

    fn colon_on(t: &LocalTime) -> bool {
        t.second % 2 == 0
    }

    fn secs_text(&self, t: &LocalTime) -> String {
        match display_hour(t.hour, self.format_24h) {
            _ if self.format_24h => format!("{:02}", t.second),
            (_, true) => format!("{:02} PM", t.second),
            (_, false) => format!("{:02} AM", t.second),
        }
    }

    fn date_text(t: &LocalTime) -> String {
        let wd = WEEKDAYS.get(t.weekday as usize).copied().unwrap_or("");
        let mon = MONTHS.get(t.month.wrapping_sub(1) as usize).copied().unwrap_or("");
        format!("{wd}  {mon} {}", t.day)
    }

    fn tube_rect(&self, tube: &Tube) -> Rect {
        Rect::from_size(tube.x, self.tube_y, TUBE_W as i32, TUBE_H as i32)
    }

    /// The two colon dots, at 1/3 and 2/3 of the cathode area.
    fn dot_positions(&self) -> [(i32, i32); 2] {
        let x = self.colon_x + (COLON_W - DOT_W as i32) / 2;
        let y = |frac: f32| self.tube_y + (TUBE_H as f32 * frac) as i32 - DOT_H as i32 / 2;
        [(x, y(0.38)), (x, y(0.66))]
    }

    fn colon_rect(&self) -> Rect {
        let [(x, y0), (_, y1)] = self.dot_positions();
        Rect::new(x, y0, x + DOT_W as i32 - 1, y1 + DOT_H as i32 - 1)
    }
}

impl Face for NixieFace {
    fn update(&mut self, now_ms: u32, t: &LocalTime) -> Vec<Rect> {
        self.now_ms = now_ms;
        self.t = *t;
        let digits = self.digits(t);
        let Some(prev) = self.shown.replace(*t) else {
            // Full repaint: first frame, or after `invalidate` (burn-in
            // shift, timezone change). Once primed, changed tubes still fade.
            let primed = std::mem::replace(&mut self.primed, true);
            for (tube, d) in self.tubes.iter_mut().zip(digits) {
                if primed && tube.digit != d {
                    tube.fade = Some(Fade { start_ms: now_ms, from: tube.digit });
                }
                tube.digit = d;
            }
            return vec![SCREEN];
        };

        let mut dirty = Vec::new();
        for i in 0..4 {
            let tube = &mut self.tubes[i];
            if tube.digit != digits[i] {
                tube.fade = Some(Fade { start_ms: now_ms, from: tube.digit });
                tube.digit = digits[i];
            }
            if let Some(f) = &tube.fade {
                if now_ms.wrapping_sub(f.start_ms) >= FADE_MS {
                    tube.fade = None;
                }
                dirty.push(self.tube_rect(&self.tubes[i]));
            }
        }
        if Self::colon_on(&prev) != Self::colon_on(t) {
            dirty.push(self.colon_rect());
        }
        if self.secs_text(&prev) != self.secs_text(t) {
            dirty.push(self.secs_rect);
        }
        if Self::date_text(&prev) != Self::date_text(t) {
            dirty.push(self.date_rect);
        }
        let (dx, dy) = self.off;
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

    fn animating(&self) -> bool {
        self.tubes.iter().any(|t| t.fade.is_some())
    }
}

impl Scene for NixieFace {
    fn draw(&self, c: &mut Canvas) {
        let (dx, dy) = self.off;
        let cx = LCD_W as i32 / 2 + dx;
        c.fill(c.area(), BG);
        if c.area().intersect(&self.date_rect.translate(dx, dy)).is_some() {
            c.text_centered(&CLOCK_DATE, cx, self.date_base + dy, &Self::date_text(&self.t), DATE);
        }

        for tube in &self.tubes {
            let (x, y) = (tube.x + dx, self.tube_y + dy);
            if c.area().intersect(&self.tube_rect(tube).translate(dx, dy)).is_none() {
                continue;
            }
            // Crossfade: the old cathode dims while the new one lights.
            let p = tube.fade.as_ref().map_or(15, |f| {
                (self.now_ms.wrapping_sub(f.start_ms) * 15 / FADE_MS).min(15) as u8
            });
            let mut layers: Vec<(&[u8], u8)> = Vec::with_capacity(2);
            if let Some(d) = tube.digit {
                layers.push((GLOW[d as usize].bytes(), p));
            }
            if let Some(Some(d)) = tube.fade.as_ref().map(|f| f.from) {
                layers.push((GLOW[d as usize].bytes(), 15 - p));
            }
            c.blit_glow(x, y, TUBE_W, TUBE_H, TUBE.bytes(), &layers, &PALETTE);
        }

        let on = Self::colon_on(&self.t);
        for (x, y) in self.dot_positions() {
            let glow = [(DOT_GLOW.bytes(), 15)];
            let layers: &[(&[u8], u8)] = if on { &glow } else { &[] };
            c.blit_glow(x + dx, y + dy, DOT_W, DOT_H, DOT.bytes(), layers, &PALETTE);
        }

        if c.area().intersect(&self.secs_rect.translate(dx, dy)).is_some() {
            c.text_centered(&CLOCK_SECS, cx, self.secs_base + dy, &self.secs_text(&self.t), SECS);
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
    fn incremental_matches_full_render_after_fade() {
        let nixie = |h24| move || Box::new(NixieFace::new(h24)) as Box<dyn Face>;
        check_incremental(nixie(true), at(12, 34, 56), at(12, 34, 57));
        check_incremental(nixie(true), at(12, 59, 59), at(13, 0, 0));
        check_incremental(nixie(false), at(9, 59, 59), at(10, 0, 0));
    }

    #[test]
    fn fades_even_when_a_full_repaint_lands_on_the_minute() {
        let mut f = NixieFace::new(true);
        f.update(0, &at(12, 23, 59));
        f.set_offset(0, 3);
        f.update(1000, &at(12, 24, 0));
        assert!(f.animating());
    }

    #[test]
    fn layout_fits_on_screen() {
        let f = NixieFace::new(true);
        assert!(f.tubes[0].x >= 0 && f.tubes[3].x + TUBE_W as i32 <= 480);
        assert!(f.date_rect.y0 >= 0 && f.secs_rect.y1 < 480);
        assert_eq!(GLOW[0].bytes().len(), TUBE_W.div_ceil(2) * TUBE_H);
    }
}
