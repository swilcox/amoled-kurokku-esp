//! AMOLED Kurokku — networked clock/sign for the Waveshare
//! ESP32-C6-Touch-AMOLED-2.16.
//!
//! Startup: hardware → time from RTC → banner → WiFi → NTP → engine. Every
//! step degrades gracefully: with no config or no network the clock still
//! runs from the RTC.
//!
//! Buttons: BOOT cycles brightness; KEY cycles the clock face (modern, LED,
//! flip, nixie) or dismisses anything else to the clock; PWR turns the panel
//! off/on.

#[cfg(target_os = "espidf")]
fn main() -> anyhow::Result<()> {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use esp_idf_svc::eventloop::EspSystemEventLoop;
    use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
    use esp_idf_svc::hal::peripherals::Peripherals;
    use esp_idf_svc::hal::units::*;
    use esp_idf_svc::nvs::EspDefaultNvsPartition;

    use amoled_kurokku_esp::co5300::Co5300;
    use amoled_kurokku_esp::engine::{Engine, Hardware};
    use amoled_kurokku_esp::poller::{HttpPoller, FIRMWARE_VERSION};
    use amoled_kurokku_esp::widget::status::Status;
    use amoled_kurokku_esp::gfx::{Renderer, Solid, SCREEN};
    use amoled_kurokku_esp::{assets, color, config, media, ota, pmic, rtc, udp_log, wifi};

    /// Attempts at the boot-time join. A failed join costs ~19 s, and the
    /// hopeless case (SSID absent) fails identically every time, so two.
    const JOIN_ATTEMPTS: u32 = 2;

    esp_idf_svc::sys::link_patches();
    udp_log::init();
    log::info!("amoled-kurokku-esp {}", FIRMWARE_VERSION.replace("%2B", "+"));

    let p = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs_partition = EspDefaultNvsPartition::take()?;

    let cfg = {
        let nvs = config::open_nvs(nvs_partition.clone())?;
        config::AppConfig::load(&nvs)
    };
    log::set_max_level(config::parse_level(&cfg.log_level));
    // Apply TZ before anything reads the clock, so an offline device still
    // shows local time.
    if let Err(e) = wifi::set_timezone(&cfg.tz) {
        log::warn!("timezone: {e}");
    }
    // Logged before any join: MAC-registration networks need it first.
    match wifi::sta_mac() {
        Ok(mac) => log::info!("station MAC: {mac}"),
        Err(e) => log::warn!("station MAC: {e}"),
    }
    match ota::running_partition_info() {
        Ok(info) => log::info!("firmware: {info}"),
        Err(e) => log::warn!("OTA info: {e}"),
    }

    let mut i2c = I2cDriver::new(
        p.i2c0,
        p.pins.gpio8,
        p.pins.gpio7,
        &I2cConfig::new().baudrate(400.kHz().into()),
    )?;
    pmic::init(&mut i2c)?;
    let mut lcd = Co5300::new(Engine::lcd_max_pixels())?;
    pmic::reset_display(&mut i2c)?;
    lcd.init()?;

    // Fonts and artwork come from an assets partition. Without the pack this
    // firmware was built with there is nothing to draw with: show red, and
    // if this is a fresh OTA app, let the bootloader roll back.
    if let Err(e) = assets::init() {
        log::error!("{e}");
        let mut r = Renderer::new(Engine::lcd_max_pixels());
        let _ = r.render(&mut lcd, &Solid(color::RED), &[SCREEN]);
        ota::rollback_if_unverified();
        loop {
            std::thread::sleep(Duration::from_secs(10));
            log::error!("halted: asset pack missing; flash assets.bin (just flash)");
        }
    }
    if let Err(e) = ota::mark_running_valid() {
        log::warn!("{e}");
    }
    // Optional: images for `sd:` URLs and slideshows.
    if let Err(e) = media::mount_sd() {
        log::info!("{e} (no card?)");
    }
    let utc = rtc::init_system_time(&mut i2c)?;
    log::info!("time from RTC: {utc:?} UTC");

    let mut engine = Engine::new(cfg.clone(), nvs_partition.clone(), Hardware { lcd, i2c })?;
    engine.show(Box::new(Status::new("KUROKKU", env!("CARGO_PKG_VERSION"), None)));

    if !cfg.has_wifi_config() {
        log::warn!("no WiFi config; running offline");
        engine.show(Box::new(Status::new("NO CONFIG", "run tools/provision.py", Some(5000))));
        engine.run(None, None);
    }

    let mut w = match wifi::init(&cfg.wifi_ssid, &cfg.wifi_password, p.modem, sysloop, nvs_partition) {
        Ok(w) => w,
        Err(e) => {
            log::error!("WiFi unavailable: {e}");
            engine.show(Box::new(Status::new("NO WIFI", &e.to_string(), Some(5000))));
            engine.run(None, None);
        }
    };

    engine.show(Box::new(Status::new("WIFI", &cfg.wifi_ssid, None)));
    let joined = (1..=JOIN_ATTEMPTS).any(|attempt| match wifi::join(&mut w) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("WiFi join attempt {attempt}/{JOIN_ATTEMPTS} failed: {e}");
            false
        }
    });
    if joined {
        let ip = wifi::get_ip(&w).unwrap_or_else(|| "?".into());
        if let Some(host) = &cfg.syslog_host {
            udp_log::enable_udp(host, &cfg.device_id);
        }
        engine.show(Box::new(Status::new("ONLINE", &ip, Some(3000))));
    } else {
        // Keep the driver: the network thread's reconnect path keeps trying.
        engine.show(Box::new(Status::new("NO WIFI", &cfg.wifi_ssid, Some(5000))));
    }

    // Start SNTP even without a link so a later reconnect still gets time.
    // No need to wait for the first sync: the RTC already has the time, and
    // the engine repaints (and updates the RTC) when NTP lands. The handle
    // must live for the whole program: dropping it stops periodic re-sync.
    let _sntp = wifi::sync_ntp(cfg.ntp_server.as_deref(), Duration::ZERO)
        .map_err(|e| log::warn!("NTP: {e}"))
        .ok();

    let poller = HttpPoller::new(&cfg.server_url, &cfg.device_id, Duration::from_millis(cfg.poll_interval_ms));
    log::info!("starting engine");
    engine.run(Some(poller), Some(Arc::new(Mutex::new(w))));
}

#[cfg(not(target_os = "espidf"))]
fn main() {}
