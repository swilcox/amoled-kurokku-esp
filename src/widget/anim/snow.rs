//! `static`: an untuned TV. Grey snow in 3-px grains with scanlines, and a
//! brighter band rolling slowly down the picture as if the vertical hold
//! were drifting. Now and then a station almost comes in: one of the
//! `assets/tv` pictures fades up through the snow, rolling until the
//! vertical hold catches, wobbling sideways with a faint ghost, then loses
//! lock and rolls away again.

use super::super::{Step, Widget};
use crate::board::LCD_H;
use crate::clock::LocalTime;
use crate::color::{rgb565, to_wire};
use crate::gfx::{Canvas, Scene, SCREEN};
use crate::rng::Rng;
use crate::sprites::tv::{PICTURES, SIZE};

const GRAIN: i32 = 3;
/// Rolling band: height, and speed in px/s.
const BAND_H: i32 = 90;
const BAND_SPEED: u32 = 45;
const LEVELS: usize = 64;

/// Pictures are SIZE×SIZE, one pixel per grain (480 / GRAIN). A rolling
/// picture shows a black blanking bar between frames.
const BLANK: i32 = 14;
const PERIOD: i32 = SIZE as i32 + BLANK;
/// The ghost: a faint copy this many grains to the right.
const GHOST: i32 = 6;
/// The first station comes in this soon; later ones after a random wait.
const FIRST_MS: u32 = 4000;
const WAIT_MS: (i32, i32) = (15_000, 45_000);
/// A station's visit: fade up, hold, fade out (ms).
const VISIT_MS: (i32, i32) = (6_000, 11_000);
const RISE_MS: u32 = 1500;
const FALL_MS: u32 = 1800;

/// A station coming in.
struct Visit {
    pic: usize,
    start: u32,
    len: u32,
    /// Strongest signal, of 256: some snow always stays.
    peak: u32,
    /// Grain rows the picture is rolled by when it first appears.
    roll0: f32,
    /// Which way it rolls away.
    away: f32,
}

pub struct Snow {
    /// Grey ramp (slightly cool, like a CRT) in wire order, and a darker
    /// copy for scanlines.
    grey: [u16; LEVELS],
    dim: [u16; LEVELS],
    frame: u32,
    start_ms: Option<u32>,
    /// Top of the rolling band.
    band: i32,
    rng: Rng,
    visit: Option<Visit>,
    next_visit: u32,
    last_pic: usize,
    /// This frame's station: signal strength (0-256), picture, vertical
    /// roll in grain rows, and horizontal offset per grain row.
    signal: u32,
    pic: usize,
    roll: i32,
    shift: [i8; SIZE],
}

fn hash(x: i32, y: i32, frame: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ frame.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^ h >> 12
}

impl Snow {
    pub fn new() -> Self {
        let ramp = |k: f32| {
            core::array::from_fn(|i| {
                let v = i as f32 / (LEVELS - 1) as f32 * 235.0 * k;
                to_wire(rgb565(v as u8, v as u8, (v * 1.06).min(255.0) as u8))
            })
        };
        Self {
            grey: ramp(1.0),
            dim: ramp(0.7),
            frame: 0,
            start_ms: None,
            band: -BAND_H,
            rng: Rng::from_entropy(),
            visit: None,
            next_visit: FIRST_MS,
            last_pic: usize::MAX,
            signal: 0,
            pic: 0,
            roll: 0,
            shift: [0; SIZE],
        }
    }

    fn start_visit(&mut self, t: u32) {
        let n = PICTURES.len();
        let mut pic = self.rng.below(n as u32) as usize;
        if pic == self.last_pic && n > 1 {
            pic = (pic + 1 + self.rng.below(n as u32 - 1) as usize) % n;
        }
        self.last_pic = pic;
        let sign = |r: &mut Rng| if r.next() & 1 == 0 { 1.0 } else { -1.0 };
        let roll0 = sign(&mut self.rng) * self.rng.range(PERIOD * 3 / 2, PERIOD * 7 / 2) as f32;
        let away = sign(&mut self.rng);
        self.visit = Some(Visit {
            pic,
            start: t,
            len: self.rng.range(VISIT_MS.0, VISIT_MS.1) as u32,
            peak: self.rng.range(130, 200) as u32,
            roll0,
            away,
        });
    }

    /// Work out this frame's signal, roll and wobble.
    fn tune(&mut self, t: u32) {
        if self.visit.is_none() && t >= self.next_visit && !PICTURES.is_empty() {
            self.start_visit(t);
        }
        let Some(v) = &self.visit else {
            self.signal = 0;
            return;
        };
        let u = t - v.start;
        if u >= v.len {
            self.visit = None;
            self.signal = 0;
            self.next_visit = t + self.rng.range(WAIT_MS.0, WAIT_MS.1) as u32;
            return;
        }
        let fall_at = v.len - FALL_MS;
        // Fade up and down (smoothstep), flickering as it comes and goes.
        let ramp = if u < RISE_MS { u as f32 / RISE_MS as f32 } else if u > fall_at { (v.len - u) as f32 / FALL_MS as f32 } else { 1.0 };
        let env = ramp * ramp * (3.0 - 2.0 * ramp);
        let mut s = v.peak as f32 * env;
        if self.rng.below(if env < 1.0 { 4 } else { 9 }) == 0 {
            s *= 0.5 + self.rng.below(40) as f32 / 100.0;
        }
        // The vertical hold catches (the roll slows to a stop), then lets go.
        let roll = if u <= fall_at {
            v.roll0 * (-(u as f32) / 550.0).exp()
        } else {
            let d = (u - fall_at) as f32 / 1000.0;
            v.away * d * d * 120.0
        };
        // Sideways wobble, worse while the signal is weak, and a hook at the
        // top of each picture where the horizontal hold is pulled.
        let wobble = 0.8 + 4.0 * (1.0 - env);
        let phase = u as f32 * 0.005;
        self.roll = roll as i32;
        for (gy, s) in self.shift.iter_mut().enumerate() {
            let row = (gy as i32 + self.roll).rem_euclid(PERIOD);
            let hook = if row < 12 { (12 - row) as f32 * 0.35 } else { 0.0 };
            *s = (wobble * (gy as f32 * 0.19 + phase).sin() + hook) as i8;
        }
        self.signal = s as u32;
        self.pic = v.pic;
    }
}

impl Default for Snow {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Snow {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let t = now_ms.wrapping_sub(*self.start_ms.get_or_insert(now_ms));
        self.frame = self.frame.wrapping_add(1);
        let span = LCD_H as u32 + BAND_H as u32;
        self.band = (t / 1000 * BAND_SPEED + t % 1000 * BAND_SPEED / 1000) as i32 % span as i32 - BAND_H;
        self.tune(t);
        Step::Redraw(vec![SCREEN])
    }

    fn invalidate(&mut self) {}

    fn frame_ms(&self) -> u32 {
        33
    }
}

impl Scene for Snow {
    fn draw(&self, c: &mut Canvas) {
        let pic = if self.signal > 0 { PICTURES[self.pic].bytes() } else { &[] };
        let s = self.signal;
        // One random level per grain, shared by the grain's rows.
        let mut levels: Vec<u8> = Vec::new();
        let mut levels_for = i32::MIN;
        c.rows(c.area(), |y, x0, row| {
            let gy = y / GRAIN;
            if gy != levels_for {
                levels_for = gy;
                let mut rng = Rng::new(hash(gy, 0, self.frame));
                // The station's row of the picture (None: the blanking bar).
                let line = (!pic.is_empty()).then(|| {
                    let r = (gy + self.roll).rem_euclid(PERIOD);
                    let line = (r < SIZE as i32).then(|| &pic[r as usize * SIZE..][..SIZE]);
                    (line, self.shift[gy as usize % SIZE] as i32)
                });
                levels.clear();
                levels.extend((0..=(x0 + row.len() as i32) / GRAIN).map(|gx| {
                    // Mid greys mostly (an average of two draws), with the
                    // odd bright speck.
                    let h = rng.next();
                    let n = ((h & 0x3F) + (h >> 6 & 0x3F)) / 2 + if h >> 28 == 0 { 20 } else { 0 };
                    let v = match line {
                        None => n,
                        Some((line, shift)) => {
                            let px = |x: i32| line.map_or(0, |l| l[x.clamp(0, SIZE as i32 - 1) as usize] as u32);
                            let x = gx - shift;
                            let p = (px(x) * 3 + px(x - GHOST)) / 16;
                            (n * (256 - s) + p * s) >> 8
                        }
                    };
                    v.min(LEVELS as u32 - 1) as u8
                }));
            }
            // The rolling band brightens smoothly towards its middle.
            let into = y - self.band;
            let boost = if (0..BAND_H).contains(&into) { 12 - (into * 24 / BAND_H - 12).abs() } else { 0 } as u8;
            let ramp = if y % GRAIN == GRAIN - 1 { &self.dim } else { &self.grey };
            // A grain at a time; the first may be cut by the canvas edge.
            let (mut i, mut g, mut run) = (0, (x0 / GRAIN) as usize, (GRAIN - x0 % GRAIN) as usize);
            while i < row.len() {
                let n = run.min(row.len() - i);
                let l = levels[g].saturating_add(boost).min(LEVELS as u8 - 1);
                row[i..i + n].fill(ramp[l as usize]);
                (i, g, run) = (i + n, g + 1, GRAIN as usize);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_is_new_snow_and_the_band_rolls() {
        let mut s = Snow::new();
        let t = LocalTime::default();
        s.tick(0, &t);
        let b0 = s.band;
        s.tick(1000, &t);
        assert_eq!(s.band - b0, BAND_SPEED as i32);
        assert_ne!(hash(1, 1, 1), hash(1, 1, 2));
    }

    #[test]
    fn a_station_comes_in_locks_and_goes() {
        assert!(!PICTURES.is_empty());
        assert!(PICTURES.iter().all(|p| p.bytes().len() == SIZE * SIZE));
        let mut s = Snow::new();
        let t = LocalTime::default();
        let mut ms = 0;
        let mut tick = |s: &mut Snow, until: u32| {
            while ms <= until {
                s.tick(ms, &t);
                ms += 33;
            }
        };
        tick(&mut s, FIRST_MS - 100);
        assert_eq!(s.signal, 0, "snow only before the first station");
        // Locked by the end of the shortest hold.
        tick(&mut s, FIRST_MS + VISIT_MS.0 as u32 - FALL_MS);
        assert!(s.visit.is_some() && s.roll == 0, "roll {}", s.roll);
        // Gone after the longest visit, with the next one a while off.
        tick(&mut s, FIRST_MS + VISIT_MS.1 as u32 + 100);
        assert!(s.visit.is_none() && s.signal == 0);
        assert!(s.next_visit >= FIRST_MS + (VISIT_MS.0 + WAIT_MS.0) as u32);
    }

    #[test]
    fn redraws_match_full_frames_while_tuned_in() {
        let t = LocalTime::default();
        let steps = [0].into_iter().chain((0..8).map(|i| FIRST_MS + 1000 + i * 700));
        super::super::tests::check_incremental(Box::new(Snow::new()), steps.map(|ms| (ms, t)));
    }
}
