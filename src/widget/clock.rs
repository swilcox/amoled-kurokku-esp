//! Clock widget: runs a clock face, and nudges it around a small orbit for
//! burn-in protection (OLED pixels that show the same thing for months age
//! unevenly; shifting a few pixels spreads the wear).

use super::{Step, Widget};
use crate::clock::{face_for, Face, LocalTime, STYLES};
use crate::gfx::{Canvas, Scene};

/// Offsets cycled through, one step per `ORBIT_MINUTES`.
const ORBIT: [(i32, i32); 8] = [(0, 0), (3, 0), (3, 3), (0, 3), (-3, 3), (-3, 0), (-3, -3), (0, -3)];
const ORBIT_MINUTES: u32 = 2;

pub struct Clock {
    face: Box<dyn Face>,
    format_24h: bool,
}

impl Clock {
    /// `style` names a face (see `clock::STYLES`); unknown names fall back
    /// to "modern".
    pub fn new(format_24h: bool, style: Option<&str>) -> Self {
        if let Some(s) = style.filter(|s| !STYLES.contains(s)) {
            log::warn!("clock style {s:?} not available; using modern");
        }
        Self { face: face_for(style, format_24h), format_24h }
    }

    pub fn format_24h(&self) -> bool {
        self.format_24h
    }
}

/// Orbit position for a time of day. Steps at second 30, so the full
/// repaint a shift causes never coincides with a minute rollover (and its
/// flip/fade animation).
fn orbit(t: &LocalTime) -> (i32, i32) {
    let secs = t.hour as u32 * 3600 + t.minute as u32 * 60 + t.second as u32 + 30;
    ORBIT[(secs / (ORBIT_MINUTES * 60)) as usize % ORBIT.len()]
}

impl Widget for Clock {
    fn tick(&mut self, now_ms: u32, local: &LocalTime) -> Step {
        let (dx, dy) = orbit(local);
        self.face.set_offset(dx, dy);
        Step::Redraw(self.face.update(now_ms, local))
    }

    fn invalidate(&mut self) {
        self.face.invalidate();
    }

    fn frame_ms(&self) -> u32 {
        if self.face.animating() {
            1
        } else {
            50
        }
    }
}

impl Scene for Clock {
    fn draw(&self, c: &mut Canvas) {
        self.face.draw(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_moves_every_two_minutes_off_the_minute_boundary() {
        let t = |minute, second| LocalTime { hour: 7, minute, second, ..Default::default() };
        // Never changes across a minute rollover...
        for m in 0..59 {
            assert_eq!(orbit(&t(m, 59)), orbit(&t(m + 1, 0)), "minute {m}");
        }
        // ...but does step every two minutes, at :30.
        let steps = (0..59).filter(|&m| orbit(&t(m, 29)) != orbit(&t(m, 30))).count();
        assert!((29..=30).contains(&steps));
        assert!(ORBIT.iter().all(|(x, y)| x.abs() <= 3 && y.abs() <= 3));
    }
}
