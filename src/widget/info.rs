//! Info screen: the device's name over a table of facts (IP, signal,
//! firmware, uptime, …), shown on a KEY long press or a tap.

use super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::rgb565;
use crate::fonts::{TEXT_M, TEXT_XS};
use crate::gfx::{Canvas, Scene, SCREEN};

const BG: u16 = rgb565(0, 0, 0);
const TITLE: u16 = rgb565(255, 140, 0);
const LABEL: u16 = rgb565(140, 140, 140);
const VALUE: u16 = rgb565(235, 235, 235);
const MARGIN: i32 = 48;
const ROW_H: i32 = 44;
const TITLE_GAP: i32 = 18;

pub struct Info {
    title: String,
    rows: Vec<(String, String)>,
    hold_ms: u32,
    start_ms: Option<u32>,
    drawn: bool,
}

impl Info {
    pub fn new(title: &str, rows: Vec<(String, String)>, hold_ms: u32) -> Self {
        Self { title: title.to_string(), rows, hold_ms, start_ms: None, drawn: false }
    }
}

impl Widget for Info {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let start = *self.start_ms.get_or_insert(now_ms);
        if now_ms.wrapping_sub(start) >= self.hold_ms {
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

impl Scene for Info {
    fn draw(&self, c: &mut Canvas) {
        c.fill(c.area(), BG);
        let title_h = (TEXT_M.ascent - TEXT_M.descent) as i32;
        let height = title_h + TITLE_GAP + ROW_H * self.rows.len() as i32;
        let top = (LCD_H as i32 - height) / 2;
        c.text_centered(&TEXT_M, LCD_W as i32 / 2, top + TEXT_M.ascent as i32, &self.title, TITLE);
        let right = LCD_W as i32 - MARGIN;
        for (i, (label, value)) in self.rows.iter().enumerate() {
            let baseline = top + title_h + TITLE_GAP + ROW_H * i as i32 + TEXT_XS.ascent as i32;
            c.text(&TEXT_XS, MARGIN, baseline, label, LABEL);
            c.text(&TEXT_XS, right - TEXT_XS.text_width(value), baseline, value, VALUE);
        }
    }
}
