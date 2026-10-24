//! Analog faces.
//!
//! - `railway` (white dial) and `railway-black`: bold bars, plain hands and
//!   a red lollipop seconds hand that sweeps round in 58.5 s, rests at 12,
//!   and sets off again as the minute hand jumps to the next minute — the
//!   way a station clock waits for the master clock's minute pulse.
//! - `classic`: charcoal dial with serif numerals, tapered hands that move
//!   every second, and a date window at 3 o'clock.
//!
//! The dials are pre-rendered (`build_support/analog.rs`); hands are drawn
//! per pixel from a distance function in fixed point (the C6 has no FPU),
//! and only the rows of screen a hand left or entered are redrawn.

use super::{Face, LocalTime};
use crate::assets::Blob;
use crate::color::rgb565;
use crate::fonts::DIAL_DATE;
use crate::gfx::{Canvas, Rect, Scene, SCREEN};
use crate::sprites::analog::{CLASSIC, DATE_WINDOW, RAILWAY_BLACK, RAILWAY_WHITE};

/// Dial center; the 480 px screen's middle falls between pixels 239 and 240.
const C: i32 = 240;
/// Hand angles are in 1/4096 turns, clockwise from 12.
const TURN: i32 = 4096;
/// Railway seconds hand: one turn in this many ms, then it waits at 12.
const SWEEP_MS: i32 = 58_500;

#[derive(Clone, Copy)]
struct Hand {
    /// Tail and tip: px from the center along the hand (tail negative).
    back: f32,
    tip: f32,
    /// Full width at the tail and at the tip.
    w_back: f32,
    w_tip: f32,
    /// A disc on the hand: (distance from the center, radius).
    disc: Option<(f32, f32)>,
    color: u16,
}

struct Shadow {
    dx: i32,
    dy: i32,
    /// Width of the soft edge, px.
    soft: f32,
    alpha: u8,
}

#[derive(PartialEq)]
enum Motion {
    /// Sweep and rest; the minute hand jumps.
    Railway,
    /// One step a second; minute and hour hands creep.
    Quartz,
}

pub struct Style {
    dial: &'static Blob,
    /// Hour, minute, second.
    hands: [Hand; 3],
    /// Center cap: radius, color.
    hub: (f32, u16),
    shadow: Option<Shadow>,
    motion: Motion,
    date: bool,
}

const RED: u16 = rgb565(214, 30, 30);

const fn railway_hands(color: u16) -> [Hand; 3] {
    [
        Hand { back: -48.0, tip: 142.0, w_back: 21.0, w_tip: 17.0, disc: None, color },
        Hand { back: -48.0, tip: 212.0, w_back: 16.0, w_tip: 12.0, disc: None, color },
        Hand { back: -66.0, tip: 145.0, w_back: 5.5, w_tip: 5.5, disc: Some((145.0, 19.5)), color: RED },
    ]
}

pub static RAILWAY: Style = Style {
    dial: &RAILWAY_WHITE,
    hands: railway_hands(rgb565(24, 24, 24)),
    hub: (6.5, RED),
    shadow: Some(Shadow { dx: 3, dy: 5, soft: 6.0, alpha: 70 }),
    motion: Motion::Railway,
    date: false,
};

pub static RAILWAY_DARK: Style = Style {
    dial: &RAILWAY_BLACK,
    hands: railway_hands(rgb565(236, 236, 232)),
    hub: (6.5, RED),
    shadow: None,
    motion: Motion::Railway,
    date: false,
};

const CREAM: u16 = rgb565(232, 222, 198);

pub static CLASSIC_STYLE: Style = Style {
    dial: &CLASSIC,
    hands: [
        Hand { back: -22.0, tip: 128.0, w_back: 18.0, w_tip: 2.5, disc: None, color: CREAM },
        Hand { back: -22.0, tip: 200.0, w_back: 13.0, w_tip: 2.0, disc: None, color: CREAM },
        Hand { back: -50.0, tip: 214.0, w_back: 2.4, w_tip: 1.4, disc: Some((-38.0, 7.0)), color: rgb565(200, 44, 40) },
    ],
    hub: (8.0, rgb565(196, 160, 92)),
    shadow: Some(Shadow { dx: 2, dy: 4, soft: 5.0, alpha: 140 }),
    motion: Motion::Quartz,
    date: true,
};

/// A hand at one angle, in fixed point for the per-pixel loop: lengths in
/// Q8 (1/256 px), sin/cos in Q14.
#[derive(Clone, Copy)]
struct Placed {
    sin: i32,
    cos: i32,
    back: i32,
    tip: i32,
    /// Half-width at `back`, and its change per unit along the hand (Q16).
    hw: i32,
    slope: i32,
    /// Center along the hand, radius, and 2^24 / (2 × radius).
    disc: Option<(i32, i32, i32)>,
}

impl Placed {
    fn new(h: &Hand, angle: i32) -> Self {
        let (s, c) = sin_cos(angle);
        let q8 = |v: f32| (v * 256.0).round() as i32;
        Self {
            sin: (s * 16384.0).round() as i32,
            cos: (c * 16384.0).round() as i32,
            back: q8(h.back),
            tip: q8(h.tip),
            hw: q8(h.w_back / 2.0),
            slope: ((h.w_tip - h.w_back) / 2.0 / (h.tip - h.back) * 65536.0).round() as i32,
            disc: h.disc.map(|(at, r)| (q8(at), q8(r), (1 << 24) / (2 * q8(r)))),
        }
    }

    /// Signed distance (Q8) from the shape to a point given relative to the
    /// dial center in Q8; negative inside.
    fn dist(&self, px: i32, py: i32) -> i32 {
        let u = (px * self.sin - py * self.cos) >> 14;
        let v = (px * self.cos + py * self.sin) >> 14;
        let hw = self.hw + ((u - self.back) as i64 * self.slope as i64 >> 16) as i32;
        let bar = (v.abs() - hw).max(self.back - u).max(u - self.tip);
        let Some((at, r, inv)) = self.disc else { return bar };
        let du = u - at;
        if du.abs() > r + 512 || v.abs() > r + 512 {
            return bar;
        }
        // Near the rim, (d² - r²) / 2r ≈ d - r; no square root needed.
        let disc = ((du * du + v * v - r * r) as i64 * inv as i64 >> 24) as i32;
        bar.min(disc)
    }

    /// Columns (pixel offsets from the dial center) that can come within
    /// `limit` (Q8) of the shape on the row `py` pixels from the center.
    /// Generous by a pixel or two; `dist` decides each pixel in it.
    fn span(&self, py: i32, limit: i32) -> Option<(i32, i32)> {
        let px = |q8: i32| q8.div_euclid(256);
        let tip_hw = self.hw + ((self.tip - self.back) as i64 * self.slope as i64 >> 16) as i32;
        let w = px(self.hw.max(tip_hw) + limit) + 2;
        let along = solve(self.sin, -py * self.cos, px(self.back - limit) - 2, px(self.tip + limit) + 2);
        let across = solve(self.cos, py * self.sin, -w, w);
        let bar = along.zip(across).map(|(a, b)| (a.0.max(b.0), a.1.min(b.1))).filter(|(x0, x1)| x0 <= x1);
        let disc = self.disc.and_then(|(at, r, _)| {
            let (cx, cy) = (px(at * self.sin >> 14), px(-(at * self.cos) >> 14));
            let r = px(r + limit) + 2;
            ((py - cy).abs() <= r).then_some((cx - r, cx + r))
        });
        match (bar, disc) {
            (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
            (a, b) => a.or(b),
        }
    }
}

/// The x for which `lo <= (x * a + b) / 2^14 <= hi`, give or take one.
fn solve(a: i32, b: i32, lo: i32, hi: i32) -> Option<(i32, i32)> {
    const S: i32 = 1 << 14;
    if a == 0 {
        return ((lo - 1) * S <= b && b <= (hi + 1) * S).then_some((i32::MIN / 2, i32::MAX / 2));
    }
    let (a, b, lo, hi) = if a < 0 { (-a, -b, -hi, -lo) } else { (a, b, lo, hi) };
    Some(((lo * S - b).div_euclid(a), (hi * S - b).div_euclid(a) + 1))
}

/// Paint one shape (or its shadow) over the part of `bounds` on this canvas,
/// row by row over just the columns it can reach. `origin` is where the
/// dial center falls on screen; `coverage` maps a distance to alpha and is
/// zero from `limit` out.
fn shade_shape(c: &mut Canvas, shape: &Placed, bounds: Rect, origin: (i32, i32), limit: i32, color: u16, coverage: impl Fn(i32) -> u8) {
    let Some(vis) = c.area().intersect(&bounds) else { return };
    // A local copy keeps the shape in registers across the pixel loop.
    let (shape, (ox, oy)) = (*shape, origin);
    for y in vis.y0..=vis.y1 {
        let Some((x0, x1)) = shape.span(y - oy, limit) else { continue };
        let (x0, x1) = ((ox + x0).max(vis.x0), (ox + x1).min(vis.x1));
        if x0 <= x1 {
            c.shade(Rect::new(x0, y, x1, y), color, |x, y| coverage(shape.dist((x - ox) * 256 + 128, (y - oy) * 256 + 128)));
        }
    }
}

fn sin_cos(angle: i32) -> (f32, f32) {
    (angle as f32 / TURN as f32 * std::f32::consts::TAU).sin_cos()
}

fn bbox(points: impl IntoIterator<Item = (f32, f32)>, margin: f32) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in points {
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
    }
    let c = C as f32;
    Rect::new(
        (c + x0 - margin).floor() as i32,
        (c + y0 - margin).floor() as i32,
        (c + x1 + margin).ceil() as i32,
        (c + y1 + margin).ceil() as i32,
    )
}

/// The screen a hand can touch at `angle`, shadow included.
fn bounds(h: &Hand, angle: i32, shadow: Option<&Shadow>) -> Rect {
    let (s, c) = sin_cos(angle);
    // Hand space (along, across) to screen offsets.
    let rotate = |u: f32, v: f32| (u * s + v * c, -u * c + v * s);
    let w = h.w_back.max(h.w_tip) / 2.0;
    let mut r = bbox([(h.back, w), (h.back, -w), (h.tip, w), (h.tip, -w)].map(|(u, v)| rotate(u, v)), 2.0);
    if let Some((at, rad)) = h.disc {
        let (x, y) = rotate(at, 0.0);
        r = r.union(&bbox([(x - rad, y - rad), (x + rad, y + rad)], 2.0));
    }
    if let Some(s) = shadow {
        let m = s.soft.ceil() as i32;
        let shifted = r.translate(s.dx, s.dy);
        r = r.union(&Rect::new(shifted.x0 - m, shifted.y0 - m, shifted.x1 + m, shifted.y1 + m));
    }
    r
}

/// Rows per band of a changed region.
const BAND: i32 = 16;

/// The screen that can differ between hand placements `a` and `b`, as
/// bands of BAND rows each only as wide as either reaches in it (shadow
/// included). The bands don't overlap, so nothing is drawn twice.
fn changed(a: &Placement, b: &Placement, shadow: Option<&Shadow>) -> Vec<Rect> {
    let shadow = shadow.map(|s| (s.dx, s.dy, (s.soft * 256.0) as i32 / 2));
    let all = a.bounds.union(&b.bounds);
    let (top, bottom) = (all.y0.max(0) & !1, all.y1.min(SCREEN.y1));
    let mut out = Vec::new();
    for y0 in (top..=bottom).step_by(BAND as usize) {
        let y1 = (y0 + BAND - 1).min(bottom);
        let (mut x0, mut x1) = (i32::MAX, i32::MIN);
        for y in y0..=y1 {
            for p in [&a.shape, &b.shape] {
                let mut reach = |span: Option<(i32, i32)>, dx: i32| {
                    if let Some((l, r)) = span {
                        (x0, x1) = (x0.min(C + dx + l), x1.max(C + dx + r));
                    }
                };
                reach(p.span(y - C, 128), 0);
                if let Some((dx, dy, limit)) = shadow {
                    reach(p.span(y - C - dy, limit), dx);
                }
            }
        }
        if x0 <= x1 {
            out.push(Rect::new(x0, y0, x1, y1));
        }
    }
    out
}

/// A hand as last placed: computed once per move, not per band.
struct Placement {
    angle: i32,
    shape: Placed,
    bounds: Rect,
}

impl Placement {
    fn new(h: &Hand, angle: i32, shadow: Option<&Shadow>) -> Self {
        Self { angle, shape: Placed::new(h, angle), bounds: bounds(h, angle, shadow) }
    }
}

pub struct AnalogFace {
    style: &'static Style,
    t: LocalTime,
    /// Hour, minute, second angles to draw.
    angles: [i32; 3],
    /// Hour, minute, second as drawn; None until the first update.
    hands: Option<[Placement; 3]>,
    shown: Option<u8>,
    /// When (now_ms) the current second began, for the sweep.
    second: Option<(u8, u32)>,
    off: (i32, i32),
}

impl AnalogFace {
    pub fn new(style: &'static Style) -> Self {
        Self { style, t: LocalTime::default(), angles: [0; 3], hands: None, shown: None, second: None, off: (0, 0) }
    }

    fn compute_angles(&mut self, now_ms: u32, t: &LocalTime) -> [i32; 3] {
        let (h, m, s) = ((t.hour % 12) as i32, t.minute as i32, t.second as i32);
        match self.style.motion {
            Motion::Railway => {
                // Track where in the second we are; until the first
                // boundary is seen, the hand lags by at most a second.
                let start = match self.second {
                    Some((sec, start)) if sec == t.second => start,
                    _ => {
                        self.second = Some((t.second, now_ms));
                        now_ms
                    }
                };
                let ms = s * 1000 + now_ms.wrapping_sub(start).min(999) as i32;
                [(h * 60 + m) * TURN / 720, m * TURN / 60, ms.min(SWEEP_MS) * TURN / SWEEP_MS % TURN]
            }
            Motion::Quartz => {
                let secs = h * 3600 + m * 60 + s;
                [secs * TURN / 43_200, (m * 60 + s) * TURN / 3600, s * TURN / 60]
            }
        }
    }

    fn date_rect(&self) -> Rect {
        let (x, y, w, h) = DATE_WINDOW;
        Rect::from_size(x - w / 2, y - h / 2, w, h)
    }
}

impl Face for AnalogFace {
    fn update(&mut self, now_ms: u32, t: &LocalTime) -> Vec<Rect> {
        self.t = *t;
        self.angles = self.compute_angles(now_ms, t);
        let style = self.style;
        let mut dirty = Vec::new();
        let place = |i: usize| Placement::new(&style.hands[i], self.angles[i], style.shadow.as_ref());
        match &mut self.hands {
            None => self.hands = Some([place(0), place(1), place(2)]),
            Some(hands) => {
                for (i, old) in hands.iter_mut().enumerate() {
                    if old.angle != self.angles[i] {
                        let new = place(i);
                        dirty.extend(changed(old, &new, style.shadow.as_ref()));
                        *old = new;
                    }
                }
            }
        }
        let Some(day) = self.shown.replace(t.day) else {
            return vec![SCREEN];
        };
        if self.style.date && day != t.day {
            dirty.push(self.date_rect());
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

    fn frame_ms(&self) -> u32 {
        // Railway: sweep smoothly; rest (and wait for the minute) at 12.
        let sweeping = self.style.motion == Motion::Railway && (self.t.second as i32) * 1000 < SWEEP_MS;
        if sweeping {
            16
        } else {
            50
        }
    }
}

impl Scene for AnalogFace {
    fn draw(&self, c: &mut Canvas) {
        let (dx, dy) = self.off;
        let style = self.style;
        c.blit_rle(dx, dy, style.dial.bytes());

        if style.date {
            let r = self.date_rect().translate(dx, dy);
            if c.area().intersect(&r).is_some() {
                let (top, bottom) = DIAL_DATE.ink_rows("0123456789");
                let baseline = (r.y0 + r.y1 + 1) / 2 - (top + bottom) / 2;
                c.text_centered(&DIAL_DATE, (r.x0 + r.x1 + 1) / 2, baseline, &self.t.day.to_string(), rgb565(30, 28, 26));
            }
        }

        let Some(hands) = &self.hands else { return };
        let (cx, cy) = (C + dx, C + dy);
        if let Some(s) = &style.shadow {
            // Alpha falls from full at -soft/2 to none at +soft/2; `scale`
            // spares a division per pixel.
            let soft = (s.soft * 256.0) as i32;
            let (alpha, scale) = (s.alpha as i32, s.alpha as i32 * 65536 / soft);
            for h in hands {
                let bounds = h.bounds.translate(dx, dy);
                shade_shape(c, &h.shape, bounds, (cx + s.dx, cy + s.dy), soft / 2, 0, move |d| {
                    ((soft / 2 - d) * scale >> 16).clamp(0, alpha) as u8
                });
            }
        }
        for (h, hand) in hands.iter().zip(&style.hands) {
            shade_shape(c, &h.shape, h.bounds.translate(dx, dy), (cx, cy), 128, hand.color, |d| (128 - d).clamp(0, 255) as u8);
        }
        let (radius, color) = style.hub;
        let r = (radius * 256.0) as i32;
        let m = radius.ceil() as i32 + 1;
        c.shade(Rect::new(cx - m, cy - m, cx + m, cy + m), color, |x, y| {
            let (px, py) = ((x - cx) * 256 + 128, (y - cy) * 256 + 128);
            ((r * r - px * px - py * py) / (2 * r) + 128).clamp(0, 255) as u8
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::tests::check_frames;

    fn at(hour: u8, minute: u8, second: u8) -> LocalTime {
        LocalTime { year: 2026, month: 10, day: 23, weekday: 5, hour, minute, second }
    }

    /// (ms, time) every `step` ms from `t0`, seconds advancing with ms.
    fn run(t0: LocalTime, from_ms: u32, to_ms: u32, step: u32) -> Vec<(u32, LocalTime)> {
        (from_ms..=to_ms)
            .step_by(step as usize)
            .map(|ms| {
                let secs = t0.hour as u32 * 3600 + t0.minute as u32 * 60 + t0.second as u32 + ms / 1000;
                let t = LocalTime { hour: (secs / 3600 % 24) as u8, minute: (secs / 60 % 60) as u8, second: (secs % 60) as u8, ..t0 };
                (ms, t)
            })
            .collect()
    }

    #[test]
    fn dirty_rects_cover_every_change() {
        for style in [&RAILWAY, &RAILWAY_DARK, &CLASSIC_STYLE] {
            // A sweep, the rest at 12, the minute jump and the restart.
            check_frames(|| Box::new(AnalogFace::new(style)), &run(at(10, 9, 57), 0, 3600, 120));
        }
        // Midnight: the classic date changes.
        let day = |d| LocalTime { day: d, ..at(23, 59, 59) };
        let steps = [(0, day(22)), (1000, LocalTime { day: 23, ..at(0, 0, 0) })];
        check_frames(|| Box::new(AnalogFace::new(&CLASSIC_STYLE)), &steps);
    }

    #[test]
    fn railway_seconds_hand_rests_then_minute_jumps() {
        let mut f = AnalogFace::new(&RAILWAY);
        f.update(0, &at(8, 15, 58));
        assert!(f.angles[2] > TURN * 99 / 100, "nearly round at 58 s");
        f.update(600, &at(8, 15, 58));
        let resting = f.angles;
        assert_eq!(resting[2], 0, "at 12 from 58.5 s");
        f.update(1000, &at(8, 15, 59));
        assert_eq!(f.angles, resting, "still waiting");
        f.update(2000, &at(8, 16, 0));
        assert_eq!(f.angles[1], 16 * TURN / 60, "minute jumps");
        f.update(2500, &at(8, 16, 0));
        assert!(f.angles[2] > 0 && f.angles[2] < TURN / 60, "sweeps off again");
    }

    #[test]
    fn hand_distance_matches_geometry() {
        // A vertical hand at 12: inside on the axis, outside past the tip.
        let p = Placed::new(&RAILWAY.hands[1], 0);
        assert!(p.dist(0, -100 * 256) < -1000);
        assert!(p.dist(0, -230 * 256) > 0);
        assert!(p.dist(20 * 256, -100 * 256) > 0);
        // At 3 o'clock the same hand lies along +x.
        let p = Placed::new(&RAILWAY.hands[1], TURN / 4);
        assert!(p.dist(100 * 256, 0) < -1000);
        // The disc on the seconds hand.
        let p = Placed::new(&RAILWAY.hands[2], 0);
        assert!(p.dist(15 * 256, -145 * 256) < 0);
    }

    #[test]
    fn spans_hold_every_covered_pixel() {
        // Shadow-wide and edge limits, every hand of every style, angles
        // off the axes and on them.
        for style in [&RAILWAY, &CLASSIC_STYLE] {
            for hand in &style.hands {
                for angle in (0..TURN).step_by(37).chain([0, TURN / 4, TURN / 2, 3 * TURN / 4]) {
                    let p = Placed::new(hand, angle);
                    for limit in [128, 640, 768] {
                        for py in -C..C {
                            let span = p.span(py, limit);
                            for px in -C..C {
                                if p.dist(px * 256 + 128, py * 256 + 128) < limit {
                                    let (x0, x1) = span.unwrap_or_else(|| panic!("angle {angle}: row {py} has no span"));
                                    assert!((x0..=x1).contains(&px), "angle {angle}: ({px}, {py}) outside {x0}..={x1}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
