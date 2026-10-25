//! Quiet hours: the server says when the screen should be off
//! (`display_off`); a button wakes it for a while. Pure — host-testable.

/// A wake during quiet hours lasts this long after the last input.
pub const WAKE_MS: u32 = 60_000;

#[derive(Default)]
pub struct Quiet {
    active: bool,
    wake_until: Option<u32>,
}

impl Quiet {
    /// The server's quiet state changed. Returns whether the panel should
    /// now be on.
    pub fn set(&mut self, active: bool) -> bool {
        self.active = active;
        self.wake_until = None;
        !active
    }

    /// Inside quiet hours (even while a button has woken the screen).
    pub fn active(&self) -> bool {
        self.active
    }

    /// Input at `now` (a press or a gesture): during quiet hours, keeps the
    /// screen awake for another `WAKE_MS`.
    pub fn input(&mut self, now: u32) {
        if self.active {
            self.wake_until = Some(now.wrapping_add(WAKE_MS));
        }
    }

    /// True once, when a wake has run out and the panel should go off.
    pub fn expired(&mut self, now: u32) -> bool {
        match self.wake_until {
            Some(t) if now.wrapping_sub(t) as i32 >= 0 => {
                self.wake_until = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_lasts_from_the_last_input() {
        let mut q = Quiet::default();
        assert!(!q.set(true));
        q.input(1000);
        q.input(1000 + WAKE_MS - 1);
        assert!(!q.expired(1000 + WAKE_MS));
        assert!(q.expired(1000 + 2 * WAKE_MS));
        assert!(!q.expired(1000 + 3 * WAKE_MS));
    }

    #[test]
    fn outside_quiet_hours_input_never_expires() {
        let mut q = Quiet::default();
        assert!(q.set(false));
        q.input(0);
        assert!(!q.expired(WAKE_MS * 2));
    }

    #[test]
    fn quiet_ending_cancels_the_wake() {
        let mut q = Quiet::default();
        q.set(true);
        q.input(0);
        assert!(q.set(false));
        assert!(!q.expired(WAKE_MS));
    }
}
