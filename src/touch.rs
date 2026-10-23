//! CST9217 touch controller (single point).
//!
//! Protocol from Waveshare's `esp_lcd_touch_cst9217` component: write the
//! 16-bit register 0xD000, wait ~2 ms, read 10 bytes. Byte 6 must be 0xAB.

pub const DATA_LEN: usize = 10;
const ACK: u8 = 0xAB;
const STATUS_PRESSED: u8 = 0x06;

/// Parse a raw report into panel coordinates, (0, 0) = top-left of the
/// image as drawn with our MADCTL (0x30). That needs `swap_xy` + `mirror_x`;
/// the vendor's `swap_xy` + `mirror_y` flags put touches 180° off.
pub fn parse(d: &[u8; DATA_LEN], max: u16) -> Option<(u16, u16)> {
    if d[6] != ACK || d[5] & 0x7F == 0 || d[0] & 0x0F != STATUS_PRESSED {
        return None;
    }
    let raw_x = (d[1] as u16) << 4 | (d[3] as u16) >> 4;
    let raw_y = (d[2] as u16) << 4 | (d[3] as u16) & 0x0F;
    Some((max.saturating_sub(raw_y), raw_x.min(max)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Swipe {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Tap(i32, i32),
    /// Reported once, while still touching.
    LongPress(i32, i32),
    Swipe(Swipe),
}

/// Travel that makes a swipe (along the dominant axis).
const SWIPE_PX: i32 = 70;
/// Wobble allowed in a tap or long press.
const STILL_PX: i32 = 24;
const LONG_MS: u32 = 700;
/// Samples with no touch before a release counts: the controller drops
/// the odd report mid-touch.
const RELEASE_SAMPLES: u8 = 2;

/// Turns polled touch points into gestures. Pure — host-testable.
#[derive(Default)]
pub struct Gestures {
    /// First point, time, last point, long press reported.
    down: Option<((i32, i32), u32, (i32, i32), bool)>,
    misses: u8,
}

impl Gestures {
    /// Feed one poll: the touch point (screen coordinates) or None.
    pub fn update(&mut self, point: Option<(i32, i32)>, now_ms: u32) -> Option<Gesture> {
        match (point, &mut self.down) {
            (Some(p), None) => {
                self.down = Some((p, now_ms, p, false));
                self.misses = 0;
                None
            }
            (Some(p), Some((start, t0, last, long))) => {
                self.misses = 0;
                *last = p;
                let still = (p.0 - start.0).abs() <= STILL_PX && (p.1 - start.1).abs() <= STILL_PX;
                if !still {
                    // Moved away: not a long press, even if it comes back.
                    *long = true;
                } else if !*long && now_ms.wrapping_sub(*t0) >= LONG_MS {
                    *long = true;
                    return Some(Gesture::LongPress(start.0, start.1));
                }
                None
            }
            (None, Some(_)) => {
                self.misses += 1;
                if self.misses < RELEASE_SAMPLES {
                    return None;
                }
                let (start, t0, last, long) = self.down.take()?;
                let (dx, dy) = (last.0 - start.0, last.1 - start.1);
                if dx.abs().max(dy.abs()) >= SWIPE_PX {
                    Some(Gesture::Swipe(match (dx.abs() > dy.abs(), dx > 0, dy > 0) {
                        (true, true, _) => Swipe::Right,
                        (true, false, _) => Swipe::Left,
                        (false, _, true) => Swipe::Down,
                        (false, _, false) => Swipe::Up,
                    }))
                } else if !long && now_ms.wrapping_sub(t0) < LONG_MS && dx.abs().max(dy.abs()) <= STILL_PX {
                    Some(Gesture::Tap(start.0, start.1))
                } else {
                    None
                }
            }
            (None, None) => None,
        }
    }
}

#[cfg(target_os = "espidf")]
mod device {
    use anyhow::{anyhow, Result};
    use esp_idf_svc::hal::delay::{FreeRtos, BLOCK};
    use esp_idf_svc::hal::i2c::I2cDriver;
    use esp_idf_svc::sys::*;

    use crate::board::*;

    /// Configure INT as input and pulse RST.
    pub fn init() -> Result<()> {
        unsafe {
            gpio_reset_pin(TP_INT);
            gpio_set_direction(TP_INT, gpio_mode_t_GPIO_MODE_INPUT);
            gpio_set_pull_mode(TP_INT, gpio_pull_mode_t_GPIO_PULLUP_ONLY);
            gpio_reset_pin(TP_RST);
            gpio_set_direction(TP_RST, gpio_mode_t_GPIO_MODE_OUTPUT);
            gpio_set_level(TP_RST, 0);
        }
        FreeRtos::delay_ms(10);
        unsafe { gpio_set_level(TP_RST, 1) };
        FreeRtos::delay_ms(100);
        Ok(())
    }

    /// INT is pulled low while the controller has data.
    pub fn pending() -> bool {
        unsafe { gpio_get_level(TP_INT) == 0 }
    }

    pub fn read(i2c: &mut I2cDriver) -> Result<Option<(u16, u16)>> {
        i2c.write(ADDR_TOUCH, &[0xD0, 0x00], BLOCK)
            .map_err(|e| anyhow!("touch write: {e}"))?;
        FreeRtos::delay_ms(2);
        let mut d = [0u8; super::DATA_LEN];
        i2c.read(ADDR_TOUCH, &mut d, BLOCK)
            .map_err(|e| anyhow!("touch read: {e}"))?;
        Ok(super::parse(&d, LCD_W - 1))
    }
}

#[cfg(target_os = "espidf")]
pub use device::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn report(x: u16, y: u16) -> [u8; DATA_LEN] {
        let mut d = [0u8; DATA_LEN];
        d[0] = STATUS_PRESSED;
        d[1] = (x >> 4) as u8;
        d[2] = (y >> 4) as u8;
        d[3] = ((x & 0x0F) << 4 | (y & 0x0F)) as u8;
        d[5] = 1;
        d[6] = ACK;
        d
    }

    #[test]
    fn decodes_and_transforms() {
        assert_eq!(parse(&report(100, 300), 479), Some((179, 100)));
    }

    fn run(points: &[(u32, Option<(i32, i32)>)]) -> Vec<Gesture> {
        let mut g = Gestures::default();
        points.iter().filter_map(|&(t, p)| g.update(p, t)).collect()
    }

    #[test]
    fn tap_survives_a_dropped_sample() {
        let got = run(&[(0, Some((100, 100))), (20, None), (40, Some((102, 99))), (60, None), (80, None)]);
        assert_eq!(got, vec![Gesture::Tap(100, 100)]);
    }

    #[test]
    fn swipes_by_dominant_axis() {
        let swipe = |to: (i32, i32)| run(&[(0, Some((240, 240))), (50, Some(to)), (100, None), (120, None)]);
        assert_eq!(swipe((100, 260)), vec![Gesture::Swipe(Swipe::Left)]);
        assert_eq!(swipe((400, 200)), vec![Gesture::Swipe(Swipe::Right)]);
        assert_eq!(swipe((250, 100)), vec![Gesture::Swipe(Swipe::Up)]);
        assert_eq!(swipe((230, 420)), vec![Gesture::Swipe(Swipe::Down)]);
        // Too short for a swipe, too far for a tap.
        assert_eq!(swipe((280, 240)), vec![]);
    }

    #[test]
    fn long_press_fires_once_while_held() {
        let got = run(&[(0, Some((50, 60))), (400, Some((52, 60))), (720, Some((51, 61))), (900, Some((50, 60))), (950, None), (970, None)]);
        assert_eq!(got, vec![Gesture::LongPress(50, 60)]);
    }

    #[test]
    fn rejects_bad_ack_and_no_points() {
        let mut d = report(1, 2);
        d[6] = 0;
        assert_eq!(parse(&d, 479), None);
        let mut d = report(1, 2);
        d[5] = 0;
        assert_eq!(parse(&d, 479), None);
    }
}
