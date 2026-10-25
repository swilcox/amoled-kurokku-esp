//! `curtain`: red velvet stage curtains under a scalloped valance. They
//! part to show the クロック neon sign, which strikes up as they open, hold
//! a while, close again, and so on.
//!
//! The curtains are the same on every row, so one row of them is worked
//! out per frame and copied down the screen.

use super::super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::{rgb565, to_wire};
use crate::gfx::{Canvas, Rect, Scene, SCREEN};
use crate::sprites::neon::{SIGN, SIGN_H, SIGN_W};
use crate::widget::splash::{sign_palette, strike};

const W: i32 = LCD_W as i32;
/// Each curtain's width, closed and open.
const CLOSED: i32 = W / 2;
const OPEN: i32 = 30;
const FOLDS: i32 = 7;
const TRIM: i32 = 5;
/// Timeline, ms: closed, opening, open, closing.
const HOLD_CLOSED: u32 = 1500;
const MOVE: u32 = 3200;
const HOLD_OPEN: u32 = 9000;
const CYCLE: u32 = HOLD_CLOSED + MOVE + HOLD_OPEN + MOVE;
/// Valance: lowest point of its scallops, scallop width and depth.
const VALANCE: i32 = 62;
const SCALLOP_W: i32 = 80;
const SCALLOP_D: i32 = 12;
const FRINGE: i32 = 6;
const LEVELS: usize = 64;

const SIGN_X: i32 = (W - SIGN_W as i32) / 2;
const SIGN_Y: i32 = (LCD_H as i32 - SIGN_H as i32) / 2 + 20;

pub struct Curtain {
    /// Velvet and gold, dark to light, wire order.
    velvet: [u16; LEVELS],
    gold: [u16; LEVELS],
    /// Fold shading: level by phase (1/256 of a fold).
    fold: [u8; 256],
    /// This frame's curtains across one row (both sides), wire order.
    row: Vec<u16>,
    /// Curtain width now and as last drawn.
    width: i32,
    drawn_width: Option<i32>,
    /// Neon sign brightness now and as drawn, and its colors.
    sign: u16,
    drawn_sign: u16,
    palette: [u16; 256],
    start_ms: Option<u32>,
}

fn smoothstep(num: u32, den: u32) -> i32 {
    let p = (num as i64 * 1024 / den as i64).clamp(0, 1024);
    (p * p * (3 * 1024 - 2 * p) / (1024 * 1024)) as i32 // 0..1024
}

/// Curtain width and sign brightness `t` ms into a cycle.
fn timeline(t: u32) -> (i32, u16) {
    let t = t % CYCLE;
    let span = CLOSED - OPEN;
    let opening = HOLD_CLOSED;
    let closing = opening + MOVE + HOLD_OPEN;
    let width = if t < opening {
        CLOSED
    } else if t < opening + MOVE {
        CLOSED - span * smoothstep(t - opening, MOVE) / 1024
    } else if t < closing {
        OPEN
    } else {
        OPEN + span * smoothstep(t - closing, MOVE) / 1024
    };
    // The sign switches on as the curtains start to part, off once shut.
    let sign = if t < opening { 0 } else { strike(t - opening) };
    (width, sign)
}

fn ramp(dark: [f32; 3], light: [f32; 3], shine: [f32; 3]) -> [u16; LEVELS] {
    core::array::from_fn(|i| {
        let k = i as f32 / (LEVELS - 1) as f32;
        let mut c = [0, 1, 2].map(|j| dark[j] + (light[j] - dark[j]) * k);
        if k > 0.8 {
            let s = (k - 0.8) / 0.2;
            c = [0, 1, 2].map(|j| c[j] + (shine[j] - c[j]) * s * 0.6);
        }
        to_wire(rgb565(c[0] as u8, c[1] as u8, c[2] as u8))
    })
}

impl Curtain {
    pub fn new() -> Self {
        let fold = core::array::from_fn(|i| {
            let c = 0.5 + 0.5 * (i as f32 / 256.0 * std::f32::consts::TAU).cos();
            ((0.25 + 0.75 * c.powf(0.8)) * (LEVELS - 1) as f32) as u8
        });
        Self {
            velvet: ramp([28.0, 0.0, 4.0], [170.0, 14.0, 30.0], [235.0, 90.0, 110.0]),
            gold: ramp([70.0, 45.0, 10.0], [210.0, 165.0, 70.0], [255.0, 235.0, 170.0]),
            fold,
            row: vec![0; W as usize],
            width: CLOSED,
            drawn_width: None,
            sign: 0,
            drawn_sign: 0,
            palette: [0; 256],
            start_ms: None,
        }
    }

    /// Lay out one row of curtains `width` wide each.
    fn build_row(&mut self) {
        let w = self.width;
        for x in 0..w {
            // From the outer edge in; folds bunch up as the curtain opens.
            let phase = (x * FOLDS * 256 / w.max(1)) as usize & 255;
            let mut level = self.fold[phase] as usize;
            // Shade towards the inner edge, where the cloth turns away.
            let edge = w - x;
            if edge < 24 {
                level = level * (12 + edge as usize) / 36;
            }
            let c = if edge <= TRIM { self.gold[(LEVELS - 1) * (TRIM - edge + 2) as usize / (TRIM + 2) as usize] } else { self.velvet[level] };
            self.row[x as usize] = c;
            self.row[(W - 1 - x) as usize] = c;
        }
        for x in w..W - w {
            self.row[x as usize] = 0;
        }
    }

    /// Bottom of the valance (with its fringe) at column x.
    fn valance_edge(x: i32) -> i32 {
        let p = (x % SCALLOP_W) - SCALLOP_W / 2;
        VALANCE - SCALLOP_D + SCALLOP_D * (SCALLOP_W * SCALLOP_W / 4 - p * p) / (SCALLOP_W * SCALLOP_W / 4)
    }

    fn sign_rect() -> Rect {
        Rect::from_size(SIGN_X, SIGN_Y, SIGN_W as i32, SIGN_H as i32)
    }
}

impl Default for Curtain {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Curtain {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let t = now_ms.wrapping_sub(*self.start_ms.get_or_insert(now_ms));
        let (width, sign) = timeline(t);
        self.width = width;
        if sign != self.sign {
            self.sign = sign;
            sign_palette(sign, &mut self.palette);
        }
        let Some(drawn) = self.drawn_width else {
            self.build_row();
            (self.drawn_width, self.drawn_sign) = (Some(width), sign);
            return Step::Redraw(vec![SCREEN]);
        };
        let mut dirty = Vec::new();
        if width != drawn {
            self.build_row();
            // Everything a curtain covers changes as its folds bunch up.
            let reach = width.max(drawn);
            dirty.push(Rect::new(0, 0, reach - 1, LCD_H as i32 - 1));
            dirty.push(Rect::new(W - reach, 0, W - 1, LCD_H as i32 - 1));
            self.drawn_width = Some(width);
        }
        if sign != self.drawn_sign {
            dirty.extend(Self::sign_rect().intersect(&Rect::new(width, 0, W - 1 - width, LCD_H as i32 - 1)));
            self.drawn_sign = sign;
        }
        Step::Redraw(dirty)
    }

    fn invalidate(&mut self) {
        self.drawn_width = None;
    }

    fn frame_ms(&self) -> u32 {
        16
    }
}

impl Scene for Curtain {
    fn draw(&self, c: &mut Canvas) {
        let sign = SIGN.bytes();
        let sr = Self::sign_rect();
        let (lo, hi) = (self.width, W - 1 - self.width);
        c.rows(c.area(), |y, x0, row| {
            let xs = x0 as usize..x0 as usize + row.len();
            row.copy_from_slice(&self.row[xs.clone()]);
            // The stage between the curtains: the sign, if it's lit.
            if self.sign > 0 && (sr.y0..=sr.y1).contains(&y) && sign.len() >= SIGN_W * SIGN_H {
                let line = &sign[(y - sr.y0) as usize * SIGN_W..][..SIGN_W];
                for x in lo.max(sr.x0).max(x0)..=hi.min(sr.x1).min(x0 + row.len() as i32 - 1) {
                    row[(x - x0) as usize] = self.palette[line[(x - sr.x0) as usize] as usize];
                }
            }
            // The valance and its gold fringe over the top.
            if y < VALANCE + FRINGE {
                for (i, px) in row.iter_mut().enumerate() {
                    let x = x0 + i as i32;
                    let edge = Self::valance_edge(x);
                    if y < edge {
                        // Gathered in vertical pleats, darker at the top.
                        let pleat = self.fold[((x * 256 / 40) & 255) as usize] as i32;
                        let level = pleat * (30 + y.min(edge)) / (30 + VALANCE);
                        *px = self.velvet[level.clamp(0, LEVELS as i32 - 1) as usize];
                    } else if y < edge + FRINGE {
                        let level = if x % 4 < 2 { LEVELS - 1 } else { LEVELS / 2 };
                        *px = self.gold[level - (y - edge) as usize * 4];
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::check_incremental;
    use super::*;

    #[test]
    fn opens_holds_and_closes() {
        assert_eq!(timeline(0), (CLOSED, 0));
        let (w, _) = timeline(HOLD_CLOSED + MOVE / 2);
        assert!(OPEN < w && w < CLOSED);
        assert_eq!(timeline(HOLD_CLOSED + MOVE + 1000), (OPEN, 255));
        assert!(timeline(CYCLE - 1).0 >= CLOSED - 1);
        assert_eq!(timeline(CYCLE), (CLOSED, 0));
    }

    #[test]
    fn incremental_redraws_match() {
        let t = LocalTime::default();
        // Through opening, the sign striking, and a little of the hold.
        check_incremental(Box::new(Curtain::new()), (0..(HOLD_CLOSED + MOVE + 900) / 40).map(|i| (i * 40, t)));
    }
}
