//! Status screen: a big title with an optional detail line, e.g. the boot
//! banner, "WIFI" while joining, or the IP address once connected.

use super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::rgb565;
use crate::fonts::{BANNER, TEXT_S};
use crate::gfx::{Canvas, Scene, SCREEN};

const BG: u16 = rgb565(0, 0, 0);
const TITLE: u16 = rgb565(235, 235, 235);
const DETAIL: u16 = rgb565(255, 140, 0);
const GAP: i32 = 24;

pub struct Status {
    title: String,
    detail: String,
    /// None = until replaced.
    hold_ms: Option<u32>,
    start_ms: Option<u32>,
    drawn: bool,
}

impl Status {
    /// `title` uses the caps-only banner font, so it is upper-cased.
    pub fn new(title: &str, detail: &str, hold_ms: Option<u32>) -> Self {
        Self {
            title: title.to_uppercase(),
            detail: detail.to_string(),
            hold_ms,
            start_ms: None,
            drawn: false,
        }
    }
}

impl Widget for Status {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let start = *self.start_ms.get_or_insert(now_ms);
        if self.hold_ms.is_some_and(|h| now_ms.wrapping_sub(start) >= h) {
            return Step::Done;
        }
        if std::mem::replace(&mut self.drawn, true) {
            Step::Redraw(vec![])
        } else {
            Step::Redraw(vec![SCREEN])
        }
    }

    fn invalidate(&mut self) {
        self.drawn = false;
    }

    fn frame_ms(&self) -> u32 {
        100
    }
}

impl Scene for Status {
    fn draw(&self, c: &mut Canvas) {
        c.fill(c.area(), BG);
        let (tt, tb) = BANNER.ink_rows("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
        let title_h = tb - tt;
        let detail_h = if self.detail.is_empty() { 0 } else { GAP + (TEXT_S.ascent - TEXT_S.descent) as i32 };
        let top = (LCD_H as i32 - title_h - detail_h) / 2;
        let cx = LCD_W as i32 / 2;
        c.text_centered(&BANNER, cx, top - tt, &self.title, TITLE);
        if !self.detail.is_empty() {
            c.text_centered(&TEXT_S, cx, top + title_h + GAP + TEXT_S.ascent as i32, &self.detail, DETAIL);
        }
    }
}
