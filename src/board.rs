//! Pin and bus assignments for the Waveshare ESP32-C6-Touch-AMOLED-2.16.
//!
//! Taken from the vendor ESP-IDF examples (`display_bsp.h` defaults, which are
//! what actually run) — the vendor `user_config.h` has LCD CS and touch INT
//! swapped and should not be trusted.

/// Panel resolution (square).
pub const LCD_W: u16 = 480;
pub const LCD_H: u16 = 480;

// QSPI display (CO5300) on SPI2. The microSD card shares CLK/D0/D1.
pub const LCD_PCLK: i32 = 0;
pub const LCD_D0: i32 = 1;
pub const LCD_D1: i32 = 2;
pub const LCD_D2: i32 = 3;
pub const LCD_D3: i32 = 4;
pub const LCD_CS: i32 = 15;

// Touch controller (CST9217) on the shared I2C bus.
pub const TP_INT: i32 = 5;
pub const TP_RST: i32 = 11;

// microSD (SDSPI on SPI2): CLK=0, MOSI=1, MISO=2.
#[allow(dead_code)]
pub const SD_CS: i32 = 6;

// Buttons (identified by the Phase 0 probe):
// - BOOT: GPIO9, active-low. A strapping pin, but a normal input after boot.
// - KEY:  GPIO10, active-low.
// - PWR:  routed through the AXP2101 (short/long-press IRQs, see `pmic`), and
//   also visible on GPIO18, which idles low and reads high while pressed.
//   Holding PWR ~4 s is a hardware power-off.
pub const BTN_BOOT: i32 = 9;
pub const BTN_KEY: i32 = 10;
pub const BTN_PWR_SENSE: i32 = 18;

// I2C addresses (bus: SCL=GPIO7, SDA=GPIO8).
pub const ADDR_PMIC: u8 = 0x34;
pub const ADDR_RTC: u8 = 0x51;
pub const ADDR_TOUCH: u8 = 0x5A;
