//! Clock widget: runs a clock face.

use super::{Step, Widget};
use crate::clock::{modern::ModernFace, Face, LocalTime};
use crate::gfx::{Canvas, Scene};

pub struct Clock {
    face: ModernFace,
}

impl Clock {
    /// `style` names a face; only "modern" exists so far.
    pub fn new(format_24h: bool, style: Option<&str>) -> Self {
        if let Some(s) = style.filter(|s| *s != "modern") {
            log::warn!("clock style {s:?} not available; using modern");
        }
        Self { face: ModernFace::new(format_24h) }
    }

    pub fn format_24h(&self) -> bool {
        self.face.format_24h()
    }
}

impl Widget for Clock {
    fn tick(&mut self, _now_ms: u32, local: &LocalTime) -> Step {
        Step::Redraw(self.face.update(local))
    }

    fn invalidate(&mut self) {
        self.face.invalidate();
    }
}

impl Scene for Clock {
    fn draw(&self, c: &mut Canvas) {
        self.face.draw(c);
    }
}
