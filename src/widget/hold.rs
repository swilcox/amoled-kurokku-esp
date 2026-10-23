//! Hold: keep whatever is on the panel (e.g. an image streamed straight to
//! it) until the next instruction. The panel retains its pixels, so there is
//! nothing to redraw — and nothing to redraw *with*.

use super::{Step, Widget};
use crate::clock::LocalTime;
use crate::gfx::{Canvas, Scene};

pub struct Hold;

impl Widget for Hold {
    fn tick(&mut self, _now_ms: u32, _local: &LocalTime) -> Step {
        Step::Redraw(vec![])
    }

    fn invalidate(&mut self) {}

    fn frame_ms(&self) -> u32 {
        100
    }
}

impl Scene for Hold {
    fn draw(&self, _c: &mut Canvas) {}
}
