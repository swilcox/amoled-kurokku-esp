//! QMI8658 accelerometer (I2C 0x6B): which way is down, for auto-rotate.
//!
//! Only the accelerometer runs (4 g range, 31.25 Hz); the gyro stays off.

use crate::gfx::Rotation;

/// Raw counts per g at the 4 g range.
pub const COUNTS_PER_G: i32 = 8192;
/// An in-plane axis must carry this much of gravity (0.6 g, ~37° of tilt
/// from flat) to pick an orientation...
const MIN_TILT: i32 = COUNTS_PER_G * 6 / 10;
/// ...and beat the other in-plane axis by this factor (~63° from the
/// diagonal), so a device near 45° doesn't flicker between two.
const DOMINANCE: i32 = 2;
/// A new orientation must hold this many consecutive samples.
const STABLE_SAMPLES: u8 = 5;

/// The screen rotation that puts "down" at the bottom, for gravity along
/// the board's X or Y axis (`(axis_is_x, positive)`). The accelerometer
/// reads +1 g on the axis pointing *up*; checked on the device.
fn rotation_for(x_axis: bool, positive: bool) -> Rotation {
    match (x_axis, positive) {
        (false, true) => Rotation::R0,
        (true, false) => Rotation::R90,
        (false, false) => Rotation::R180,
        (true, true) => Rotation::R270,
    }
}

/// Picks the screen rotation from accelerometer samples. Pure —
/// host-testable.
#[derive(Default)]
pub struct Orientation {
    current: Option<Rotation>,
    candidate: Option<Rotation>,
    count: u8,
}

impl Orientation {
    /// Feed one sample (raw counts). Returns the new rotation when it
    /// changes. Lying flat or tilted ambiguously keeps the current one.
    pub fn update(&mut self, (x, y, _z): (i16, i16, i16)) -> Option<Rotation> {
        let (x, y) = (x as i32, y as i32);
        let seen = if x.abs() >= MIN_TILT && x.abs() >= DOMINANCE * y.abs() {
            Some(rotation_for(true, x > 0))
        } else if y.abs() >= MIN_TILT && y.abs() >= DOMINANCE * x.abs() {
            Some(rotation_for(false, y > 0))
        } else {
            None
        };
        let seen = seen?;
        if Some(seen) != self.candidate {
            self.candidate = Some(seen);
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        (self.count >= STABLE_SAMPLES && self.current != Some(seen)).then(|| {
            self.current = Some(seen);
            seen
        })
    }
}

#[cfg(target_os = "espidf")]
mod device {
    use anyhow::{anyhow, bail, Result};
    use esp_idf_svc::hal::delay::BLOCK;
    use esp_idf_svc::hal::i2c::I2cDriver;

    use crate::board::ADDR_IMU;

    const REG_WHO_AM_I: u8 = 0x00;
    const REG_CTRL1: u8 = 0x02;
    const REG_CTRL2: u8 = 0x03;
    const REG_CTRL7: u8 = 0x08;
    const REG_AX_L: u8 = 0x35;
    const WHO_AM_I: u8 = 0x05;

    fn write(i2c: &mut I2cDriver, reg: u8, v: u8) -> Result<()> {
        i2c.write(ADDR_IMU, &[reg, v], BLOCK).map_err(|e| anyhow!("IMU write {reg:#04x}: {e}"))
    }

    pub fn init(i2c: &mut I2cDriver) -> Result<()> {
        let mut id = [0u8];
        i2c.write_read(ADDR_IMU, &[REG_WHO_AM_I], &mut id, BLOCK)
            .map_err(|e| anyhow!("IMU not found: {e}"))?;
        if id[0] != WHO_AM_I {
            bail!("unexpected IMU id {:#04x}", id[0]);
        }
        write(i2c, REG_CTRL1, 0x40)?; // register auto-increment, little-endian
        write(i2c, REG_CTRL2, 0x01 << 4 | 0x08)?; // ±4 g, 31.25 Hz
        write(i2c, REG_CTRL7, 0x01)?; // accelerometer only
        Ok(())
    }

    /// One accelerometer sample, raw counts (see `COUNTS_PER_G`).
    pub fn read(i2c: &mut I2cDriver) -> Result<(i16, i16, i16)> {
        let mut b = [0u8; 6];
        i2c.write_read(ADDR_IMU, &[REG_AX_L], &mut b, BLOCK)
            .map_err(|e| anyhow!("IMU read: {e}"))?;
        let v = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]);
        Ok((v(0), v(2), v(4)))
    }
}

#[cfg(target_os = "espidf")]
pub use device::*;

#[cfg(test)]
mod tests {
    use super::*;

    const G: i16 = COUNTS_PER_G as i16;

    fn feed(o: &mut Orientation, s: (i16, i16, i16), n: usize) -> Vec<Rotation> {
        (0..n).filter_map(|_| o.update(s)).collect()
    }

    #[test]
    fn settles_after_stable_samples() {
        let mut o = Orientation::default();
        assert_eq!(feed(&mut o, (0, -G, 0), STABLE_SAMPLES as usize - 1), vec![]);
        assert_eq!(feed(&mut o, (0, -G, 0), 3), vec![Rotation::R180]);
        assert_eq!(feed(&mut o, (-G, 0, 0), 10), vec![Rotation::R90]);
    }

    #[test]
    fn flat_and_diagonal_keep_current() {
        let mut o = Orientation::default();
        feed(&mut o, (0, G, 0), 10);
        assert_eq!(feed(&mut o, (0, 0, G), 10), vec![]);
        assert_eq!(feed(&mut o, (G / 10 * 6, G / 10 * 5, G / 2), 10), vec![]);
    }

    #[test]
    fn a_wobble_restarts_the_count() {
        let mut o = Orientation::default();
        feed(&mut o, (0, -G, 0), 10);
        let mut got = feed(&mut o, (-G, 0, 0), 3);
        got.extend(feed(&mut o, (0, -G, 0), 1));
        got.extend(feed(&mut o, (-G, 0, 0), 3));
        assert_eq!(got, vec![]);
    }
}
