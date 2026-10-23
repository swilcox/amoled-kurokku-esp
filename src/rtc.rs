//! PCF85063 real-time clock. Keeps time across reboots so the clock works
//! before (or without) WiFi/NTP.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

fn bcd(v: u8) -> u8 {
    (v >> 4) * 10 + (v & 0x0F)
}

/// Decode registers 0x04..=0x0A. Returns `None` if the oscillator-stopped
/// flag is set (time was lost and must be set again).
pub fn decode(r: &[u8; 7]) -> Option<DateTime> {
    if r[0] & 0x80 != 0 {
        return None;
    }
    Some(DateTime {
        second: bcd(r[0] & 0x7F),
        minute: bcd(r[1] & 0x7F),
        hour: bcd(r[2] & 0x3F),
        day: bcd(r[3] & 0x3F),
        // r[4] is the weekday
        month: bcd(r[5] & 0x1F),
        year: 2000 + bcd(r[6]) as u16,
    })
}

#[cfg(target_os = "espidf")]
pub fn read(i2c: &mut esp_idf_svc::hal::i2c::I2cDriver) -> anyhow::Result<Option<DateTime>> {
    let mut r = [0u8; 7];
    i2c.write_read(crate::board::ADDR_RTC, &[0x04], &mut r, esp_idf_svc::hal::delay::BLOCK)
        .map_err(|e| anyhow::anyhow!("rtc read: {e}"))?;
    Ok(decode(&r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_bcd() {
        let r = [0x59, 0x34, 0x23, 0x22, 0x03, 0x10, 0x26];
        assert_eq!(
            decode(&r),
            Some(DateTime { year: 2026, month: 10, day: 22, hour: 23, minute: 34, second: 59 })
        );
    }

    #[test]
    fn oscillator_stopped_is_invalid() {
        assert_eq!(decode(&[0x80, 0, 0, 1, 0, 1, 0]), None);
    }
}
