//! `pong`: a Pong clock. The left player's score is the hour and the right
//! player's the minute; the players are good enough never to miss, except
//! that the left one misses as the minute changes (and the right one as
//! the hour does), timed so the point lands on the minute.

use super::super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::{display_hour, LocalTime};
use crate::color::rgb565;
use crate::gfx::{Canvas, Rect, Scene, SCREEN};
use crate::rng::Rng;

const W: f32 = LCD_W as f32;
const H: f32 = LCD_H as f32;
const FG: u16 = rgb565(235, 235, 235);
const BALL: f32 = 12.0;
const PADDLE_W: f32 = 12.0;
const PADDLE_H: f32 = 72.0;
/// Left edge of each paddle.
const PADDLE_X: [f32; 2] = [28.0, W - 28.0 - PADDLE_W];
/// Ball speeds across the court, px/ms.
const SPEED: f32 = 0.42;
const MIN_SPEED: f32 = 0.2;
const MAX_SPEED: f32 = 0.9;
/// Steepest bounce off a paddle edge, px/ms vertically.
const MAX_VY: f32 = 0.32;
const PADDLE_SPEED: f32 = 0.5;
/// Start timing rallies this long before the minute changes.
const PLAN_MS: u32 = 4400;
const SERVE_PAUSE_MS: u32 = 800;
/// Score digits: 3×5 blocks of this size, centered over each half.
const BLOCK: i32 = 14;
const SCORE_Y: i32 = 36;

const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b001, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b001, 0b001, 0b001],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b001],
];

pub struct Pong {
    format_24h: bool,
    rng: Rng,
    /// Ball top-left and velocity (px/ms); None while waiting to serve.
    ball: Option<((f32, f32), (f32, f32))>,
    serve_at: u32,
    /// Paddle tops, and where each aims to hit the ball (offset from its
    /// center, for varied angles).
    paddles: [f32; 2],
    aim: [f32; 2],
    /// The side that will let the ball through on this rally.
    doomed: Option<usize>,
    /// Hour and minute on the scoreboard; None before the first tick.
    shown: Option<(u8, u8)>,
    /// When (now_ms) the current second began, for sub-second timing.
    second: Option<(u8, u32)>,
    last_ms: u32,
    drawn: Vec<Rect>,
}

impl Pong {
    pub fn new(format_24h: bool, rng: Rng) -> Self {
        let mid = (H - PADDLE_H) / 2.0;
        Self {
            format_24h,
            rng,
            ball: None,
            serve_at: 0,
            paddles: [mid, mid],
            aim: [0.0; 2],
            doomed: None,
            shown: None,
            second: None,
            last_ms: 0,
            drawn: Vec::new(),
        }
    }

    fn score(&self, t: &LocalTime) -> (u8, u8) {
        (display_hour(t.hour, self.format_24h).0, t.minute)
    }

    fn ball_rect(&self) -> Option<Rect> {
        self.ball.map(|((x, y), _)| Rect::from_size(x.round() as i32, y.round() as i32, BALL as i32, BALL as i32))
    }

    fn paddle_rect(&self, side: usize) -> Rect {
        Rect::from_size(PADDLE_X[side] as i32, self.paddles[side].round() as i32, PADDLE_W as i32, PADDLE_H as i32)
    }

    fn pieces(&self) -> Vec<Rect> {
        let mut v = vec![self.paddle_rect(0), self.paddle_rect(1)];
        v.extend(self.ball_rect());
        v
    }

    /// Where the ball's top will be when it reaches x = `face`, bouncing
    /// off the top and bottom walls on the way.
    fn predict(&self, face: f32) -> f32 {
        let Some(((x, y), (vx, vy))) = self.ball else { return (H - BALL) / 2.0 };
        let span = H - BALL;
        let y = y + vy * ((face - x) / vx).max(0.0);
        let m = y.rem_euclid(2.0 * span);
        if m > span {
            2.0 * span - m
        } else {
            m
        }
    }

    fn serve(&mut self) {
        let dir = if self.rng.below(2) == 0 { -1.0 } else { 1.0 };
        let vy = (self.rng.range(-100, 101) as f32 / 100.0) * MAX_VY * 0.6;
        self.ball = Some((((W - BALL) / 2.0, self.rng.range(120, LCD_H as i32 - 120) as f32), (dir * SPEED, vy)));
        self.new_rally_leg(if dir < 0.0 { 0 } else { 1 });
    }

    /// The ball is now heading for `side`: pick how that player will aim.
    fn new_rally_leg(&mut self, side: usize) {
        self.aim[side] = self.rng.range(-26, 27) as f32;
    }

    /// The paddle on `side` hits the ball back. `until_ms` is how long
    /// until the next scheduled point, and `loser` who should lose it.
    fn hit(&mut self, side: usize, until_ms: Option<(u32, usize)>) {
        let Some(((x, y), (_, _))) = self.ball else { return };
        let center = self.paddles[side] + PADDLE_H / 2.0;
        let off = ((y + BALL / 2.0 - center) / (PADDLE_H / 2.0 + BALL / 2.0)).clamp(-1.0, 1.0);
        let vy = off * MAX_VY;
        let to = 1 - side;
        // Distance to the other side: to its paddle face, or right out of
        // the court if that player is going to miss.
        let face = if to == 0 { PADDLE_X[0] + PADDLE_W } else { PADDLE_X[1] - BALL };
        let exit = |side: usize| if side == 0 { -BALL } else { W };
        let mut speed = SPEED;
        if let Some((t, loser)) = until_ms.filter(|&(t, _)| t < PLAN_MS) {
            if loser == to {
                // Last leg: out past the loser exactly on time, if the
                // ball can go that slowly; otherwise rally once more.
                let need = (exit(to) - x).abs() / t.max(1) as f32;
                if need >= MIN_SPEED {
                    speed = need;
                    self.doomed = Some(to);
                }
            } else {
                // Second to last: arrive leaving the last leg its usual
                // time.
                let last = (exit(side) - face).abs() / SPEED;
                speed = (face - x).abs() / (t as f32 - last).max(1.0);
            }
        }
        let vx = speed.clamp(MIN_SPEED, MAX_SPEED) * if to == 0 { -1.0 } else { 1.0 };
        let x = if side == 0 { PADDLE_X[0] + PADDLE_W } else { PADDLE_X[1] - BALL };
        self.ball = Some(((x, y), (vx, vy)));
        self.new_rally_leg(to);
    }

    fn step(&mut self, dt: f32, now_ms: u32, until: Option<(u32, usize)>, late: Option<usize>) {
        // Paddles: track the ball coming at them, idle back to the middle
        // otherwise; the losing player heads the wrong way.
        for side in 0..2 {
            let coming = self.ball.is_some_and(|(_, (vx, _))| (vx < 0.0) == (side == 0));
            let target = if coming {
                let face = if side == 0 { PADDLE_X[0] + PADDLE_W } else { PADDLE_X[1] - BALL };
                let y = self.predict(face) + BALL / 2.0;
                if self.doomed == Some(side) || late == Some(side) {
                    if y < H / 2.0 { y + 120.0 } else { y - 120.0 }
                } else {
                    y - self.aim[side]
                }
            } else {
                H / 2.0
            } - PADDLE_H / 2.0;
            let p = &mut self.paddles[side];
            let max = PADDLE_SPEED * dt;
            *p = (*p + (target - *p).clamp(-max, max)).clamp(0.0, H - PADDLE_H);
        }

        let Some(((x, y), (vx, vy))) = self.ball else {
            if now_ms.wrapping_sub(self.serve_at) < u32::MAX / 2 {
                self.serve();
            }
            return;
        };
        let (mut nx, mut ny, mut vy) = (x + vx * dt, y + vy * dt, vy);
        if ny < 0.0 {
            (ny, vy) = (-ny, -vy);
        } else if ny > H - BALL {
            (ny, vy) = (2.0 * (H - BALL) - ny, -vy);
        }
        self.ball = Some(((nx, ny), (vx, vy)));
        // Paddle faces.
        for side in 0..2 {
            let face = if side == 0 { PADDLE_X[0] + PADDLE_W } else { PADDLE_X[1] - BALL };
            let crossed = if side == 0 { vx < 0.0 && x >= face && nx < face } else { vx > 0.0 && x <= face && nx > face };
            let p = self.paddles[side];
            if crossed && ny + BALL > p && ny < p + PADDLE_H && self.doomed != Some(side) {
                nx = face;
                self.ball = Some(((nx, ny), (vx, vy)));
                self.hit(side, until);
            }
        }
        if nx < -BALL || nx > W {
            self.ball = None;
            self.doomed = None;
            self.serve_at = now_ms.wrapping_add(SERVE_PAUSE_MS);
        }
    }
}

/// Fill the score `n` centered at `cx`, as two digits if `pad`.
fn draw_score(c: &mut Canvas, n: u8, cx: i32, pad: bool) {
    let digits: Vec<usize> = if n >= 10 || pad { vec![n as usize / 10, n as usize % 10] } else { vec![n as usize] };
    for (i, rect) in score_digits(digits.len(), cx).into_iter().enumerate() {
        for (row, bits) in DIGITS[digits[i]].iter().enumerate() {
            for col in 0..3 {
                if bits >> (2 - col) & 1 == 1 {
                    c.fill(Rect::from_size(rect.x0 + col * BLOCK, rect.y0 + row as i32 * BLOCK, BLOCK, BLOCK), FG);
                }
            }
        }
    }
}

/// Where each of `n` score digits goes, centered at `cx`.
fn score_digits(n: usize, cx: i32) -> Vec<Rect> {
    let (w, gap) = (3 * BLOCK, BLOCK);
    let total = n as i32 * w + (n as i32 - 1) * gap;
    (0..n as i32).map(|i| Rect::from_size(cx - total / 2 + i * (w + gap), SCORE_Y, w, 5 * BLOCK)).collect()
}

/// Both scores' area (any number of digits).
fn score_area(cx: i32) -> Rect {
    let r = score_digits(2, cx);
    r[0].union(&r[1])
}

const SCORE_CX: [i32; 2] = [LCD_W as i32 / 4, 3 * LCD_W as i32 / 4];

impl Widget for Pong {
    fn tick(&mut self, now_ms: u32, t: &LocalTime) -> Step {
        let start = match self.second {
            Some((s, start)) if s == t.second => start,
            _ => {
                self.second = Some((t.second, now_ms));
                now_ms
            }
        };
        let real = self.score(t);
        let Some(shown) = self.shown else {
            self.shown = Some(real);
            self.last_ms = now_ms;
            self.serve_at = now_ms.wrapping_add(SERVE_PAUSE_MS);
            self.drawn = self.pieces();
            return Step::Redraw(vec![SCREEN]);
        };
        // The next point: when it's due, and who loses it (left misses as
        // the minute changes, right as the hour does).
        let in_minute = now_ms.wrapping_sub(start).min(999);
        let until = (59 - t.second as u32) * 1000 + 1000 - in_minute;
        let loser = if t.minute == 59 { 1 } else { 0 };
        // Already behind (first second of a new minute, or a time jump).
        let late = (shown != real).then(|| if shown.0 != real.0 { 1 } else { 0 });

        let dt = now_ms.wrapping_sub(self.last_ms).min(50);
        self.last_ms = now_ms;
        // Small steps keep fast balls from skipping a paddle.
        let n = dt.div_ceil(10).max(1);
        for _ in 0..n {
            self.step(dt as f32 / n as f32, now_ms, Some((until, loser)), late);
        }

        let mut dirty = Vec::new();
        if self.ball.is_none() && shown != real {
            self.shown = Some(real);
            dirty.extend(SCORE_CX.map(score_area));
        }
        let now = self.pieces();
        for r in self.drawn.iter().chain(&now) {
            dirty.push(*r);
        }
        self.drawn = now;
        Step::Redraw(dirty)
    }

    fn invalidate(&mut self) {
        self.shown = None;
    }

    fn frame_ms(&self) -> u32 {
        16
    }
}

impl Scene for Pong {
    fn draw(&self, c: &mut Canvas) {
        c.fill(c.area(), 0);
        // The net.
        let x = LCD_W as i32 / 2 - 6;
        for y in (8..LCD_H as i32).step_by(36) {
            c.fill(Rect::from_size(x, y, 12, 20), FG);
        }
        if let Some((h, m)) = self.shown {
            draw_score(c, h, SCORE_CX[0], false);
            draw_score(c, m, SCORE_CX[1], true);
        }
        for r in self.pieces() {
            c.fill(r, FG);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::check_incremental;
    use super::*;

    fn at(hour: u8, minute: u8, second: u8) -> LocalTime {
        LocalTime { year: 2026, month: 10, day: 25, hour, minute, second, ..Default::default() }
    }

    /// 16 ms frames for `secs` from `t0`.
    fn frames(t0: (u8, u8, u8), secs: u32) -> impl Iterator<Item = (u32, LocalTime)> {
        let start = t0.0 as u32 * 3600 + t0.1 as u32 * 60 + t0.2 as u32;
        (0..secs * 1000).step_by(16).map(move |ms| {
            let s = start + ms / 1000;
            (ms, at((s / 3600 % 24) as u8, (s / 60 % 60) as u8, (s % 60) as u8))
        })
    }

    /// Simulate from `t0` for `secs`; return the score changes as (ms,
    /// shown).
    fn play(t0: (u8, u8, u8), secs: u32, seed: u32) -> Vec<(u32, (u8, u8))> {
        let mut p = Pong::new(true, Rng::new(seed));
        let mut changes = Vec::new();
        let mut last = None;
        for (ms, t) in frames(t0, secs) {
            p.tick(ms, &t);
            if p.shown != last {
                changes.push((ms, p.shown.unwrap()));
                last = p.shown;
            }
        }
        changes
    }

    #[test]
    fn points_land_on_the_minute() {
        for seed in 0..40 {
            // Starts at 10:58:50: the minute changes at 10 s, the hour at 70 s.
            let changes = play((10, 58, 50), 75, seed);
            assert_eq!(changes[0], (0, (10, 58)));
            let (ms, s) = changes[1];
            assert_eq!(s, (10, 59), "seed {seed}");
            assert!((10_000..10_600).contains(&ms), "seed {seed}: minute point at {ms} ms");
            let (ms, s) = changes[2];
            assert_eq!(s, (11, 0), "seed {seed}");
            assert!((70_000..70_600).contains(&ms), "seed {seed}: hour point at {ms} ms");
            assert_eq!(changes.len(), 3, "seed {seed}: no other points");
        }
    }

    #[test]
    fn incremental_redraws_match() {
        // Across a point, so the scores change too.
        check_incremental(Box::new(Pong::new(false, Rng::new(3))), frames((9, 59, 52), 14));
    }
}
