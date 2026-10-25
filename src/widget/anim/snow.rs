//! `static`: an untuned TV. Grey snow in 3-px grains with scanlines, and a
//! brighter band rolling slowly down the picture as if the vertical hold
//! were drifting.

use super::super::{Step, Widget};
use crate::board::LCD_H;
use crate::clock::LocalTime;
use crate::color::{rgb565, to_wire};
use crate::gfx::{Canvas, Scene, SCREEN};
use crate::rng::Rng;

const GRAIN: i32 = 3;
/// Rolling band: height, and speed in px/s.
const BAND_H: i32 = 90;
const BAND_SPEED: u32 = 45;
const LEVELS: usize = 64;

pub struct Snow {
    /// Grey ramp (slightly cool, like a CRT) in wire order, and a darker
    /// copy for scanlines.
    grey: [u16; LEVELS],
    dim: [u16; LEVELS],
    frame: u32,
    start_ms: Option<u32>,
    /// Top of the rolling band.
    band: i32,
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
        Self { grey: ramp(1.0), dim: ramp(0.7), frame: 0, start_ms: None, band: -BAND_H }
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
        Step::Redraw(vec![SCREEN])
    }

    fn invalidate(&mut self) {}

    fn frame_ms(&self) -> u32 {
        33
    }
}

impl Scene for Snow {
    fn draw(&self, c: &mut Canvas) {
        // One random level per grain, shared by the grain's rows.
        let mut levels: Vec<u8> = Vec::new();
        let mut levels_for = i32::MIN;
        c.rows(c.area(), |y, x0, row| {
            let gy = y / GRAIN;
            if gy != levels_for {
                levels_for = gy;
                let mut rng = Rng::new(hash(gy, 0, self.frame));
                levels.clear();
                levels.extend((0..=(x0 + row.len() as i32) / GRAIN).map(|_| {
                    // Mid greys mostly (an average of two draws), with the
                    // odd bright speck.
                    let h = rng.next();
                    let v = ((h & 0x3F) + (h >> 6 & 0x3F)) / 2 + if h >> 28 == 0 { 20 } else { 0 };
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
}
