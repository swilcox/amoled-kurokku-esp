//! AMOLED Kurokku firmware library.
//!
//! Everything that doesn't touch hardware (fonts, rendering, clock faces,
//! time math, protocol parsing) is pure and builds/tests on the host. Device
//! drivers are gated on `target_os = "espidf"`.

pub mod assets;
pub mod board;
pub mod buttons;
pub mod clock;
pub mod color;
pub mod font;
pub mod fonts;
pub mod gfx;
pub mod image;
pub mod protocol;
pub mod qoi;
pub mod rtc;
pub mod sprites;
pub mod touch;
pub mod widget;

#[cfg(target_os = "espidf")]
pub mod co5300;
#[cfg(target_os = "espidf")]
pub mod config;
#[cfg(target_os = "espidf")]
pub mod engine;
#[cfg(target_os = "espidf")]
pub mod media;
#[cfg(target_os = "espidf")]
pub mod ota;
#[cfg(target_os = "espidf")]
pub mod pmic;
#[cfg(target_os = "espidf")]
pub mod poller;
#[cfg(target_os = "espidf")]
pub mod udp_log;
#[cfg(target_os = "espidf")]
pub mod wifi;
