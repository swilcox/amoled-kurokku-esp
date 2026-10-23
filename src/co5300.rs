//! CO5300 AMOLED driver over ESP-IDF's `esp_lcd` QSPI panel IO.
//!
//! The panel speaks the SH8601 command set. In QSPI mode every transaction
//! starts with a 32-bit "command" word: `opcode << 24 | dcs_cmd << 8`.
//! Opcode 0x02 writes parameters on one line; 0x32 writes pixel data on four.
//!
//! There is no framebuffer (480x480x2 = 460 KB > available RAM): callers send
//! rectangles of RGB565 pixels in wire (MSB-first) order, typically in strips.

use core::ffi::c_void;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Result};
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys::*;

use crate::board::*;

const OPCODE_WRITE_CMD: u32 = 0x02;
const OPCODE_WRITE_COLOR: u32 = 0x32;

const CMD_SLPOUT: u8 = 0x11;
const CMD_DISPOFF: u8 = 0x28;
const CMD_DISPON: u8 = 0x29;
const CMD_CASET: u8 = 0x2A;
const CMD_RASET: u8 = 0x2B;
const CMD_RAMWR: u8 = 0x2C;
const CMD_BRIGHTNESS: u8 = 0x51;

/// Vendor init sequence (ESP-IDF example `09_LVGL_V9_Test/display_bsp.cpp`):
/// (command, params, delay after in ms).
const INIT_CMDS: &[(u8, &[u8], u32)] = &[
    (CMD_SLPOUT, &[], 600),
    (0xFE, &[0x20], 0),
    (0x19, &[0x10], 0),
    (0x1C, &[0xA0], 0),
    (0xFE, &[0x00], 0),
    (0xC4, &[0x80], 0),
    (0x3A, &[0x55], 0), // 16 bpp
    (0x35, &[0x00], 0), // tearing effect on
    (0x36, &[0x30], 0), // MADCTL
    (0x53, &[0x20], 0),
    (CMD_BRIGHTNESS, &[0xFF], 0),
    (0x63, &[0xFF], 0),
    (CMD_CASET, &[0x00, 0x00, 0x01, 0xDF], 0),
    (CMD_RASET, &[0x00, 0x00, 0x01, 0xDF], 0),
    (CMD_DISPON, &[], 100),
];

/// Pixel rows per DMA transfer used by `fill_rect`.
pub const STRIP_ROWS: u16 = 32;

static COLOR_DONE: AtomicBool = AtomicBool::new(true);

unsafe extern "C" fn on_color_done(
    _io: esp_lcd_panel_io_handle_t,
    _edata: *mut esp_lcd_panel_io_event_data_t,
    _ctx: *mut c_void,
) -> bool {
    COLOR_DONE.store(true, Ordering::Release);
    false
}

fn check(err: esp_err_t, what: &str) -> Result<()> {
    EspError::convert(err).map_err(|e| anyhow!("{what}: {e}"))
}

pub struct Co5300 {
    io: esp_lcd_panel_io_handle_t,
    strip: Vec<u16>,
}

impl Co5300 {
    /// Set up SPI2 in quad mode and the panel IO. Call `pmic::reset_display`
    /// and then `init` before drawing.
    pub fn new() -> Result<Self> {
        let strip_px = LCD_W as usize * STRIP_ROWS as usize;

        let mut bus: spi_bus_config_t = Default::default();
        bus.sclk_io_num = LCD_PCLK;
        bus.__bindgen_anon_1.data0_io_num = LCD_D0;
        bus.__bindgen_anon_2.data1_io_num = LCD_D1;
        bus.__bindgen_anon_3.data2_io_num = LCD_D2;
        bus.__bindgen_anon_4.data3_io_num = LCD_D3;
        bus.data4_io_num = -1;
        bus.data5_io_num = -1;
        bus.data6_io_num = -1;
        bus.data7_io_num = -1;
        bus.max_transfer_sz = (strip_px * 2) as i32;
        check(
            unsafe { spi_bus_initialize(spi_host_device_t_SPI2_HOST, &bus, spi_common_dma_t_SPI_DMA_CH_AUTO) },
            "spi_bus_initialize",
        )?;

        let mut cfg: esp_lcd_panel_io_spi_config_t = Default::default();
        cfg.cs_gpio_num = LCD_CS;
        cfg.dc_gpio_num = -1;
        cfg.spi_mode = 0;
        cfg.pclk_hz = 40 * 1000 * 1000;
        cfg.trans_queue_depth = 1;
        cfg.on_color_trans_done = Some(on_color_done);
        cfg.lcd_cmd_bits = 32;
        cfg.lcd_param_bits = 8;
        cfg.flags.set_quad_mode(1);

        let mut io: esp_lcd_panel_io_handle_t = ptr::null_mut();
        check(
            unsafe {
                esp_lcd_new_panel_io_spi(spi_host_device_t_SPI2_HOST as esp_lcd_spi_bus_handle_t, &cfg, &mut io)
            },
            "esp_lcd_new_panel_io_spi",
        )?;

        Ok(Self { io, strip: vec![0; strip_px] })
    }

    /// Send the vendor init sequence. The panel must have just been reset.
    pub fn init(&mut self) -> Result<()> {
        for &(cmd, params, delay) in INIT_CMDS {
            self.cmd(cmd, params)?;
            if delay > 0 {
                FreeRtos::delay_ms(delay);
            }
        }
        Ok(())
    }

    fn cmd(&self, cmd: u8, params: &[u8]) -> Result<()> {
        let word = (OPCODE_WRITE_CMD << 24 | (cmd as u32) << 8) as i32;
        let p = if params.is_empty() { ptr::null() } else { params.as_ptr() as *const c_void };
        check(
            unsafe { esp_lcd_panel_io_tx_param(self.io, word, p, params.len()) },
            "esp_lcd_panel_io_tx_param",
        )
    }

    /// Panel brightness, 0-255.
    pub fn set_brightness(&self, level: u8) -> Result<()> {
        self.cmd(CMD_BRIGHTNESS, &[level])
    }

    /// Turn the panel output on/off (pixels are retained).
    #[allow(dead_code)]
    pub fn set_on(&self, on: bool) -> Result<()> {
        self.cmd(if on { CMD_DISPON } else { CMD_DISPOFF }, &[])
    }

    fn wait_idle(&self) {
        while !COLOR_DONE.load(Ordering::Acquire) {
            FreeRtos::delay_ms(1);
        }
    }

    /// Write `pixels` (wire order, row-major) into the inclusive rectangle
    /// `(x0, y0)..=(x1, y1)`. The controller needs an even start and odd end
    /// on both axes. Blocks until the DMA transfer completes.
    pub fn draw(&self, x0: u16, y0: u16, x1: u16, y1: u16, pixels: &[u16]) -> Result<()> {
        debug_assert!(x0 % 2 == 0 && y0 % 2 == 0 && x1 % 2 == 1 && y1 % 2 == 1);
        debug_assert!(x1 < LCD_W && y1 < LCD_H);
        let n = (x1 - x0 + 1) as usize * (y1 - y0 + 1) as usize;
        debug_assert!(pixels.len() >= n);

        self.wait_idle();
        let [xh0, xl0] = x0.to_be_bytes();
        let [xh1, xl1] = x1.to_be_bytes();
        let [yh0, yl0] = y0.to_be_bytes();
        let [yh1, yl1] = y1.to_be_bytes();
        self.cmd(CMD_CASET, &[xh0, xl0, xh1, xl1])?;
        self.cmd(CMD_RASET, &[yh0, yl0, yh1, yl1])?;

        COLOR_DONE.store(false, Ordering::Release);
        let word = (OPCODE_WRITE_COLOR << 24 | (CMD_RAMWR as u32) << 8) as i32;
        if let Err(e) = check(
            unsafe { esp_lcd_panel_io_tx_color(self.io, word, pixels.as_ptr() as *const c_void, n * 2) },
            "esp_lcd_panel_io_tx_color",
        ) {
            COLOR_DONE.store(true, Ordering::Release);
            return Err(e);
        }
        self.wait_idle();
        Ok(())
    }

    /// Fill an inclusive rectangle with one RGB565 color.
    pub fn fill_rect(&mut self, x0: u16, y0: u16, x1: u16, y1: u16, color: u16) -> Result<()> {
        let w = (x1 - x0 + 1) as usize;
        let rows = (STRIP_ROWS as usize * LCD_W as usize / w).min(LCD_H as usize) as u16 & !1;
        let wire = crate::color::to_wire(color);
        self.strip.fill(wire);
        let strip = core::mem::take(&mut self.strip);
        let mut y = y0;
        let res = (|| {
            while y <= y1 {
                let end = (y + rows - 1).min(y1);
                self.draw(x0, y, x1, end, &strip)?;
                y = end + 1;
            }
            Ok(())
        })();
        self.strip = strip;
        res
    }

    pub fn fill_screen(&mut self, color: u16) -> Result<()> {
        self.fill_rect(0, 0, LCD_W - 1, LCD_H - 1, color)
    }
}
