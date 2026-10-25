//! Boot splash: クロック as a neon sign that flickers on, glows for a couple
//! of seconds, and fades out.

use super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::{rgb565, to_wire};
use crate::gfx::{Canvas, Rect, Scene, SCREEN};
use crate::sprites::neon::{RAMP, SIGN, SIGN_H, SIGN_W};

/// Brightness (0-255) from each time (ms) on: a neon tube striking, with
/// a few stutters before it holds.
const FLICKER: [(u32, u16); 9] =
    [(0, 0), (250, 140), (310, 12), (420, 205), (470, 25), (560, 255), (640, 90), (700, 255), (FADE_AT, 255)];
const FADE_AT: u32 = 2500;
const FADE_MS: u32 = 400;
const TOTAL_MS: u32 = FADE_AT + FADE_MS + 100;

const SIGN_Y: i32 = (LCD_H as i32 - SIGN_H as i32) / 2;

pub struct Splash {
    start_ms: Option<u32>,
    /// Brightness drawn; None until the first frame.
    shown: Option<u16>,
    /// RAMP at that brightness, RGB565 in wire order.
    palette: [u16; 256],
}

impl Splash {
    pub fn new() -> Self {
        Self { start_ms: None, shown: None, palette: [0; 256] }
    }

    fn sign_rect() -> Rect {
        Rect::from_size((LCD_W as i32 - SIGN_W as i32) / 2, SIGN_Y, SIGN_W as i32, SIGN_H as i32)
    }
}

/// A neon sign's brightness (0-255) `t` ms after it's switched on: a few
/// stutters, then steady.
pub fn strike(t: u32) -> u16 {
    FLICKER.iter().rev().find(|(at, _)| *at <= t).map_or(0, |&(_, b)| b)
}

/// The sign's colors at `brightness` (0-255), RGB565 in wire order.
pub fn sign_palette(brightness: u16, palette: &mut [u16; 256]) {
    for (dst, &rgb) in palette.iter_mut().zip(&RAMP) {
        let ch = |shift: u32| ((rgb >> shift & 0xFF) * brightness as u32 / 255) as u8;
        *dst = to_wire(rgb565(ch(16), ch(8), ch(0)));
    }
}

/// Sign brightness `t` ms in.
fn brightness(t: u32) -> u16 {
    if t >= FADE_AT {
        return (255 * FADE_MS.saturating_sub(t - FADE_AT) / FADE_MS) as u16;
    }
    strike(t)
}

impl Default for Splash {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Splash {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let t = now_ms.wrapping_sub(*self.start_ms.get_or_insert(now_ms));
        if t >= TOTAL_MS {
            return Step::Done;
        }
        let b = brightness(t);
        let first = self.shown.is_none();
        if self.shown.replace(b) == Some(b) {
            return Step::Redraw(vec![]);
        }
        sign_palette(b, &mut self.palette);
        Step::Redraw(vec![if first { SCREEN } else { Self::sign_rect() }])
    }

    fn invalidate(&mut self) {
        self.shown = None;
    }

    fn frame_ms(&self) -> u32 {
        16
    }
}

impl Scene for Splash {
    fn draw(&self, c: &mut Canvas) {
        c.fill(c.area(), 0);
        let r = Self::sign_rect();
        c.blit_indexed(r.x0, r.y0, SIGN_W, SIGN_H, SIGN.bytes(), &self.palette);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flickers_on_holds_then_fades_out() {
        assert_eq!(brightness(0), 0);
        assert_eq!(brightness(300), 140);
        assert_eq!(brightness(320), 12);
        assert_eq!(brightness(1500), 255);
        assert!(brightness(FADE_AT + FADE_MS / 2) < 255);
        assert_eq!(brightness(FADE_AT + FADE_MS), 0);
    }

    #[test]
    fn redraws_the_sign_only_when_brightness_changes_and_ends() {
        let t = LocalTime::default();
        let mut s = Splash::new();
        assert!(matches!(s.tick(1000, &t), Step::Redraw(r) if r == vec![SCREEN]));
        assert!(matches!(s.tick(1100, &t), Step::Redraw(r) if r.is_empty()));
        assert!(matches!(s.tick(1250, &t), Step::Redraw(r) if r == vec![Splash::sign_rect()]));
        assert!(matches!(s.tick(1000 + TOTAL_MS, &t), Step::Done));
    }
}
