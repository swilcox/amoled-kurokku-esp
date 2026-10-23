//! AMOLED Kurokku — Phase 1: offline clock.
//!
//! Boots straight to the "modern" clock face using the PCF85063 RTC for time
//! (no WiFi yet). If the RTC has never been set, it is seeded from the
//! firmware build time until NTP arrives in Phase 2.
//!
//! Buttons: BOOT cycles brightness, KEY toggles 12/24h, PWR turns the panel
//! off/on.

#[cfg(target_os = "espidf")]
fn main() -> anyhow::Result<()> {
    use std::time::Instant;

    use esp_idf_svc::hal::delay::FreeRtos;
    use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
    use esp_idf_svc::hal::peripherals::Peripherals;
    use esp_idf_svc::hal::units::*;
    use esp_idf_svc::sys::*;

    use amoled_kurokku_esp::board::LCD_W;
    use amoled_kurokku_esp::buttons::{Button, Buttons};
    use amoled_kurokku_esp::clock::{self, modern::ModernFace, Face};
    use amoled_kurokku_esp::co5300::Co5300;
    use amoled_kurokku_esp::gfx::Renderer;
    use amoled_kurokku_esp::{pmic, rtc};

    /// Until NVS config exists (Phase 2): US Central, like most kurokku devices.
    const DEFAULT_TZ: &str = "CST6CDT,M3.2.0,M11.1.0";
    const BAND_PIXELS: usize = LCD_W as usize * 32;
    const BRIGHTNESS: [u8; 5] = [255, 160, 96, 48, 16];

    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::info!("amoled-kurokku-esp {}", env!("CARGO_PKG_VERSION"));

    let p = Peripherals::take()?;
    let mut i2c = I2cDriver::new(
        p.i2c0,
        p.pins.gpio8,
        p.pins.gpio7,
        &I2cConfig::new().baudrate(400.kHz().into()),
    )?;
    pmic::init(&mut i2c)?;

    let mut lcd = Co5300::new(BAND_PIXELS)?;
    pmic::reset_display(&mut i2c)?;
    lcd.init()?;

    // System time from the RTC (kept in UTC); seed the RTC if it was never set.
    let built: i64 = env!("KUROKKU_BUILD_UNIX_TIME").parse().unwrap_or(0);
    let utc = match rtc::read(&mut i2c) {
        Ok(Some(dt)) if dt.plausible(built) => dt,
        other => {
            let dt = rtc::DateTime::from_unix(built);
            log::warn!("rtc unset ({other:?}); seeding with build time {dt:?}");
            rtc::write(&mut i2c, &dt)?;
            dt
        }
    };
    let tv = timeval { tv_sec: utc.to_unix() as _, tv_usec: 0 };
    unsafe { settimeofday(&tv, core::ptr::null()) };
    std::env::set_var("TZ", DEFAULT_TZ);
    unsafe { tzset() };
    log::info!("time: {utc:?} UTC, TZ={DEFAULT_TZ}");

    let mut renderer = Renderer::new(BAND_PIXELS);
    let mut face = ModernFace::new(true);
    let mut buttons = Buttons::new();
    let mut brightness = 0;
    let mut panel_on = true;
    lcd.set_brightness(BRIGHTNESS[brightness])?;
    log::info!("free heap: {} bytes", unsafe { esp_get_free_heap_size() });

    let boot = Instant::now();
    let mut last = None;
    loop {
        let now_ms = boot.elapsed().as_millis() as u32;
        for b in buttons.poll(now_ms) {
            log::info!("button: {b:?}");
            match b {
                Button::Boot => {
                    brightness = (brightness + 1) % BRIGHTNESS.len();
                    lcd.set_brightness(BRIGHTNESS[brightness])?;
                }
                Button::Key => {
                    face.set_format_24h(!face.format_24h());
                    last = None;
                }
                Button::Power => {
                    panel_on = !panel_on;
                    lcd.set_on(panel_on)?;
                }
            }
        }

        let t = clock::now_local();
        if last != Some(t) {
            let started = Instant::now();
            let rects = face.update(&t);
            renderer.render(&mut lcd, &face, &rects)?;
            if t.second == 0 || last.is_none() {
                log::info!("{t:?}: {} region(s) in {} ms", rects.len(), started.elapsed().as_millis());
            }
            last = Some(t);
        }
        FreeRtos::delay_ms(20);
    }
}

#[cfg(not(target_os = "espidf"))]
fn main() {}
