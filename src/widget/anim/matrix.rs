//! `matrix`: digital rain. Each column has a drop of mirrored half-width
//! katakana falling at its own speed, a white-hot glyph at its head and a
//! green trail fading behind; glyphs in the trails keep changing.
//!
//! The screen is a grid of glyph cells; only cells whose glyph or
//! brightness changed are redrawn, merged into runs down each column.

use super::super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::rgb565;
use crate::fonts::MATRIX;
use crate::gfx::{Canvas, Rect, Scene, SCREEN};
use crate::rng::Rng;

const CELL_W: i32 = 20;
const CELL_H: i32 = 24;
const COLS: i32 = LCD_W as i32 / CELL_W;
const ROWS: i32 = LCD_H as i32 / CELL_H;
/// Brightness levels: 0 = dark, 1..=TRAIL_LEVELS trail, HEAD the head.
const TRAIL_LEVELS: u8 = 15;
const HEAD: u8 = TRAIL_LEVELS + 1;
/// Glyph swaps per tick, among all cells (only lit ones show).
const SWAPS: u32 = 8;
/// Drops fall between this many rows per second.
const FALL: (i32, i32) = (13, 36);
/// Trail lengths, rows (the screen is 20 rows).
const TRAIL: (i32, i32) = (14, 32);

struct Drop {
    /// Head row in 1/256 cells, speed in 1/256 cells per second.
    head: i32,
    speed: i32,
    trail: i32,
}

pub struct Matrix {
    rng: Rng,
    glyphs: &'static [crate::font::Glyph],
    colors: [u16; HEAD as usize + 1],
    drops: Vec<Drop>,
    /// Per cell (row-major): glyph index, and (glyph, level) as drawn.
    cell: Vec<u8>,
    drawn: Vec<(u8, u8)>,
    last_ms: Option<u32>,
    first: bool,
}

impl Matrix {
    pub fn new(mut rng: Rng) -> Self {
        let glyphs = MATRIX.glyphs;
        let n = glyphs.len() as u32;
        let cell = (0..COLS * ROWS).map(|_| rng.below(n) as u8).collect();
        let mut colors = [0; HEAD as usize + 1];
        for l in 1..=TRAIL_LEVELS {
            let k = l as f32 / TRAIL_LEVELS as f32;
            // A gentle fade, so the back of a long trail is still legible.
            colors[l as usize] = rgb565((20.0 * k) as u8, (45.0 + 210.0 * k.powf(1.4)) as u8, (15.0 + 55.0 * k) as u8);
        }
        colors[HEAD as usize] = rgb565(215, 255, 220);
        let mut m = Self { rng, glyphs, colors, drops: Vec::new(), cell, drawn: vec![(0, 0); (COLS * ROWS) as usize], last_ms: None, first: true };
        m.drops = (0..COLS).map(|_| m.new_drop(true)).collect();
        m
    }

    /// A drop starting above the screen (anywhere down it, at the start).
    fn new_drop(&mut self, anywhere: bool) -> Drop {
        let start = if anywhere { self.rng.range(-ROWS, ROWS) } else { -self.rng.range(1, ROWS / 2) };
        Drop { head: start * 256, speed: self.rng.range(FALL.0 * 256, FALL.1 * 256), trail: self.rng.range(TRAIL.0, TRAIL.1) }
    }

    /// Brightness level each cell should have now.
    fn level(&self, col: i32, row: i32) -> u8 {
        let d = &self.drops[col as usize];
        let back = (d.head >> 8) - row;
        match back {
            0 => HEAD,
            b if b > 0 && b < d.trail => (TRAIL_LEVELS as i32 - (b - 1) * TRAIL_LEVELS as i32 / d.trail) as u8,
            _ => 0,
        }
    }

    fn cell_rect(col: i32, row: i32) -> Rect {
        Rect::from_size(col * CELL_W, row * CELL_H, CELL_W, CELL_H)
    }
}

impl Widget for Matrix {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let dt = now_ms.wrapping_sub(self.last_ms.unwrap_or(now_ms)).min(100) as i32;
        self.last_ms = Some(now_ms);
        for col in 0..COLS as usize {
            self.drops[col].head += self.drops[col].speed * dt / 1000;
            let d = &self.drops[col];
            if (d.head >> 8) - d.trail > ROWS {
                self.drops[col] = self.new_drop(false);
            }
        }
        for _ in 0..SWAPS {
            let i = self.rng.below((COLS * ROWS) as u32) as usize;
            self.cell[i] = self.rng.below(self.glyphs.len() as u32) as u8;
        }

        let first = std::mem::replace(&mut self.first, false);
        let mut dirty = Vec::new();
        for col in 0..COLS {
            // Runs of changed cells down this column.
            let mut run: Option<(i32, i32)> = None;
            for row in 0..ROWS {
                let i = (row * COLS + col) as usize;
                let want = (self.cell[i], self.level(col, row));
                let (was_glyph, was_level) = self.drawn[i];
                // A dark cell's glyph doesn't matter.
                let changed = want.1 != was_level || (want.1 > 0 && want.0 != was_glyph);
                self.drawn[i] = want;
                if changed {
                    run = Some(run.map_or((row, row), |(a, _)| (a, row)));
                } else if let Some((a, b)) = run.take() {
                    dirty.push(Self::cell_rect(col, a).union(&Self::cell_rect(col, b)));
                }
            }
            if let Some((a, b)) = run {
                dirty.push(Self::cell_rect(col, a).union(&Self::cell_rect(col, b)));
            }
        }
        Step::Redraw(if first { vec![SCREEN] } else { dirty })
    }

    fn invalidate(&mut self) {
        self.first = true;
    }

    fn frame_ms(&self) -> u32 {
        20
    }
}

impl Scene for Matrix {
    fn draw(&self, c: &mut Canvas) {
        let a = c.area();
        c.fill(a, 0);
        let (c0, c1) = ((a.x0 / CELL_W).max(0), (a.x1 / CELL_W).min(COLS - 1));
        let (r0, r1) = ((a.y0 / CELL_H).max(0), (a.y1 / CELL_H).min(ROWS - 1));
        let baseline = (CELL_H + MATRIX.ascent as i32 + MATRIX.descent as i32) / 2;
        for row in r0..=r1 {
            for col in c0..=c1 {
                let (glyph, level) = self.drawn[(row * COLS + col) as usize];
                if level == 0 {
                    continue;
                }
                let g = &self.glyphs[glyph as usize];
                let x = col * CELL_W + (CELL_W - g.w as i32) / 2 - g.left as i32;
                // Kept inside its cell, so redrawing a neighbor can't cut it.
                c.set_clip(Self::cell_rect(col, row));
                c.glyph(&MATRIX, g, x, row * CELL_H + baseline, self.colors[level as usize]);
            }
        }
        c.reset_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::check_incremental;
    use super::*;

    #[test]
    fn incremental_redraws_match() {
        let t = LocalTime::default();
        check_incremental(Box::new(Matrix::new(Rng::new(4))), (0..250u32).map(|i| (i * 20, t)));
    }

    #[test]
    fn heads_are_brightest_and_trails_fade() {
        let mut m = Matrix::new(Rng::new(1));
        m.drops[0] = Drop { head: 10 * 256, speed: 0, trail: 8 };
        assert_eq!(m.level(0, 10), HEAD);
        assert_eq!(m.level(0, 9), TRAIL_LEVELS);
        assert!(m.level(0, 5) < m.level(0, 8));
        assert_eq!(m.level(0, 2), 0);
        assert_eq!(m.level(0, 11), 0);
    }
}
