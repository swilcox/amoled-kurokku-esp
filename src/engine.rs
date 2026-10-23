//! Two-thread engine, modeled on led-kurokku-esp.
//!
//! - **Network thread**: polls the server, deduplicates, and hands changes
//!   (instruction, brightness, config) to the display thread via `Shared`.
//! - **Display thread** (the caller of `run`): owns the panel and I2C bus;
//!   runs the active widget, applies changes, handles buttons, performs OTA,
//!   and writes NTP time back to the RTC.
//!
//! When the server is unreachable or a widget finishes, the clock shows.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::i2c::I2cDriver;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};

use crate::board::LCD_W;
use crate::buttons::{Button, Buttons};
use crate::clock;
use crate::co5300::Co5300;
use crate::color::WHITE;
use crate::config::{self, AppConfig};
use crate::gfx::Renderer;
use crate::poller::HttpPoller;
use crate::protocol::{panel_brightness, parse_color, scroll_px_per_ms, ConfigUpdate, Instruction};
use crate::widget::message::{font_for, Direction, Message};
use crate::widget::{clock::Clock, status::Status, Step, Widget};
use crate::{rtc, udp_log, wifi};

pub type SharedWifi = Arc<Mutex<BlockingWifi<EspWifi<'static>>>>;

const BAND_PIXELS: usize = LCD_W as usize * 32;
/// Brightness levels (server 0-15 scale) the BOOT button cycles through.
const LOCAL_BRIGHTNESS: [u8; 5] = [15, 10, 6, 3, 0];
/// Reboot after this many consecutive failed polls (~5 min at 5 s).
const MAX_CONSECUTIVE_FAILURES: u32 = 60;
const TELEMETRY_INTERVAL: Duration = Duration::from_secs(60);

/// Changes from the network thread, taken by the display thread.
#[derive(Default)]
struct Shared {
    instruction: Option<Instruction>,
    brightness: Option<u8>,
    config: Option<ConfigUpdate>,
}

pub struct Hardware {
    pub lcd: Co5300,
    pub i2c: I2cDriver<'static>,
}

pub struct Engine {
    cfg: AppConfig,
    nvs: EspDefaultNvsPartition,
    hw: Hardware,
    buttons: Buttons,
    renderer: Renderer,
    widget: Box<dyn Widget>,
    is_clock: bool,
    format_24h: bool,
    /// Current brightness on the server's 0-15 scale.
    brightness: u8,
    panel_on: bool,
    boot: Instant,
    prof: Profile,
}

#[derive(Default)]
struct Profile {
    since_ms: u32,
    frames: u32,
    total_us: u32,
    pixels: u32,
}

impl Engine {
    pub fn new(cfg: AppConfig, nvs: EspDefaultNvsPartition, hw: Hardware) -> anyhow::Result<Self> {
        let format_24h = cfg.format_24h;
        let brightness = cfg.default_brightness.min(15);
        hw.lcd.set_brightness(panel_brightness(brightness))?;
        Ok(Self {
            cfg,
            nvs,
            hw,
            buttons: Buttons::new(),
            renderer: Renderer::new(BAND_PIXELS),
            widget: Box::new(Clock::new(format_24h, None)),
            is_clock: true,
            format_24h,
            brightness,
            panel_on: true,
            boot: Instant::now(),
            prof: Profile::default(),
        })
    }

    pub fn lcd_max_pixels() -> usize {
        BAND_PIXELS
    }

    fn now_ms(&self) -> u32 {
        self.boot.elapsed().as_millis() as u32
    }

    /// Replace the active widget and paint it immediately. Used during boot
    /// (before `run`) for status screens.
    pub fn show(&mut self, widget: Box<dyn Widget>) {
        self.widget = widget;
        self.is_clock = false;
        self.step();
    }

    fn show_clock(&mut self) {
        self.widget = Box::new(Clock::new(self.format_24h, None));
        self.is_clock = true;
    }

    /// Tick the active widget once and paint what changed.
    fn step(&mut self) {
        let now = self.now_ms();
        match self.widget.tick(now, &clock::now_local()) {
            Step::Redraw(rects) if !rects.is_empty() => {
                let t0 = Instant::now();
                if let Err(e) = self.renderer.render(&mut self.hw.lcd, &*self.widget, &rects) {
                    log::error!("render failed: {e}");
                }
                self.profile(now, t0.elapsed(), &rects);
            }
            Step::Redraw(_) => {}
            Step::Done => {
                log::debug!("widget finished; showing clock");
                self.show_clock();
            }
        }
    }

    /// Frame profiling: every 2 s of continuous redraws, log frame rate and
    /// where the time goes (total vs. time inside the panel transfer).
    fn profile(&mut self, now: u32, took: Duration, rects: &[crate::gfx::Rect]) {
        let p = &mut self.prof;
        p.frames += 1;
        p.total_us += took.as_micros() as u32;
        p.pixels += rects.iter().map(|r| (r.width() * r.height()) as u32).sum::<u32>();
        if now.wrapping_sub(p.since_ms) >= 2000 {
            let draw_us = crate::co5300::WAIT_US.swap(0, core::sync::atomic::Ordering::Relaxed);
            if p.frames > 5 {
                log::info!(
                    "frames: {:.1} fps, {} ms/frame ({} ms waiting on DMA), {} px/frame",
                    p.frames as f32 * 1000.0 / now.wrapping_sub(p.since_ms) as f32,
                    p.total_us / p.frames / 1000,
                    draw_us / p.frames / 1000,
                    p.pixels / p.frames,
                );
            }
            *p = Profile { since_ms: now, ..Default::default() };
        }
    }

    /// Run forever. With no poller (unprovisioned), only the clock runs.
    pub fn run(mut self, poller: Option<HttpPoller>, wifi: Option<SharedWifi>) -> ! {
        let shared = Arc::new(Mutex::new(Shared::default()));
        if let Some(poller) = poller {
            let net_shared = shared.clone();
            std::thread::Builder::new()
                .name("net".into())
                .stack_size(12 * 1024)
                .spawn(move || network_loop(poller, &net_shared, wifi))
                .expect("failed to spawn network thread");
        }

        loop {
            let (instruction, brightness, config) = {
                let mut s = shared.lock().unwrap();
                (s.instruction.take(), s.brightness.take(), s.config.take())
            };
            if let Some(level) = brightness {
                self.set_brightness(level);
            }
            if let Some(update) = config {
                self.apply_config(&update);
            }
            if let Some(inst) = instruction {
                self.apply_instruction(inst);
            }

            if let Some(epoch) = wifi::take_ntp_sync() {
                log::info!("NTP time synced (epoch: {epoch}s)");
                let now = unsafe { esp_idf_svc::sys::time(core::ptr::null_mut()) };
                let dt = rtc::DateTime::from_unix(now as i64);
                if let Err(e) = rtc::write(&mut self.hw.i2c, &dt) {
                    log::warn!("RTC write-back failed: {e}");
                }
                // The time may have jumped; repaint everything.
                self.widget.invalidate();
            }

            let now = self.now_ms();
            let pressed: Vec<Button> = self.buttons.poll(now).collect();
            for b in pressed {
                self.on_button(b);
            }

            self.step();
            FreeRtos::delay_ms(self.widget.frame_ms().clamp(1, 20));
        }
    }

    fn set_brightness(&mut self, level: u8) {
        self.brightness = level.min(15);
        if let Err(e) = self.hw.lcd.set_brightness(panel_brightness(self.brightness)) {
            log::warn!("set brightness failed: {e}");
        }
    }

    fn on_button(&mut self, b: Button) {
        log::info!("button: {b:?}");
        match b {
            Button::Boot => {
                let i = LOCAL_BRIGHTNESS.iter().position(|&l| l <= self.brightness).unwrap_or(0);
                self.set_brightness(LOCAL_BRIGHTNESS[(i + 1) % LOCAL_BRIGHTNESS.len()]);
            }
            // On the clock: toggle 12/24h. On anything else: dismiss to the clock.
            Button::Key => {
                if self.is_clock {
                    self.format_24h = !self.format_24h;
                }
                self.show_clock();
            }
            Button::Power => {
                self.panel_on = !self.panel_on;
                if let Err(e) = self.hw.lcd.set_on(self.panel_on) {
                    log::warn!("panel on/off failed: {e}");
                }
            }
        }
    }

    fn apply_instruction(&mut self, inst: Instruction) {
        match inst {
            Instruction::Clock { format_24h, style } => {
                self.format_24h = format_24h;
                self.widget = Box::new(Clock::new(format_24h, style.as_deref()));
                self.is_clock = true;
            }
            Instruction::Message { text, scroll_speed_ms, repeats, direction, color, size } => {
                let dir = Direction::parse(direction.as_deref());
                let color = color.as_deref().and_then(parse_color).unwrap_or(WHITE);
                let font = font_for(size.as_deref(), dir);
                let speed = scroll_px_per_ms(scroll_speed_ms);
                self.widget = Box::new(Message::new(&text, font, color, dir, speed, repeats));
                self.is_clock = false;
            }
            Instruction::Ota { url } => self.ota(&url),
            Instruction::Unsupported => {
                log::warn!("unsupported instruction type; showing clock");
                self.show_clock();
            }
        }
    }

    fn ota(&mut self, url: &str) {
        log::info!("OTA requested: {url}");
        self.show(Box::new(Status::new("UPDATING", "downloading firmware…", None)));
        match crate::ota::perform_ota(url) {
            Ok(()) => unreachable!("perform_ota reboots on success"),
            Err(e) => {
                log::error!("OTA failed: {e}");
                self.show(Box::new(Status::new("UPDATE FAILED", &e.to_string(), Some(5000))));
            }
        }
    }

    /// Persist changed keys to NVS and apply them live where possible.
    fn apply_config(&mut self, update: &ConfigUpdate) {
        let mut nvs = match config::open_nvs(self.nvs.clone()) {
            Ok(n) => n,
            Err(e) => {
                log::error!("config update: NVS open failed: {e}");
                return;
            }
        };
        let persist = |nvs: &mut _, key: &str, value: Option<&str>| {
            if let Err(e) = config::persist_str_opt(nvs, key, value) {
                log::error!("config update: persist {key} failed: {e}");
            }
        };

        if let Some(host) = update.syslog_host.as_deref() {
            if host.is_empty() {
                persist(&mut nvs, "syslog_host", None);
                udp_log::disable_udp();
            } else {
                persist(&mut nvs, "syslog_host", Some(host));
                udp_log::enable_udp(host, &self.cfg.device_id);
            }
        }
        if let Some(tz) = update.tz.as_deref().filter(|t| !t.is_empty()) {
            persist(&mut nvs, "tz", Some(tz));
            match wifi::set_timezone(tz) {
                Ok(()) => self.widget.invalidate(),
                Err(e) => log::warn!("config update: apply tz failed: {e}"),
            }
        }
        if let Some(server) = update.ntp_server.as_deref() {
            // Persisted only: lwIP's SNTP client keeps the server name by
            // pointer, so it applies on next boot.
            let value = Some(server).filter(|s| !s.is_empty());
            persist(&mut nvs, "ntp_server", value);
            log::info!("config update: ntp_server = {}; applies on next boot", value.unwrap_or("(pool defaults)"));
        }
        if let Some(level) = update.log_level.as_deref().filter(|l| !l.is_empty()) {
            persist(&mut nvs, "log_level", Some(level));
            let filter = config::parse_level(level);
            log::set_max_level(filter);
            log::info!("log level set: {filter}");
        }
    }
}

fn network_loop(mut poller: HttpPoller, shared: &Mutex<Shared>, wifi: Option<SharedWifi>) {
    // The server returns the *current* desired state on every poll, so only
    // forward changes: re-sending would restart the active widget, and
    // re-sending brightness would undo a BOOT-button adjustment.
    let mut last_instruction: Option<Instruction> = None;
    let mut last_brightness: Option<u8> = None;
    let mut last_config: Option<ConfigUpdate> = None;
    let mut failures: u32 = 0;
    let mut last_telemetry = Instant::now();

    loop {
        match poller.poll() {
            Ok(resp) => {
                failures = 0;
                let mut s = shared.lock().unwrap();
                if let Some(inst) = resp.instruction.filter(|i| last_instruction.as_ref() != Some(i)) {
                    log::info!("instruction changed: {inst:?}");
                    s.instruction = Some(inst.clone());
                    last_instruction = Some(inst);
                }
                if let Some(b) = resp.brightness.filter(|b| last_brightness != Some(*b)) {
                    s.brightness = Some(b);
                    last_brightness = Some(b);
                }
                if let Some(c) = resp.config.filter(|c| last_config.as_ref() != Some(c)) {
                    log::info!("config update received: {c:?}");
                    s.config = Some(c.clone());
                    last_config = Some(c);
                }
            }
            Err(e) => {
                failures += 1;
                log::warn!("poll failed ({failures}/{MAX_CONSECUTIVE_FAILURES}): {e}");
                // Recover from a dropped AP in one interval rather than
                // waiting for the reboot escape hatch.
                if let Some(w) = &wifi {
                    match wifi::reconnect_if_down(&mut w.lock().unwrap()) {
                        Ok(true) => failures = 0,
                        Ok(false) => {}
                        Err(e) => log::warn!("reconnect failed: {e}"),
                    }
                }
                if failures >= MAX_CONSECUTIVE_FAILURES {
                    log::error!("too many consecutive failures, rebooting");
                    unsafe { esp_idf_svc::sys::esp_restart() };
                }
            }
        }

        if last_telemetry.elapsed() >= TELEMETRY_INTERVAL {
            last_telemetry = Instant::now();
            let rssi = wifi.as_ref().and_then(|w| w.lock().ok()).and_then(|g| wifi::get_rssi(&g));
            log::info!(
                "telemetry: rssi={} free_heap={}",
                rssi.map_or("?".into(), |r| r.to_string()),
                unsafe { esp_idf_svc::sys::esp_get_free_heap_size() },
            );
        }

        std::thread::sleep(poller.interval());
    }
}
