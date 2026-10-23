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

fn to_bcd(v: u8) -> u8 {
    (v / 10) << 4 | (v % 10)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: u8, d: u8) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of `days_from_civil`.
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

impl DateTime {
    /// Seconds since the Unix epoch, treating this as UTC.
    pub fn to_unix(&self) -> i64 {
        days_from_civil(self.year as i64, self.month, self.day) * 86_400
            + self.hour as i64 * 3600
            + self.minute as i64 * 60
            + self.second as i64
    }

    pub fn from_unix(t: i64) -> Self {
        let (y, month, day) = civil_from_days(t.div_euclid(86_400));
        let secs = t.rem_euclid(86_400);
        Self {
            year: y as u16,
            month,
            day,
            hour: (secs / 3600) as u8,
            minute: (secs / 60 % 60) as u8,
            second: (secs % 60) as u8,
        }
    }

    /// 0 = Sunday.
    pub fn weekday(&self) -> u8 {
        // 1970-01-01 was a Thursday.
        (days_from_civil(self.year as i64, self.month, self.day) + 4).rem_euclid(7) as u8
    }

    /// Whether this looks like a real time, given when the firmware was
    /// built: not before the build (the RTC was never set) and not more than
    /// ~10 years after it (set with another year convention — the vendor demo
    /// stores years from 1970, which reads back as 2056 here).
    pub fn plausible(&self, built_unix: i64) -> bool {
        const TEN_YEARS: i64 = 10 * 365 * 86_400;
        let t = self.to_unix();
        t >= built_unix - 86_400 && t <= built_unix + TEN_YEARS
    }
}

/// Encode registers 0x04..=0x0A. Clears the oscillator-stopped flag.
pub fn encode(dt: &DateTime) -> [u8; 7] {
    [
        to_bcd(dt.second),
        to_bcd(dt.minute),
        to_bcd(dt.hour),
        to_bcd(dt.day),
        dt.weekday(),
        to_bcd(dt.month),
        to_bcd((dt.year - 2000) as u8),
    ]
}

#[cfg(target_os = "espidf")]
pub fn write(i2c: &mut esp_idf_svc::hal::i2c::I2cDriver, dt: &DateTime) -> anyhow::Result<()> {
    let r = encode(dt);
    let mut buf = [0u8; 8];
    buf[0] = 0x04;
    buf[1..].copy_from_slice(&r);
    i2c.write(crate::board::ADDR_RTC, &buf, esp_idf_svc::hal::delay::BLOCK)
        .map_err(|e| anyhow::anyhow!("rtc write: {e}"))
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
    fn unix_round_trip() {
        let dt = DateTime { year: 2026, month: 10, day: 22, hour: 23, minute: 34, second: 59 };
        assert_eq!(dt.to_unix(), 1_792_712_099);
        assert_eq!(DateTime::from_unix(dt.to_unix()), dt);
        assert_eq!(DateTime::from_unix(0).year, 1970);
        let leap = DateTime { year: 2028, month: 2, day: 29, hour: 0, minute: 0, second: 0 };
        assert_eq!(DateTime::from_unix(leap.to_unix()), leap);
    }

    #[test]
    fn weekday_and_encode() {
        let dt = DateTime { year: 2026, month: 10, day: 22, hour: 23, minute: 34, second: 59 };
        assert_eq!(dt.weekday(), 4); // Thursday
        assert_eq!(encode(&dt), [0x59, 0x34, 0x23, 0x22, 0x04, 0x10, 0x26]);
        assert_eq!(decode(&encode(&dt)), Some(dt));
    }

    #[test]
    fn plausibility_is_relative_to_build_time() {
        let built = DateTime { year: 2026, month: 10, day: 22, hour: 0, minute: 0, second: 0 }.to_unix();
        let at = |year| DateTime { year, month: 10, day: 23, hour: 0, minute: 0, second: 0 };
        assert!(at(2026).plausible(built));
        assert!(at(2035).plausible(built));
        assert!(!at(2025).plausible(built));
        assert!(!at(2056).plausible(built)); // vendor demo's 1970-based year
    }

    #[test]
    fn oscillator_stopped_is_invalid() {
        assert_eq!(decode(&[0x80, 0, 0, 1, 0, 1, 0]), None);
    }
}
