//! `sine`: an oscilloscope. A glowing green trace scrolls across a dim
//! graticule; its amplitude, wavelength and a touch of third harmonic drift
//! slowly so it never quite repeats.
//!
//! All per-column math is fixed point from a sine table (no FPU here), and
//! only 16-column strips spanning the old and new trace are redrawn.

use super::super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::rgb565;
use crate::gfx::blend565_8;
use crate::gfx::{Canvas, Rect, Scene, SCREEN};

const W: usize = LCD_W as usize;
const CY: i32 = LCD_H as i32 / 2;
/// Sine table: one turn in this many entries, Q14 values.
const TABLE: usize = 1024;
/// Graticule pitch, and the center axes' tick pitch.
const DIV: i32 = 60;
const TICK: i32 = 12;
/// How far the glow reaches, px.
const GLOW: i32 = 10;
const STRIP: i32 = 8;

const GRID: u16 = rgb565(0, 46, 24);
const AXIS: u16 = rgb565(0, 70, 36);
const GLOW_C: u16 = rgb565(60, 255, 130);
const CORE_C: u16 = rgb565(215, 255, 225);
/// Scroll speed, px/s.
const SPEED: i64 = 140;

pub struct Sine {
    sin: Vec<i16>,
    /// Final color by distance from the trace (1/8 px), over each of the
    /// backgrounds: black, grid line, axis.
    shades: [Vec<u16>; 3],
    /// Per column: trace height (Q8 px from the top) and 1/|normal| scale
    /// (Q8) that turns vertical distance into distance from the line.
    ys: Vec<i32>,
    norm: Vec<i32>,
    /// Previous frame's trace, for the dirty strips.
    prev: Vec<i32>,
    prev_norm: Vec<i32>,
    /// Scroll position in 1/2^16 turns, and when it was last advanced.
    phase: i32,
    last_t: u32,
    start_ms: Option<u32>,
    first: bool,
}

/// Slow drift: a value swinging around `mid` by `amp` with this period.
fn drift(sin: &[i16], t_ms: u32, period_ms: u32, mid: i32, amp: i32) -> i32 {
    let i = (t_ms % period_ms) as usize * TABLE / period_ms as usize;
    mid + (amp * sin[i] as i32 >> 14)
}

fn isqrt(n: u32) -> u32 {
    let mut x = (n as f32).sqrt() as u32;
    while x * x > n {
        x -= 1;
    }
    while (x + 1) * (x + 1) <= n {
        x += 1;
    }
    x
}

impl Sine {
    pub fn new() -> Self {
        let sin = (0..TABLE).map(|i| ((i as f32 * std::f32::consts::TAU / TABLE as f32).sin() * 16384.0).round() as i16).collect();
        let n = (GLOW * 8) as usize + 1;
        let shades = [0, GRID, AXIS].map(|bg| {
            (0..n)
                .map(|i| {
                    let d = i as f32 / 8.0;
                    let glow = (190.0 * (-d / 3.2).exp()) as u8;
                    let core = (255.0 * (1.6 - d).clamp(0.0, 1.0)) as u8;
                    blend565_8(blend565_8(bg, GLOW_C, glow), CORE_C, core)
                })
                .collect()
        });
        Self { sin, shades, ys: vec![CY * 256; W], norm: vec![256; W], prev: vec![CY * 256; W], prev_norm: vec![256; W], phase: 0, last_t: 0, start_ms: None, first: true }
    }

    /// Recompute the trace for `t` ms in.
    fn trace(&mut self, t: u32) {
        let amp = drift(&self.sin, t, 11_000, 110, 45); // px
        let wavelength = drift(&self.sin, t, 17_000, 220, 70); // px
        let harmonic = drift(&self.sin, t, 23_000, 0, 10); // percent of amp
        // Scroll at SPEED px/s whatever the wavelength.
        let dt = t.wrapping_sub(self.last_t).min(100) as i64;
        self.last_t = t;
        self.phase = self.phase.wrapping_add((dt * SPEED * 65536 / 1000 / wavelength as i64) as i32);
        let phase = self.phase;
        let step = 65536 / wavelength; // per column
        let mask = TABLE as i32 - 1;
        let at = |turns: i32| self.sin[((turns >> 6) & mask) as usize] as i32;
        for x in 0..W {
            let th = (x as i32 * step).wrapping_sub(phase);
            let (s1, c1) = (at(th), at(th.wrapping_add(16384)));
            let (s3, c3) = (at(th.wrapping_mul(3)), at(th.wrapping_mul(3).wrapping_add(16384)));
            // y in Q8; the screen's y grows downward.
            let y = (amp * (s1 + harmonic * s3 / 100)) >> 6;
            self.ys[x] = CY * 256 - y;
            // Slope dy/dx (Q8) = amp·2π/λ·(cos θ + 3h·cos 3θ).
            let slope = (amp as i64 * 1608 * (c1 + 3 * harmonic * c3 / 100) as i64 / wavelength as i64 >> 14) as i32;
            let slope = slope.clamp(-60 * 256, 60 * 256);
            let len = isqrt((65536 + slope * slope) as u32);
            self.norm[x] = (65536 / len.max(256)) as i32;
        }
    }

    /// Strips whose trace moved, spanning old and new.
    fn dirty(&self) -> Vec<Rect> {
        (0..W as i32)
            .step_by(STRIP as usize)
            .map(|x0| {
                // A steep trace's glow reaches further up and down.
                let reach = |y: i32, norm: i32| {
                    let r = GLOW * 65536 / norm.max(1);
                    ((y - r) >> 8, (y + r) >> 8)
                };
                let (lo, hi) = (x0 as usize..(x0 + STRIP) as usize)
                    .flat_map(|x| [reach(self.ys[x], self.norm[x]), reach(self.prev[x], self.prev_norm[x])])
                    .fold((i32::MAX, i32::MIN), |(lo, hi), (a, b)| (lo.min(a), hi.max(b)));
                Rect::new(x0, lo - 2, x0 + STRIP - 1, hi + 2)
            })
            .collect()
    }

    /// Distance of pixel (x, y) from the trace, 1/8 px.
    fn dist(&self, x: i32, y: i32) -> usize {
        ((y * 256 + 128 - self.ys[x as usize]).abs() as i64 * self.norm[x as usize] as i64 >> 13) as usize
    }
}

impl Default for Sine {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Sine {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let t = now_ms.wrapping_sub(*self.start_ms.get_or_insert(now_ms));
        std::mem::swap(&mut self.prev, &mut self.ys);
        std::mem::swap(&mut self.prev_norm, &mut self.norm);
        self.trace(t);
        if std::mem::replace(&mut self.first, false) {
            return Step::Redraw(vec![SCREEN]);
        }
        Step::Redraw(self.dirty())
    }

    fn invalidate(&mut self) {
        self.first = true;
    }

    fn frame_ms(&self) -> u32 {
        16
    }
}

impl Scene for Sine {
    fn draw(&self, c: &mut Canvas) {
        let a = c.area();
        c.fill(a, 0);
        // Graticule: division lines, and ticks along the center axes; only
        // those crossing this canvas.
        let from = |lo: i32, pitch: i32| (lo + pitch - 1).max(pitch) / pitch * pitch;
        let (cx, last) = (LCD_W as i32 / 2, LCD_W as i32 - 1);
        for x in (from(a.x0, DIV)..=a.x1.min(last - 1)).step_by(DIV as usize) {
            c.fill(Rect::new(x, a.y0, x, a.y1), if x == cx { AXIS } else { GRID });
        }
        for y in (from(a.y0, DIV)..=a.y1.min(last - 1)).step_by(DIV as usize) {
            c.fill(Rect::new(a.x0, y, a.x1, y), if y == CY { AXIS } else { GRID });
        }
        if a.y0 <= CY + 3 && a.y1 >= CY - 3 {
            for k in (from(a.x0, TICK)..=a.x1).step_by(TICK as usize) {
                c.fill(Rect::new(k, CY - 3, k, CY + 3), AXIS);
            }
        }
        if a.x0 <= cx + 3 && a.x1 >= cx - 3 {
            for k in (from(a.y0, TICK)..=a.y1).step_by(TICK as usize) {
                c.fill(Rect::new(cx - 3, k, cx + 3, k), AXIS);
            }
        }
        // The trace: glow and bright core, colored by what's beneath.
        let n = self.shades[0].len();
        c.map(a, |x, y, bg| {
            let d = self.dist(x, y);
            if d >= n {
                return bg;
            }
            let k = match bg {
                0 => 0,
                GRID => 1,
                _ => 2,
            };
            self.shades[k][d]
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::check_incremental;
    use super::*;

    #[test]
    fn incremental_redraws_match() {
        let t = LocalTime::default();
        check_incremental(Box::new(Sine::new()), (0..300u32).map(|i| (i * 37, t)));
    }

    #[test]
    fn trace_stays_on_screen() {
        let mut s = Sine::new();
        for t in (0..120_000).step_by(250) {
            s.trace(t);
            assert!(s.ys.iter().all(|&y| (0..LCD_H as i32 * 256).contains(&y)), "t={t}");
        }
    }
}
