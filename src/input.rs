//! Input thread: polls the touch controller and the accelerometer, which
//! share the I2C bus with the RTC, and sends gestures and orientation
//! changes to the display thread.
//!
//! A touch read takes ~3 ms (the controller needs 2 ms between the register
//! write and the read). Doing that on the display thread would stall a
//! smooth scroll every few frames; here it only sleeps this thread.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::i2c::I2cDriver;

use crate::gfx::Rotation;
use crate::imu::{self, Orientation};
use crate::touch::{self, Gesture, Gestures};

pub type SharedI2c = Arc<Mutex<I2cDriver<'static>>>;

const TOUCH_PERIOD: Duration = Duration::from_millis(20);
/// IMU samples every this many touch polls (5 Hz).
const IMU_EVERY: u32 = 10;

pub enum Input {
    Gesture(Gesture),
    /// The way the device is now standing.
    Orientation(Rotation),
}

/// The rotation the display thread has applied, so touches map to screen
/// coordinates. Stored as `Rotation as u8`.
#[derive(Clone, Default)]
pub struct AppliedRotation(Arc<AtomicU8>);

impl AppliedRotation {
    pub fn set(&self, r: Rotation) {
        self.0.store(r as u8, Ordering::Relaxed);
    }

    fn get(&self) -> Rotation {
        Rotation::ALL[self.0.load(Ordering::Relaxed) as usize & 3]
    }
}

pub fn spawn(i2c: SharedI2c, applied: AppliedRotation, tx: Sender<Input>) {
    std::thread::Builder::new()
        .name("input".into())
        .stack_size(6 * 1024)
        .spawn(move || run(i2c, applied, tx))
        .expect("failed to spawn input thread");
}

fn run(i2c: SharedI2c, applied: AppliedRotation, tx: Sender<Input>) {
    let imu_ok = match imu::init(&mut i2c.lock().unwrap()) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("{e}; no auto-rotate");
            false
        }
    };
    let boot = Instant::now();
    let mut gestures = Gestures::default();
    let mut orientation = Orientation::default();
    let mut touch_errors = 0u32;
    for n in 0u32.. {
        let point = match touch::read(&mut i2c.lock().unwrap()) {
            Ok(p) => p,
            Err(e) => {
                touch_errors += 1;
                if touch_errors.is_power_of_two() {
                    log::warn!("touch read failed ({touch_errors}x): {e}");
                }
                None
            }
        };
        let point = point.map(|(x, y)| applied.get().to_screen((x as i32, y as i32)));
        if let Some(g) = gestures.update(point, boot.elapsed().as_millis() as u32) {
            log::info!("touch: {g:?}");
            let _ = tx.send(Input::Gesture(g));
        }

        if imu_ok && n % IMU_EVERY == 0 {
            match imu::read(&mut i2c.lock().unwrap()) {
                Ok(sample) => {
                    if let Some(r) = orientation.update(sample) {
                        log::info!("orientation: {}° (accel {sample:?})", r.degrees());
                        let _ = tx.send(Input::Orientation(r));
                    }
                }
                Err(e) => log::debug!("{e}"),
            }
        }
        std::thread::sleep(TOUCH_PERIOD);
    }
}
