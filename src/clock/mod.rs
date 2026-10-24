//! Clock faces. Each face is a `Scene` that also tracks what it last showed,
//! so it can report only the regions that changed.

pub mod analog;
pub mod flip;
pub mod led;
pub mod modern;
pub mod nixie;

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

pub trait Face: Scene + Send {
    /// Record `t` as the time to show and return the regions that changed
    /// since the last call (the whole screen on the first call, or after
    /// `invalidate`). `now_ms` is a monotonic clock for animations.
    fn update(&mut self, now_ms: u32, t: &LocalTime) -> Vec<Rect>;

    /// Force a full redraw on the next `update`.
    fn invalidate(&mut self);

    /// Burn-in protection: shift everything by (dx, dy) px. Implementations
    /// force a full redraw when the offset changes.
    fn set_offset(&mut self, dx: i32, dy: i32);

    /// True while an animation is running (the engine then ticks fast).
    fn animating(&self) -> bool {
        false
    }

    /// How soon to call `update` again.
    fn frame_ms(&self) -> u32 {
        if self.animating() {
            1
        } else {
            50
        }
    }
}

/// Face names accepted by `face_for`.
pub const STYLES: [&str; 7] = ["modern", "led", "flip", "nixie", "railway", "railway-black", "classic"];

/// Build the face for a style name; unknown names give the modern face.
pub fn face_for(style: Option<&str>, format_24h: bool) -> Box<dyn Face> {
    match style.unwrap_or("modern") {
        "led" => Box::new(led::LedFace::new(format_24h)),
        "flip" => Box::new(flip::FlipFace::new(format_24h)),
        "nixie" => Box::new(nixie::NixieFace::new(format_24h)),
        // Analog faces ignore 12/24h.
        "railway" => Box::new(analog::AnalogFace::new(&analog::RAILWAY)),
        "railway-black" => Box::new(analog::AnalogFace::new(&analog::RAILWAY_DARK)),
        "classic" => Box::new(analog::AnalogFace::new(&analog::CLASSIC_STYLE)),
        _ => Box::new(modern::ModernFace::new(format_24h)),
    }
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
pub(crate) mod tests {
    use super::*;
    use crate::gfx::{MemPanel, Renderer, SCREEN};

    fn render_all(face: &mut dyn Face, steps: &[(u32, LocalTime)]) -> Vec<u16> {
        let mut panel = MemPanel::default();
        let mut r = Renderer::new(480 * 32);
        for (ms, t) in steps {
            let rects = face.update(*ms, t);
            r.render(&mut panel, face, &rects).unwrap();
        }
        panel.pixels
    }

    /// Rendering `from` in full and then only the dirty regions for `to`
    /// (at a later time, after any animation has finished) must give exactly
    /// the same pixels as rendering `to` in full. Also checks the same with a
    /// burn-in offset applied in between.
    pub fn check_incremental(make: impl Fn() -> Box<dyn Face>, from: LocalTime, to: LocalTime) {
        let settle = 60_000;
        let full = {
            let mut f = make();
            render_all(&mut *f, &[(settle, to)])
        };
        let mut f = make();
        let steps: Vec<(u32, LocalTime)> =
            [(0, from), (1000, to)].into_iter().chain((1..=60).map(|i| (1000 + i * 50, to))).collect();
        assert!(render_all(&mut *f, &steps) == full, "{from:?} -> {to:?}");

        // A burn-in shift forces a full repaint that matches a fresh render.
        let mut f = make();
        f.update(0, &from);
        f.set_offset(3, -3);
        assert_eq!(f.update(settle, &to), vec![SCREEN]);
    }

    /// After every step, the incrementally rendered screen must equal a full
    /// render of the face's current state: the dirty rects missed nothing.
    pub fn check_frames(make: impl Fn() -> Box<dyn Face>, steps: &[(u32, LocalTime)]) {
        let mut face = make();
        let mut panel = MemPanel::default();
        let mut r = Renderer::new(480 * 32);
        for (ms, t) in steps {
            let rects = face.update(*ms, t);
            r.render(&mut panel, &*face, &rects).unwrap();
            let mut full = MemPanel::default();
            r.render(&mut full, &*face, &[SCREEN]).unwrap();
            assert!(panel.pixels == full.pixels, "stale pixels at {ms} ms, {t:?}");
        }
    }

    #[test]
    fn twelve_hour_conversion() {
        assert_eq!(display_hour(0, false), (12, false));
        assert_eq!(display_hour(11, false), (11, false));
        assert_eq!(display_hour(12, false), (12, true));
        assert_eq!(display_hour(23, false), (11, true));
        assert_eq!(display_hour(23, true), (23, false));
    }
}
