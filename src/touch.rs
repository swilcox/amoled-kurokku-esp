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
