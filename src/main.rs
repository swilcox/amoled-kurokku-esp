//! AMOLED Kurokku — Phase 0 hardware bring-up.
//!
//! Proves each piece of hardware from Rust: I2C bus scan, PMIC rails,
//! display init + test pattern + brightness sweep, RTC read, touch (draws a
//! dot where you touch), and a button probe that logs level changes on
//! BOOT and the KEY candidate GPIOs plus PWR presses via the PMIC.

mod color;
mod rtc;
mod touch;

#[cfg(target_os = "espidf")]
mod board;
#[cfg(target_os = "espidf")]
mod co5300;
#[cfg(target_os = "espidf")]
mod pmic;

#[cfg(target_os = "espidf")]
fn main() -> anyhow::Result<()> {
    use esp_idf_svc::hal::delay::FreeRtos;
    use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
    use esp_idf_svc::hal::peripherals::Peripherals;
    use esp_idf_svc::hal::units::*;
    use esp_idf_svc::sys::*;

    use board::*;
    use color::*;

    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::info!("amoled-kurokku-esp {} — phase 0 bring-up", env!("CARGO_PKG_VERSION"));
    log::info!("free heap: {} bytes", unsafe { esp_get_free_heap_size() });

    let p = Peripherals::take()?;
    let mut i2c = I2cDriver::new(
        p.i2c0,
        p.pins.gpio8,
        p.pins.gpio7,
        &I2cConfig::new().baudrate(400.kHz().into()),
    )?;

    let found: Vec<String> = (0x08u8..0x78)
        .filter(|&a| i2c.read(a, &mut [0u8], 10).is_ok())
        .map(|a| format!("0x{a:02x}"))
        .collect();
    log::info!("i2c devices: {}", found.join(" "));

    pmic::init(&mut i2c)?;

    let mut lcd = co5300::Co5300::new()?;
    pmic::reset_display(&mut i2c)?;
    lcd.init()?;
    log::info!("display initialized");

    // Quadrants plus a yellow marker in the top-left corner, to check
    // color order and orientation at a glance.
    let (w, h) = (LCD_W - 1, LCD_H - 1);
    let (mx, my) = (LCD_W / 2, LCD_H / 2);
    lcd.fill_rect(0, 0, mx - 1, my - 1, RED)?;
    lcd.fill_rect(mx, 0, w, my - 1, GREEN)?;
    lcd.fill_rect(0, my, mx - 1, h, BLUE)?;
    lcd.fill_rect(mx, my, w, h, WHITE)?;
    lcd.fill_rect(0, 0, 47, 47, YELLOW)?;

    let t = std::time::Instant::now();
    lcd.fill_screen(BLACK)?;
    let full_frame = t.elapsed();
    lcd.fill_rect(0, 0, mx - 1, my - 1, RED)?;
    lcd.fill_rect(mx, 0, w, my - 1, GREEN)?;
    lcd.fill_rect(0, my, mx - 1, h, BLUE)?;
    lcd.fill_rect(mx, my, w, h, WHITE)?;
    lcd.fill_rect(0, 0, 47, 47, YELLOW)?;
    log::info!("full-screen fill: {} ms", full_frame.as_millis());

    for level in (16..=255u16).rev().step_by(16).chain((16..=255).step_by(16)) {
        lcd.set_brightness(level as u8)?;
        FreeRtos::delay_ms(60);
    }
    lcd.set_brightness(255)?;

    match rtc::read(&mut i2c) {
        Ok(Some(dt)) => log::info!("rtc: {dt:?}"),
        Ok(None) => log::warn!("rtc: oscillator stopped — time not set"),
        Err(e) => log::error!("{e}"),
    }

    touch::init()?;

    // Button probe: inputs with pull-ups; log every level change.
    let mut probe: Vec<(i32, i32)> = std::iter::once(BTN_BOOT)
        .chain([BTN_KEY, BTN_PWR_SENSE])
        .map(|pin| unsafe {
            gpio_reset_pin(pin);
            gpio_set_direction(pin, gpio_mode_t_GPIO_MODE_INPUT);
            gpio_set_pull_mode(pin, gpio_pull_mode_t_GPIO_PULLUP_ONLY);
            (pin, gpio_get_level(pin))
        })
        .collect();
    for (pin, level) in &probe {
        log::info!("probe GPIO{pin}: idle level {level}");
    }

    let marker = |pin: i32| match pin {
        BTN_BOOT => CYAN,
        BTN_KEY => MAGENTA,
        BTN_PWR_SENSE => ORANGE,
        _ => YELLOW,
    };

    log::info!("ready: press buttons / touch the screen");
    let mut tick: u32 = 0;
    let mut touch_int_low = false;
    loop {
        for (pin, last) in probe.iter_mut() {
            let level = unsafe { gpio_get_level(*pin) };
            if level != *last {
                log::info!("button GPIO{pin}: {last} -> {level}");
                *last = level;
                let c = if level == 0 { marker(*pin) } else { BLACK };
                lcd.fill_rect(mx - 40, my - 40, mx + 39, my + 39, c)?;
            }
        }

        if tick % 5 == 0 {
            match pmic::take_power_key(&mut i2c) {
                Ok(k) if k.short || k.long => {
                    log::info!("button PWR: {k:?}");
                    lcd.fill_rect(mx - 40, my - 40, mx + 39, my + 39, if k.long { RED } else { GREEN })?;
                }
                Ok(_) => {}
                Err(e) => log::warn!("{e}"),
            }
        }

        // Poll over I2C every 20 ms as well as on INT, in case INT only
        // pulses briefly; log INT edges to learn which one sees touches.
        let int_low = touch::pending();
        if int_low != touch_int_low {
            log::info!("touch INT: {}", if int_low { "low" } else { "high" });
            touch_int_low = int_low;
        }
        if int_low || tick % 2 == 0 {
            match touch::read(&mut i2c) {
                Ok(Some((x, y))) => {
                    log::info!("touch: ({x}, {y})");
                    let (x0, y0) = ((x & !1).min(LCD_W - 8), (y & !1).min(LCD_H - 8));
                    lcd.fill_rect(x0, y0, x0 + 7, y0 + 7, WHITE)?;
                }
                Ok(None) => {}
                Err(e) => log::warn!("{e}"),
            }
        }

        if tick % 1000 == 0 {
            match rtc::read(&mut i2c) {
                Ok(dt) => log::info!("rtc: {dt:?}"),
                Err(e) => log::warn!("{e}"),
            }
        }

        tick = tick.wrapping_add(1);
        FreeRtos::delay_ms(10);
    }
}

#[cfg(not(target_os = "espidf"))]
fn main() {}
