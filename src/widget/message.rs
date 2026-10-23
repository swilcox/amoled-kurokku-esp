//! Message widget: static, horizontal marquee, or vertical crawl.
//!
//! Text that fits is shown centered and held `STATIC_HOLD_MS` per repeat.
//! Otherwise it scrolls: horizontally as one line entering from the right,
//! or vertically as word-wrapped lines rising from the bottom. Position is
//! derived from elapsed time, so speed doesn't depend on frame rate.

use super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::rgb565;
use crate::font::Font;
use crate::fonts::{TEXT_L, TEXT_M, TEXT_S};
use crate::gfx::{Canvas, Rect, Scene, SCREEN};

const BG: u16 = rgb565(0, 0, 0);
const STATIC_HOLD_MS: u32 = 5000;
/// Horizontal margin for static and wrapped text.
const MARGIN: i32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Up,
}

impl Direction {
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(str::to_ascii_lowercase).as_deref() {
            Some("up" | "vertical") => Direction::Up,
            _ => Direction::Left,
        }
    }
}

/// Font for a size name. Defaults depend on direction: a one-line marquee
/// reads best large; wrapped text needs to fit several lines.
pub fn font_for(size: Option<&str>, dir: Direction) -> &'static Font {
    match (size.map(str::to_ascii_lowercase).as_deref(), dir) {
        (Some("small"), _) => &TEXT_S,
        (Some("medium"), _) | (None, Direction::Up) => &TEXT_M,
        _ => &TEXT_L,
    }
}

/// Greedy word wrap to `width` pixels; words wider than a line are split.
pub fn wrap(font: &Font, text: &str, width: i32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if font.text_width(&candidate) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            for c in word.chars() {
                if !line.is_empty() && font.text_width(&format!("{line}{c}")) > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(c);
            }
        }
        lines.push(line);
    }
    lines
}

/// Replace characters the font lacks with '?'.
fn sanitize(font: &Font, text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' || font.glyph(c).is_some() { c } else { '?' })
        .collect()
}

enum Mode {
    /// Lines centered as a block.
    Static,
    /// One line; `pos` counts pixels scrolled, over `travel` per pass.
    Horizontal { width: i32 },
    /// Wrapped lines rising from the bottom.
    Vertical,
}

pub struct Message {
    font: &'static Font,
    color: u16,
    lines: Vec<String>,
    mode: Mode,
    px_per_ms: f32,
    repeats: i32,
    line_h: i32,
    start_ms: Option<u32>,
    /// Pixels scrolled within the current pass; None = needs full repaint.
    pos: Option<i32>,
}

impl Message {
    pub fn new(text: &str, font: &'static Font, color: u16, dir: Direction, px_per_ms: f32, repeats: i32) -> Self {
        let text = sanitize(font, text);
        let line_h = (font.ascent - font.descent) as i32 * 11 / 10;
        let avail = LCD_W as i32 - 2 * MARGIN;
        let (lines, mode) = match dir {
            Direction::Left => {
                let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let width = font.text_width(&one_line);
                let mode = if width <= avail { Mode::Static } else { Mode::Horizontal { width } };
                (vec![one_line], mode)
            }
            Direction::Up => {
                let lines = wrap(font, &text, avail);
                let mode = if lines.len() as i32 * line_h <= LCD_H as i32 { Mode::Static } else { Mode::Vertical };
                (lines, mode)
            }
        };
        Self { font, color, lines, mode, px_per_ms, repeats, line_h, start_ms: None, pos: None }
    }

    /// Pixels per pass for scrolling modes.
    fn travel(&self) -> i32 {
        match self.mode {
            Mode::Static => 0,
            Mode::Horizontal { width } => LCD_W as i32 + width,
            Mode::Vertical => LCD_H as i32 + self.lines.len() as i32 * self.line_h,
        }
    }

    fn block_top(&self) -> i32 {
        (LCD_H as i32 - self.lines.len() as i32 * self.line_h) / 2
    }

    /// Baseline of the first line, given the top of the text block.
    fn first_baseline(&self, top: i32) -> i32 {
        top + (self.line_h - (self.font.ascent - self.font.descent) as i32) / 2 + self.font.ascent as i32
    }

    /// The rows the single horizontal line occupies.
    fn line_band(&self) -> Rect {
        let top = self.block_top();
        Rect::new(0, top - 4, SCREEN.x1, top + self.line_h + 4)
    }
}

impl Widget for Message {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let elapsed = now_ms.wrapping_sub(*self.start_ms.get_or_insert(now_ms));
        let (pass, pos) = match self.mode {
            Mode::Static => ((elapsed / STATIC_HOLD_MS) as i32, 0),
            _ => {
                let dist = (elapsed as f32 * self.px_per_ms) as i32;
                (dist / self.travel(), dist % self.travel())
            }
        };
        if self.repeats >= 0 && pass >= self.repeats {
            return Step::Done;
        }
        let rects = match self.pos.replace(pos) {
            None => vec![SCREEN],
            Some(old) if old == pos => vec![],
            Some(_) => match self.mode {
                Mode::Horizontal { .. } => vec![self.line_band()],
                _ => vec![SCREEN],
            },
        };
        Step::Redraw(rects)
    }

    fn invalidate(&mut self) {
        self.pos = None;
    }

    fn frame_ms(&self) -> u32 {
        // Scrolling: redraw as often as possible. Each frame takes ~12 ms to
        // render and send, so this runs ~75 fps — small steps per frame keep
        // motion smooth, and any tear is only a few pixels.
        match self.mode {
            Mode::Static => 100,
            _ => 1,
        }
    }
}

impl Scene for Message {
    fn draw(&self, c: &mut Canvas) {
        c.fill(c.area(), BG);
        let pos = self.pos.unwrap_or(0);
        let cx = LCD_W as i32 / 2;
        match self.mode {
            Mode::Static => {
                let base = self.first_baseline(self.block_top());
                for (i, line) in self.lines.iter().enumerate() {
                    c.text_centered(self.font, cx, base + i as i32 * self.line_h, line, self.color);
                }
            }
            Mode::Horizontal { .. } => {
                let base = self.first_baseline(self.block_top());
                c.text(self.font, LCD_W as i32 - pos, base, &self.lines[0], self.color);
            }
            Mode::Vertical => {
                let base = self.first_baseline(LCD_H as i32 - pos);
                let area = c.area();
                for (i, line) in self.lines.iter().enumerate() {
                    let y = base + i as i32 * self.line_h;
                    if y - self.line_h <= area.y1 && y + self.line_h >= area.y0 {
                        c.text_centered(self.font, cx, y, line, self.color);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::WHITE;
    use crate::gfx::{MemPanel, Renderer};

    const T: LocalTime = LocalTime { year: 2026, month: 10, day: 22, weekday: 4, hour: 12, minute: 0, second: 0 };

    fn msg(text: &str, dir: Direction, repeats: i32) -> Message {
        Message::new(text, font_for(None, dir), WHITE, dir, 0.3, repeats)
    }

    #[test]
    fn wrap_respects_width_and_splits_long_words() {
        let lines = wrap(&TEXT_M, "the quick brown fox jumps over the lazy dog", 432);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| TEXT_M.text_width(l) <= 432));
        assert_eq!(lines.join(" "), "the quick brown fox jumps over the lazy dog");
        let long = wrap(&TEXT_M, "Supercalifragilisticexpialidocious", 200);
        assert!(long.len() > 1 && long.iter().all(|l| TEXT_M.text_width(l) <= 200));
    }

    #[test]
    fn short_text_is_static_and_held_per_repeat() {
        let mut m = msg("72°F", Direction::Left, 2);
        assert!(matches!(m.mode, Mode::Static));
        assert!(matches!(m.tick(0, &T), Step::Redraw(r) if r == vec![SCREEN]));
        assert!(matches!(m.tick(9_999, &T), Step::Redraw(r) if r.is_empty()));
        assert!(matches!(m.tick(10_000, &T), Step::Done));
    }

    #[test]
    fn long_text_scrolls_and_finishes_after_repeats() {
        let mut m = msg("This message is far too long to fit on one line", Direction::Left, 1);
        let travel = m.travel();
        assert!(matches!(m.mode, Mode::Horizontal { .. }));
        assert!(matches!(m.tick(0, &T), Step::Redraw(r) if r == vec![SCREEN]));
        assert!(matches!(m.tick(100, &T), Step::Redraw(r) if r == vec![m.line_band()]));
        let done_at = (travel as f32 / 0.3) as u32 + 10;
        assert!(matches!(m.tick(done_at, &T), Step::Done));
    }

    #[test]
    fn infinite_repeats_never_finish() {
        let mut m = msg("This message is far too long to fit on one line", Direction::Left, -1);
        m.tick(0, &T);
        assert!(matches!(m.tick(10_000_000, &T), Step::Redraw(_)));
    }

    #[test]
    fn horizontal_band_redraw_matches_full_render() {
        let text = "This message is far too long to fit on one line";
        let mut m = msg(text, Direction::Left, -1);
        let mut panel = MemPanel::default();
        let mut r = Renderer::new(480 * 32);
        for ms in [0, 700, 1400] {
            if let Step::Redraw(rects) = m.tick(ms, &T) {
                r.render(&mut panel, &m, &rects).unwrap();
            }
        }
        let mut full = MemPanel::default();
        r.render(&mut full, &m, &[SCREEN]).unwrap();
        assert!(panel.pixels == full.pixels);
    }

    #[test]
    fn missing_glyphs_become_question_marks() {
        assert_eq!(sanitize(&TEXT_S, "a→b"), "a?b");
    }
}
