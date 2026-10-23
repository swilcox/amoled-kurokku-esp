//! The three case buttons: debounced press/release edges.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Key,
    Boot,
    Power,
}

pub const ALL: [Button; 3] = [Button::Key, Button::Boot, Button::Power];

const DEBOUNCE_MS: u32 = 30;

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

#[cfg(target_os = "espidf")]
mod device {
    use super::{Button, Debounced, ALL};
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
        state: [Debounced; 3],
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

        /// Buttons newly pressed since the last poll.
        pub fn poll(&mut self, now_ms: u32) -> impl Iterator<Item = Button> + '_ {
            ALL.into_iter().zip(self.state.iter_mut()).filter_map(move |(b, s)| {
                let level = unsafe { gpio_get_level(pin(b)) };
                // KEY and BOOT are active-low; PWR's sense line is active-high.
                let raw = if b == Button::Power { level == 1 } else { level == 0 };
                (s.update(raw, now_ms) == Some(true)).then_some(b)
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
}
