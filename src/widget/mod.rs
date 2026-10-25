//! Widgets: what the screen is showing. Pure — host-testable and
//! previewable.
//!
//! A widget is a `Scene` that is ticked by the engine. Each tick reports the
//! regions that need repainting (possibly none) or that the widget is done,
//! in which case the engine falls back to the clock.

pub mod clock;
pub mod hold;
pub mod info;
pub mod message;
pub mod splash;
pub mod status;

use crate::clock::LocalTime;
use crate::gfx::{Rect, Scene};

pub enum Step {
    Redraw(Vec<Rect>),
    Done,
}

pub trait Widget: Scene + Send {
    /// Advance to `now_ms` (a monotonic millisecond clock) / `local` time.
    fn tick(&mut self, now_ms: u32, local: &LocalTime) -> Step;

    /// Force a full repaint on the next tick (e.g. after a timezone change).
    fn invalidate(&mut self);

    /// Preferred time from the start of one tick to the next.
    fn frame_ms(&self) -> u32 {
        50
    }
}
