//! `starfield`: flying through space. Stars come out of the distance,
//! brightening and growing as they near, and stream past the edges; every
//! so often the ship jumps to warp and they stretch into long streaks.
//!
//! Each star is a point in 3-D projected onto the screen as a short
//! segment, from where it was a moment ago (dim) to where it is now
//! (bright). Only stars whose segment changed are redrawn.

use super::super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::rgb565;
use crate::gfx::{Canvas, Rect, Scene, SCREEN};
use crate::rng::Rng;

const STARS: usize = 200;
/// Space: x and y within ±SPREAD, depth Z_NEAR..Z_FAR (units), seen with
/// this focal length (px per unit at depth 1).
const SPREAD: i32 = 2048;
const Z_FAR: i32 = 4096;
const Z_NEAR: i32 = 48;
const FOCAL: i32 = 256;
/// Stars this close to the line of flight would sit in the middle of the
/// screen all the way in; none are put there.
const CLEAR: i32 = 200;
/// Cruising speed, units/s, and how many times faster at warp.
const CRUISE: i32 = 650;
const WARP: i32 = 7;
/// Timeline, ms: cruise, speed up, warp, slow down.
const CRUISE_MS: u32 = 14_000;
const RAMP_MS: u32 = 2_000;
const WARP_MS: u32 = 4_000;
const CYCLE: u32 = CRUISE_MS + RAMP_MS + WARP_MS + RAMP_MS;
/// A star's segment reaches back to where it was this long ago.
const TRAIL_MS: i32 = 45;
const LEVELS: usize = 16;
/// Star colors: mostly white, some blue, a few yellow and orange.
const TINTS: [[f32; 3]; 4] = [[235.0, 240.0, 255.0], [170.0, 200.0, 255.0], [255.0, 235.0, 170.0], [255.0, 190.0, 140.0]];
const TINT_ODDS: [u8; 10] = [0, 0, 0, 0, 0, 0, 1, 1, 2, 3];

const CX: i32 = LCD_W as i32 / 2;
const CY: i32 = LCD_H as i32 / 2;

struct Star {
    x: i32,
    y: i32,
    /// Depth in 1/16 units.
    z16: i32,
    tint: u8,
}

/// A star as drawn: from (x0, y0) at level b0 to (x1, y1) at b1, thickening
/// from 1 px to `size`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Seg {
    x0: i16,
    y0: i16,
    x1: i16,
    y1: i16,
    b0: u8,
    b1: u8,
    size: u8,
    tint: u8,
}

impl Seg {
    fn bounds(&self) -> Rect {
        let p = self.size as i32;
        let (x0, x1) = (self.x0.min(self.x1) as i32, self.x0.max(self.x1) as i32);
        let (y0, y1) = (self.y0.min(self.y1) as i32, self.y0.max(self.y1) as i32);
        Rect::new(x0 - p, y0 - p, x1 + p, y1 + p)
    }
}

pub struct Starfield {
    rng: Rng,
    /// Per tint, RGB565 by brightness level.
    colors: [[u16; LEVELS]; 4],
    stars: Vec<Star>,
    drawn: Vec<Option<Seg>>,
    start_ms: Option<u32>,
    last_ms: Option<u32>,
    first: bool,
}

fn smoothstep(num: u32, den: u32) -> i32 {
    let p = (num as i64 * 1024 / den as i64).clamp(0, 1024);
    (p * p * (3 * 1024 - 2 * p) / (1024 * 1024)) as i32 // 0..1024
}

/// Speed, units/s, `t` ms in.
fn speed(t: u32) -> i32 {
    let t = t % CYCLE;
    let up = CRUISE_MS;
    let warp = up + RAMP_MS;
    let down = warp + WARP_MS;
    let k = if t < up {
        0
    } else if t < warp {
        smoothstep(t - up, RAMP_MS)
    } else if t < down {
        1024
    } else {
        1024 - smoothstep(t - down, RAMP_MS)
    };
    CRUISE + CRUISE * (WARP - 1) * k / 1024
}

/// Brightness level of a star at depth `z`: fades in from the distance.
fn level(z: i32) -> u8 {
    ((Z_FAR - z) * LEVELS as i32 / (Z_FAR * 11 / 20)).clamp(0, LEVELS as i32 - 1) as u8
}

fn on_screen(x: i32, y: i32) -> bool {
    SCREEN.x0 <= x && x <= SCREEN.x1 && SCREEN.y0 <= y && y <= SCREEN.y1
}

fn project(x: i32, y: i32, z: i32) -> (i32, i32) {
    (CX + x * FOCAL / z, CY + y * FOCAL / z)
}

/// How far (1/1024) from `from` towards `to` the segment stays on screen;
/// `from` must be on it.
fn clip(from: (i32, i32), to: (i32, i32)) -> i32 {
    let mut t = 1024i64;
    for (a, b, lo, hi) in [(from.0, to.0, SCREEN.x0, SCREEN.x1), (from.1, to.1, SCREEN.y0, SCREEN.y1)] {
        let edge = if b > hi {
            hi
        } else if b < lo {
            lo
        } else {
            continue;
        };
        t = t.min((edge - a) as i64 * 1024 / (b - a) as i64);
    }
    t as i32
}

impl Starfield {
    pub fn new(mut rng: Rng) -> Self {
        let colors = TINTS.map(|c| {
            core::array::from_fn(|l| {
                let k = (l as f32 / (LEVELS - 1) as f32).powf(1.3);
                rgb565((c[0] * k) as u8, (c[1] * k) as u8, (c[2] * k) as u8)
            })
        });
        let stars = (0..STARS)
            .map(|_| {
                let mut s = Self::spawn(&mut rng);
                s.z16 = rng.range(Z_NEAR * 4, Z_FAR) * 16;
                s
            })
            .collect();
        Self { rng, colors, stars, drawn: vec![None; STARS], start_ms: None, last_ms: None, first: true }
    }

    /// A new star in the far distance.
    fn spawn(rng: &mut Rng) -> Star {
        let (x, y) = loop {
            let (x, y) = (rng.range(-SPREAD, SPREAD), rng.range(-SPREAD, SPREAD));
            if x.abs() >= CLEAR || y.abs() >= CLEAR {
                break (x, y);
            }
        };
        Star { x, y, z16: Z_FAR * 16, tint: TINT_ODDS[rng.below(TINT_ODDS.len() as u32) as usize] }
    }

    /// The star's segment, or None once it has left the screen.
    fn segment(s: &Star, trail: i32) -> Option<Seg> {
        let z = s.z16 / 16;
        if z < Z_NEAR {
            return None;
        }
        let tail_z = z + trail;
        let tail = project(s.x, s.y, tail_z);
        if !on_screen(tail.0, tail.1) {
            return None;
        }
        let mut head = project(s.x, s.y, z);
        let (b0, mut b1) = (level(tail_z) as i32 * 2 / 5, level(z) as i32);
        if !on_screen(head.0, head.1) {
            let t = clip(tail, head);
            head = (tail.0 + (head.0 - tail.0) * t / 1024, tail.1 + (head.1 - tail.1) * t / 1024);
            b1 = b0 + (b1 - b0) * t / 1024;
        }
        let size = if z > 1400 {
            1
        } else if z > 600 {
            2
        } else {
            3
        };
        Some(Seg { x0: tail.0 as i16, y0: tail.1 as i16, x1: head.0 as i16, y1: head.1 as i16, b0: b0 as u8, b1: b1 as u8, size, tint: s.tint })
    }

    fn draw_seg(&self, c: &mut Canvas, s: &Seg) {
        let colors = &self.colors[s.tint as usize];
        let (dx, dy) = ((s.x1 - s.x0) as i32, (s.y1 - s.y0) as i32);
        let n = dx.abs().max(dy.abs()).max(1);
        let (b0, db) = (s.b0 as i32, s.b1 as i32 - s.b0 as i32);
        // Tail to head, so the head ends up on top.
        let first = if dx == 0 && dy == 0 { n } else { 0 };
        for i in first..=n {
            let level = b0 + db * i / n;
            if level == 0 {
                continue;
            }
            let size = 1 + (s.size as i32 - 1) * i / n;
            let (x, y) = (s.x0 as i32 + dx * i / n, s.y0 as i32 + dy * i / n);
            c.fill(Rect::from_size(x - (size - 1) / 2, y - (size - 1) / 2, size, size), colors[level as usize]);
        }
    }
}

impl Widget for Starfield {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let t = now_ms.wrapping_sub(*self.start_ms.get_or_insert(now_ms));
        let dt = now_ms.wrapping_sub(self.last_ms.unwrap_or(now_ms)).min(100) as i32;
        self.last_ms = Some(now_ms);
        let v = speed(t);
        let trail = v * TRAIL_MS / 1000;

        let mut dirty = Vec::new();
        let mut area = 0;
        for i in 0..STARS {
            self.stars[i].z16 -= v * 16 * dt / 1000;
            let seg = match Self::segment(&self.stars[i], trail) {
                Some(seg) => Some(seg),
                None => {
                    self.stars[i] = Self::spawn(&mut self.rng);
                    Self::segment(&self.stars[i], trail)
                }
            };
            if seg != self.drawn[i] {
                let r = match (self.drawn[i], seg) {
                    (Some(a), Some(b)) => a.bounds().union(&b.bounds()),
                    (Some(a), None) | (None, Some(a)) => a.bounds(),
                    (None, None) => continue,
                };
                area += r.width() * r.height();
                dirty.push(r);
                self.drawn[i] = seg;
            }
        }
        // At warp the streaks cover so much that one repaint is cheaper.
        let first = std::mem::replace(&mut self.first, false);
        if first || area > SCREEN.width() * SCREEN.height() / 2 {
            return Step::Redraw(vec![SCREEN]);
        }
        Step::Redraw(dirty)
    }

    fn invalidate(&mut self) {
        self.first = true;
    }

    fn frame_ms(&self) -> u32 {
        25
    }
}

impl Scene for Starfield {
    fn draw(&self, c: &mut Canvas) {
        let a = c.area();
        c.fill(a, 0);
        for s in self.drawn.iter().flatten() {
            if s.bounds().intersect(&a).is_some() {
                self.draw_seg(c, s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::check_incremental;
    use super::*;

    #[test]
    fn cruises_then_warps() {
        assert_eq!(speed(0), CRUISE);
        assert_eq!(speed(CRUISE_MS + RAMP_MS + 100), CRUISE * WARP);
        assert_eq!(speed(CYCLE), CRUISE);
    }

    #[test]
    fn stars_brighten_as_they_near_and_leave_off_the_edge() {
        assert_eq!(level(Z_FAR), 0);
        assert!(level(Z_FAR / 2) > level(Z_FAR * 3 / 4));
        let s = Star { x: 1000, y: 0, z16: 2000 * 16, tint: 0 };
        let seg = Starfield::segment(&s, 30).unwrap();
        assert!(seg.x1 > seg.x0 && seg.b1 > seg.b0);
        // Its trail clipped where it runs off the right edge.
        let s = Star { x: 1000, y: 0, z16: 1010 * 16, tint: 0 };
        let seg = Starfield::segment(&s, 300).unwrap();
        assert!((SCREEN.x1 - 1..=SCREEN.x1).contains(&(seg.x1 as i32)));
        let s = Star { x: 1000, y: 0, z16: 500 * 16, tint: 0 };
        assert!(Starfield::segment(&s, 30).is_none());
    }

    #[test]
    fn incremental_redraws_match() {
        let t = LocalTime::default();
        // Cruising, then into warp.
        let steps = (0..200u32).map(|i| i * 25).chain((0..80).map(|i| CRUISE_MS + i * 50));
        check_incremental(Box::new(Starfield::new(Rng::new(7))), steps.map(|ms| (ms, t)));
    }
}
