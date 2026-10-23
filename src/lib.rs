//! AMOLED Kurokku firmware library.
//!
//! Everything that doesn't touch hardware (fonts, rendering, clock faces,
//! time math, protocol parsing) is pure and builds/tests on the host. Device
//! drivers are gated on `target_os = "espidf"`.

pub mod board;
pub mod buttons;
pub mod clock;
pub mod color;
pub mod font;
pub mod fonts;
pub mod gfx;
pub mod rtc;
pub mod touch;

#[cfg(target_os = "espidf")]
pub mod co5300;
#[cfg(target_os = "espidf")]
pub mod pmic;
