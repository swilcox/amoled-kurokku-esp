//! Clock faces. Each face is a `Scene` that also tracks what it last showed,
//! so it can report only the regions that changed.

pub mod modern;

use crate::gfx::{Rect, Scene};

/// Broken-down local time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1-12
    pub month: u8,
    /// 1-31
    pub day: u8,
    /// 0 = Sunday
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub const WEEKDAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
pub const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// Hour for display plus whether it is PM (always false in 24h mode).
pub fn display_hour(hour: u8, format_24h: bool) -> (u8, bool) {
    if format_24h {
        (hour, false)
    } else {
        let h = hour % 12;
        (if h == 0 { 12 } else { h }, hour >= 12)
    }
}

pub trait Face: Scene {
    /// Record `t` as the time to show and return the regions that changed
    /// since the last call (the whole screen on the first call, or after
    /// `invalidate`).
    fn update(&mut self, t: &LocalTime) -> Vec<Rect>;

    /// Force a full redraw on the next `update`.
    fn invalidate(&mut self);
}

/// Current local time per the `TZ` environment variable.
#[cfg(target_os = "espidf")]
pub fn now_local() -> LocalTime {
    use esp_idf_svc::sys::{localtime_r, time, tm};
    let mut tm: tm = Default::default();
    unsafe {
        let t = time(core::ptr::null_mut());
        localtime_r(&t, &mut tm);
    }
    LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u8,
        day: tm.tm_mday as u8,
        weekday: tm.tm_wday as u8,
        hour: tm.tm_hour as u8,
        minute: tm.tm_min as u8,
        second: tm.tm_sec as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_hour_conversion() {
        assert_eq!(display_hour(0, false), (12, false));
        assert_eq!(display_hour(11, false), (11, false));
        assert_eq!(display_hour(12, false), (12, true));
        assert_eq!(display_hour(23, false), (11, true));
        assert_eq!(display_hour(23, true), (23, false));
    }
}
