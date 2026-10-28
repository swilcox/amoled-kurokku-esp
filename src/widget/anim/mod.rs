//! Animations (the server's `animation` widget). The names match the LED
//! firmware's, but each is redone for a 480×480 color screen; they run
//! until the next instruction.

pub mod curtain;
pub mod fishtank;
pub mod matrix;
pub mod pong;
pub mod sine;
pub mod snake;
pub mod snow;
pub mod starfield;

use super::Widget;
use crate::rng::Rng;

/// Animation names; `random` picks one of these.
pub const ANIMATIONS: [&str; 8] = ["static", "pong", "matrix", "snake", "curtain", "sine", "starfield", "fishtank"];

/// Build the animation called `name`: one of ANIMATIONS or "random".
/// Unknown names get a random one.
pub fn animation_for(name: &str, format_24h: bool) -> Box<dyn Widget> {
    let mut rng = Rng::from_entropy();
    let name = match name {
        n if ANIMATIONS.contains(&n) => n,
        n => {
            if n != "random" {
                log::warn!("animation {n:?} not available; picking one");
            }
            let pick = ANIMATIONS[rng.below(ANIMATIONS.len() as u32) as usize];
            log::info!("random animation: {pick}");
            pick
        }
    };
    match name {
        "snake" => Box::new(snake::Snake::new(rng)),
        "sine" => Box::new(sine::Sine::new()),
        "static" => Box::new(snow::Snow::new()),
        "curtain" => Box::new(curtain::Curtain::new()),
        "matrix" => Box::new(matrix::Matrix::new(rng)),
        "starfield" => Box::new(starfield::Starfield::new(rng)),
        "fishtank" => Box::new(fishtank::Fishtank::new(rng)),
        _ => Box::new(pong::Pong::new(format_24h, rng)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::Step;
    use super::*;
    use crate::clock::LocalTime;
    use crate::gfx::{MemPanel, Renderer, SCREEN};

    /// Tick `w` through `steps`, painting only what it reports, and check
    /// after every step that the panel matches a full repaint.
    pub fn check_incremental(mut w: Box<dyn Widget>, steps: impl IntoIterator<Item = (u32, LocalTime)>) {
        let (mut inc, mut full) = (MemPanel::default(), MemPanel::default());
        let mut r = Renderer::new(480 * 32);
        for (i, (ms, t)) in steps.into_iter().enumerate() {
            match w.tick(ms, &t) {
                Step::Redraw(rects) => r.render(&mut inc, &*w, &rects).unwrap(),
                Step::Done => panic!("animation ended"),
            }
            r.render(&mut full, &*w, &[SCREEN]).unwrap();
            assert!(inc.pixels == full.pixels, "step {i} at {ms} ms: incremental redraw differs");
        }
    }
}
