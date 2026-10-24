//! `random`: shows one of the other faces, picked at random, and switches to
//! a different one every quarter hour. Variety, and no single layout stays
//! on the panel long enough to burn in.

use super::{face_for, Face, LocalTime, FACES};
use crate::gfx::{Canvas, Rect, Scene, SCREEN};

/// Minutes between switches; switches land on multiples of this.
const SWITCH_MINUTES: u8 = 15;

pub struct RandomFace {
    face: Box<dyn Face>,
    /// Index into FACES of the face showing.
    current: usize,
    format_24h: bool,
    /// xorshift32 state.
    rng: u32,
    /// (hour, quarter) the current face belongs to; None until first update.
    slot: Option<(u8, u8)>,
    off: (i32, i32),
}

impl RandomFace {
    pub fn new(format_24h: bool, seed: u32) -> Self {
        let mut rng = seed | 1;
        let current = (xorshift(&mut rng) as usize) % FACES.len();
        Self { face: face_for(Some(FACES[current]), format_24h), current, format_24h, rng, slot: None, off: (0, 0) }
    }

    /// The face showing, by name.
    pub fn current(&self) -> &'static str {
        FACES[self.current]
    }

    /// Switch to a different face, chosen at random.
    fn switch(&mut self) {
        let step = 1 + xorshift(&mut self.rng) as usize % (FACES.len() - 1);
        self.current = (self.current + step) % FACES.len();
        self.face = face_for(Some(FACES[self.current]), self.format_24h);
        let (dx, dy) = self.off;
        self.face.set_offset(dx, dy);
        log::info!("random clock face: {}", self.current());
    }
}

fn xorshift(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// A seed that differs from boot to boot (and run to run on the host).
pub fn seed() -> u32 {
    #[cfg(target_os = "espidf")]
    {
        unsafe { esp_idf_svc::sys::esp_random() }
    }
    #[cfg(not(target_os = "espidf"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos())
    }
}

impl Face for RandomFace {
    fn update(&mut self, now_ms: u32, t: &LocalTime) -> Vec<Rect> {
        let slot = (t.hour, t.minute / SWITCH_MINUTES);
        match self.slot.replace(slot) {
            Some(prev) if prev != slot => {
                self.switch();
                // The new face's first update repaints everything.
                self.face.update(now_ms, t);
                vec![SCREEN]
            }
            _ => self.face.update(now_ms, t),
        }
    }

    fn invalidate(&mut self) {
        self.face.invalidate();
    }

    fn set_offset(&mut self, dx: i32, dy: i32) {
        self.off = (dx, dy);
        self.face.set_offset(dx, dy);
    }

    fn animating(&self) -> bool {
        self.face.animating()
    }

    fn frame_ms(&self) -> u32 {
        self.face.frame_ms()
    }
}

impl Scene for RandomFace {
    fn draw(&self, c: &mut Canvas) {
        self.face.draw(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u8, minute: u8, second: u8) -> LocalTime {
        LocalTime { year: 2026, month: 10, day: 24, hour, minute, second, ..Default::default() }
    }

    #[test]
    fn switches_to_a_different_face_each_quarter_hour() {
        let mut f = RandomFace::new(false, 12345);
        f.update(0, &at(9, 0, 0));
        let mut shown = vec![f.current()];
        // Within the quarter: same face, no full repaint forced.
        for s in 1..60 {
            f.update(s * 1000, &at(9, 14, s as u8));
            assert_eq!(f.current(), shown[0]);
        }
        for (k, m) in [15u8, 30, 45].into_iter().enumerate() {
            let rects = f.update(100_000 + k as u32, &at(9, m, 0));
            assert_eq!(rects, vec![SCREEN]);
            assert_ne!(f.current(), *shown.last().unwrap());
            shown.push(f.current());
        }
    }

    #[test]
    fn seeds_spread_over_every_face() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..200 {
            seen.insert(RandomFace::new(true, seed * 7919).current());
        }
        assert_eq!(seen.len(), FACES.len());
    }
}
