//! "Flip" face: split-flap cards for hours and minutes. When a card changes,
//! the old top half folds down over the hinge (revealing the new top behind
//! it), then the new bottom half unfolds over the old bottom.

use super::{display_hour, Face, LocalTime, MONTHS, WEEKDAYS};
use crate::board::LCD_W;
use crate::color::rgb565;
use crate::fonts::{CLOCK_DATE, CLOCK_SECS, FLIP};
use crate::gfx::{blend565, Canvas, Rect, Scene, SCREEN};

const BG: u16 = rgb565(0, 0, 0);
const UPPER: (u16, u16) = (rgb565(52, 52, 55), rgb565(38, 38, 41));
const LOWER: (u16, u16) = (rgb565(34, 34, 37), rgb565(24, 24, 27));
const DIGIT: u16 = rgb565(236, 236, 230);
const DATE: u16 = rgb565(125, 125, 125);
const SECS: u16 = rgb565(150, 150, 150);

const CARD_W: i32 = 212;
const CARD_H: i32 = 250;
const CARD_R: i32 = 18;
const CARD_GAP: i32 = 20;
const LINE_GAP: i32 = 26;
/// Full flip: top half falls, then bottom half lands.
const FLIP_MS: u32 = 600;

#[derive(Clone, Copy, PartialEq)]
enum Half {
    Top,
    Bottom,
}

struct Flip {
    start_ms: u32,
    from: String,
}

struct Card {
    rect: Rect,
    text: String,
    flip: Option<Flip>,
}

impl Card {
    fn hinge(&self) -> i32 {
        self.rect.y0 + CARD_H / 2
    }
}

struct Layout {
    date_base: i32,
    secs_base: i32,
    digit_base: i32,
    cell_w: i32,
    date_rect: Rect,
    secs_rect: Rect,
}

impl Layout {
    fn new(cards_top: &mut i32) -> Self {
        let caps = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let (dt, db) = CLOCK_DATE.ink_rows(caps);
        let (st, sb) = CLOCK_SECS.ink_rows("0123456789APM");
        let (gt, gb) = FLIP.ink_rows("0123456789");
        let block = (db - dt) + LINE_GAP + CARD_H + LINE_GAP + (sb - st);
        let top = (SCREEN.height() - block) / 2;
        let date_base = top - dt;
        *cards_top = date_base + db + LINE_GAP;
        let secs_base = *cards_top + CARD_H + LINE_GAP - st;
        let band = |base: i32, (t, b): (i32, i32)| Rect::new(0, base + t - 4, SCREEN.x1, base + b + 3);
        Self {
            date_base,
            secs_base,
            // Digits vertically centered on the card.
            digit_base: *cards_top + (CARD_H - (gb - gt)) / 2 - gt,
            cell_w: "0123456789".chars().map(|c| FLIP.advance(c)).max().unwrap_or(0),
            date_rect: band(date_base, (dt, db)),
            secs_rect: band(secs_base, (st, sb)),
        }
    }
}

pub struct FlipFace {
    format_24h: bool,
    layout: Layout,
    cards: [Card; 2],
    t: LocalTime,
    now_ms: u32,
    shown: Option<LocalTime>,
    off: (i32, i32),
}

impl FlipFace {
    pub fn new(format_24h: bool) -> Self {
        let mut top = 0;
        let layout = Layout::new(&mut top);
        let x0 = (LCD_W as i32 - (2 * CARD_W + CARD_GAP)) / 2;
        let card = |x| Card { rect: Rect::from_size(x, top, CARD_W, CARD_H), text: String::new(), flip: None };
        Self {
            format_24h,
            layout,
            cards: [card(x0), card(x0 + CARD_W + CARD_GAP)],
            t: LocalTime::default(),
            now_ms: 0,
            shown: None,
            off: (0, 0),
        }
    }

    fn texts(&self, t: &LocalTime) -> [String; 2] {
        let (h, _) = display_hour(t.hour, self.format_24h);
        let hours = if self.format_24h { format!("{h:02}") } else { h.to_string() };
        [hours, format!("{:02}", t.minute)]
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

    /// Draw one half of a card showing `text`, squashed towards the hinge by
    /// `scale` and darkened by `shade` (1 = full brightness).
    fn draw_half(&self, c: &mut Canvas, card: &Rect, hinge: i32, text: &str, half: Half, scale: f32, shade: f32) {
        // A 2 px black split runs along the hinge (rows hinge-1 and hinge).
        let (near, far) = match half {
            Half::Top => (hinge - 2, card.y0),
            Half::Bottom => (hinge + 1, card.y1),
        };
        let far = near + ((far - near) as f32 * scale).round() as i32;
        let rect = Rect::new(card.x0, near.min(far), card.x1, near.max(far));
        let k = (shade.clamp(0.0, 1.0) * 15.0).round() as u8;
        let dim = |col: u16| blend565(BG, col, k);
        let (c0, c1) = if half == Half::Top { UPPER } else { LOWER };
        c.fill_round_rect(rect, CARD_R, dim(c0), dim(c1), half == Half::Top, half == Half::Bottom);

        c.set_clip(rect);
        let cell = self.layout.cell_w;
        let mut pen = card.x0 + (CARD_W - cell * text.chars().count() as i32) / 2;
        let base = self.layout.digit_base + (card.y0 - self.cards[0].rect.y0);
        for ch in text.chars() {
            if let Some(g) = FLIP.glyph(ch) {
                let x = pen + (cell - g.advance as i32) / 2;
                c.glyph_scaled(&FLIP, g, x, base, dim(DIGIT), near, scale);
            }
            pen += cell;
        }
        c.reset_clip();
    }

    fn draw_card(&self, c: &mut Canvas, card: &Card, dx: i32, dy: i32) {
        let rect = card.rect.translate(dx, dy);
        if c.area().intersect(&rect).is_none() {
            return;
        }
        let hinge = card.hinge() + dy;
        let Some(flip) = &card.flip else {
            self.draw_half(c, &rect, hinge, &card.text, Half::Top, 1.0, 1.0);
            self.draw_half(c, &rect, hinge, &card.text, Half::Bottom, 1.0, 1.0);
            return;
        };
        let p = (self.now_ms.wrapping_sub(flip.start_ms) as f32 / FLIP_MS as f32).min(1.0);
        // Behind the flap: new top, old bottom.
        self.draw_half(c, &rect, hinge, &card.text, Half::Top, 1.0, 1.0);
        self.draw_half(c, &rect, hinge, &flip.from, Half::Bottom, 1.0, 1.0);
        let quarter = core::f32::consts::FRAC_PI_2;
        if p < 0.5 {
            // Old top falls towards the hinge, darkening as it tilts away.
            let q = p * 2.0;
            self.draw_half(c, &rect, hinge, &flip.from, Half::Top, (q * quarter).cos(), 1.0 - 0.6 * q);
        } else {
            // New bottom swings down from the hinge, brightening as it lands.
            let q = (p - 0.5) * 2.0;
            self.draw_half(c, &rect, hinge, &card.text, Half::Bottom, (q * quarter).sin(), 0.4 + 0.6 * q);
        }
    }
}

impl Face for FlipFace {
    fn update(&mut self, now_ms: u32, t: &LocalTime) -> Vec<Rect> {
        self.now_ms = now_ms;
        self.t = *t;
        let texts = self.texts(t);
        let Some(prev) = self.shown.replace(*t) else {
            // Full repaint: first frame, or after `invalidate` (burn-in
            // shift, timezone change). Cards that were already showing
            // something still flip to a new value, and a flip in progress
            // carries on.
            for (card, text) in self.cards.iter_mut().zip(texts) {
                if !card.text.is_empty() && card.text != text {
                    let from = std::mem::replace(&mut card.text, text);
                    card.flip = Some(Flip { start_ms: now_ms, from });
                } else {
                    card.text = text;
                }
            }
            return vec![SCREEN];
        };

        let (dx, dy) = self.off;
        let mut dirty = Vec::new();
        for (card, text) in self.cards.iter_mut().zip(texts) {
            if card.text != text {
                let from = std::mem::replace(&mut card.text, text);
                card.flip = Some(Flip { start_ms: now_ms, from });
            }
            if let Some(f) = &card.flip {
                dirty.push(card.rect);
                if now_ms.wrapping_sub(f.start_ms) >= FLIP_MS {
                    card.flip = None;
                }
            }
        }
        if self.secs_text(&prev) != self.secs_text(t) {
            dirty.push(self.layout.secs_rect);
        }
        if Self::date_text(&prev) != Self::date_text(t) {
            dirty.push(self.layout.date_rect);
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

    fn animating(&self) -> bool {
        self.cards.iter().any(|c| c.flip.is_some())
    }
}

impl Scene for FlipFace {
    fn draw(&self, c: &mut Canvas) {
        let l = &self.layout;
        let (dx, dy) = self.off;
        let cx = LCD_W as i32 / 2 + dx;
        c.fill(c.area(), BG);
        if c.area().intersect(&l.date_rect.translate(dx, dy)).is_some() {
            c.text_centered(&CLOCK_DATE, cx, l.date_base + dy, &Self::date_text(&self.t), DATE);
        }
        for card in &self.cards {
            self.draw_card(c, card, dx, dy);
        }
        if c.area().intersect(&l.secs_rect.translate(dx, dy)).is_some() {
            c.text_centered(&CLOCK_SECS, cx, l.secs_base + dy, &self.secs_text(&self.t), SECS);
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
    fn incremental_matches_full_render_after_flip() {
        let flip = |h24| move || Box::new(FlipFace::new(h24)) as Box<dyn Face>;
        check_incremental(flip(true), at(12, 34, 56), at(12, 34, 57));
        check_incremental(flip(true), at(12, 59, 59), at(13, 0, 0));
        check_incremental(flip(false), at(9, 59, 59), at(10, 0, 0));
    }

    #[test]
    fn flip_animates_then_settles() {
        let mut f = FlipFace::new(true);
        f.update(0, &at(12, 34, 59));
        assert!(!f.animating());
        let rects = f.update(1000, &at(12, 35, 0));
        assert!(f.animating() && rects.contains(&f.cards[1].rect));
        f.update(1000 + FLIP_MS / 2, &at(12, 35, 0));
        assert!(f.animating());
        f.update(1000 + FLIP_MS, &at(12, 35, 0));
        assert!(!f.animating());
    }

    #[test]
    fn flips_even_when_a_full_repaint_lands_on_the_minute() {
        let mut f = FlipFace::new(true);
        f.update(0, &at(12, 23, 59));
        f.set_offset(3, 0);
        assert_eq!(f.update(1000, &at(12, 24, 0)), vec![SCREEN]);
        assert!(f.animating());
        assert!(f.update(1100, &at(12, 24, 0)).contains(&f.cards[1].rect.translate(3, 0)));
    }

    #[test]
    fn layout_fits_on_screen() {
        let f = FlipFace::new(true);
        assert!(f.cards[0].rect.x0 >= 0 && f.cards[1].rect.x1 < 480);
        assert!(f.layout.date_rect.y0 >= 0 && f.layout.secs_rect.y1 < 480);
    }
}
