//! The three case buttons: debounced, and classified as short or long
//! presses.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Key,
    Boot,
    Power,
}

pub const ALL: [Button; 3] = [Button::Key, Button::Boot, Button::Power];

const DEBOUNCE_MS: u32 = 30;
/// Held this long, a press is long. Well under the PMIC's ~4 s hardware
/// power-off on PWR.
pub const LONG_MS: u32 = 800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Released before `LONG_MS`; reported on release.
    Short(Button),
    /// Reported once, as soon as the button has been held `LONG_MS`.
    Long(Button),
}

/// Debounces one raw input. Pure — host-testable.
#[derive(Default)]
pub struct Debounced {
    pressed: bool,
    raw: bool,
    since_ms: u32,
}

impl Debounced {
    /// Feed the raw "pressed" level. Returns `Some(pressed)` when the stable
    /// state changes.
    pub fn update(&mut self, raw: bool, now_ms: u32) -> Option<bool> {
        if raw != self.raw {
            self.raw = raw;
            self.since_ms = now_ms;
        }
        let settled = now_ms.wrapping_sub(self.since_ms) >= DEBOUNCE_MS;
        (settled && self.raw != self.pressed).then(|| {
            self.pressed = self.raw;
            self.pressed
        })
    }
}

/// Turns one button's debounced edges into presses. Pure — host-testable.
#[derive(Default)]
pub struct PressTracker {
    down_ms: Option<u32>,
    long_sent: bool,
}

impl PressTracker {
    /// Feed the debounced edge (if any) at `now_ms`. Call regularly while
    /// held, so a long press is reported without waiting for release.
    pub fn update(&mut self, b: Button, edge: Option<bool>, now_ms: u32) -> Option<Press> {
        match edge {
            Some(true) => {
                self.down_ms = Some(now_ms);
                self.long_sent = false;
                None
            }
            Some(false) => {
                let down = self.down_ms.take()?;
                (!self.long_sent && now_ms.wrapping_sub(down) < LONG_MS).then_some(Press::Short(b))
            }
            None => {
                let down = self.down_ms?;
                (!self.long_sent && now_ms.wrapping_sub(down) >= LONG_MS).then(|| {
                    self.long_sent = true;
                    Press::Long(b)
                })
            }
        }
    }
}

#[cfg(target_os = "espidf")]
mod device {
    use super::{Button, Debounced, Press, PressTracker, ALL};
    use crate::board::{BTN_BOOT, BTN_KEY, BTN_PWR_SENSE};
    use esp_idf_svc::sys::*;

    fn pin(b: Button) -> i32 {
        match b {
            Button::Key => BTN_KEY,
            Button::Boot => BTN_BOOT,
            Button::Power => BTN_PWR_SENSE,
        }
    }

    pub struct Buttons {
        state: [(Debounced, PressTracker); 3],
    }

    impl Buttons {
        pub fn new() -> Self {
            for b in ALL {
                unsafe {
                    gpio_reset_pin(pin(b));
                    gpio_set_direction(pin(b), gpio_mode_t_GPIO_MODE_INPUT);
                    // PWR's sense line is driven by the PMIC; KEY/BOOT need pull-ups.
                    let pull = if b == Button::Power {
                        gpio_pull_mode_t_GPIO_FLOATING
                    } else {
                        gpio_pull_mode_t_GPIO_PULLUP_ONLY
                    };
                    gpio_set_pull_mode(pin(b), pull);
                }
            }
            Self { state: Default::default() }
        }

        /// Presses completed (short) or reached (long) since the last poll.
        pub fn poll(&mut self, now_ms: u32) -> impl Iterator<Item = Press> + '_ {
            ALL.into_iter().zip(self.state.iter_mut()).filter_map(move |(b, (d, t))| {
                let level = unsafe { gpio_get_level(pin(b)) };
                // KEY and BOOT are active-low; PWR's sense line is active-high.
                let raw = if b == Button::Power { level == 1 } else { level == 0 };
                t.update(b, d.update(raw, now_ms), now_ms)
            })
        }
    }
}

#[cfg(target_os = "espidf")]
pub use device::Buttons;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_bounces_and_reports_edges() {
        let mut d = Debounced::default();
        assert_eq!(d.update(true, 0), None);
        assert_eq!(d.update(false, 5), None); // bounce
        assert_eq!(d.update(true, 10), None);
        assert_eq!(d.update(true, 39), None);
        assert_eq!(d.update(true, 40), Some(true));
        assert_eq!(d.update(true, 100), None);
        assert_eq!(d.update(false, 200), None);
        assert_eq!(d.update(false, 230), Some(false));
    }

    #[test]
    fn short_on_release_long_once_while_held() {
        let k = Button::Key;
        let mut t = PressTracker::default();
        assert_eq!(t.update(k, Some(true), 0), None);
        assert_eq!(t.update(k, None, 500), None);
        assert_eq!(t.update(k, Some(false), 600), Some(Press::Short(k)));

        assert_eq!(t.update(k, Some(true), 1000), None);
        assert_eq!(t.update(k, None, 1000 + LONG_MS), Some(Press::Long(k)));
        assert_eq!(t.update(k, None, 3000), None);
        assert_eq!(t.update(k, Some(false), 3100), None);
        // A release without a press (e.g. held at boot) is ignored.
        assert_eq!(t.update(k, Some(false), 4000), None);
    }
}
