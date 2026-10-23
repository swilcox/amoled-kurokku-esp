//! Two-thread engine, modeled on led-kurokku-esp.
//!
//! - **Network thread**: polls the server, deduplicates, and hands changes
//!   (instruction, brightness, config) to the display thread via `Shared`.
//! - **Input thread** (`input`): polls touch and the accelerometer and
//!   sends gestures and orientation changes.
//! - **Display thread** (the caller of `run`): owns the panel; runs the
//!   active widget, applies changes, handles buttons and gestures, performs
//!   OTA, and writes NTP time back to the RTC.
//!
//! When the server is unreachable or a widget finishes, the clock shows.
//! An overlay (the info screen, a notice) covers the active widget for a
//! few seconds; meanwhile server changes wait, and afterwards the widget is
//! repainted.
//!
//! Controls:
//!
//! | Input | Action |
//! |---|---|
//! | KEY | next clock face / close overlay / dismiss to the clock |
//! | KEY (hold) | info screen |
//! | BOOT | cycle brightness |
//! | BOOT (hold) | lock rotation where it is / back to auto-rotate |
//! | PWR | screen off/on (any button wakes it) |
//! | tap | info screen on/off |
//! | swipe left / right | next / previous clock face (dismiss to the clock) |
//! | swipe up / down | brighter / dimmer |

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};

use crate::board::LCD_W;
use crate::buttons::{Button, Buttons, Press};
use crate::clock::{self, STYLES};
use crate::co5300::Co5300;
use crate::color::WHITE;
use crate::config::{self, AppConfig};
use crate::gfx::{Renderer, Rotation};
use crate::input::{self, AppliedRotation, Input, SharedI2c};
use crate::poller::{HttpPoller, FIRMWARE_VERSION};
use crate::protocol::{panel_brightness, parse_color, scroll_px_per_ms, ConfigUpdate, Instruction};
use crate::widget::message::{font_for, Direction, Message};
use crate::touch::{Gesture, Swipe};
use crate::widget::{clock::Clock, hold::Hold, info::Info, status::Status, Step, Widget};
use crate::{image, media, rtc, udp_log, wifi};

pub type SharedWifi = Arc<Mutex<BlockingWifi<EspWifi<'static>>>>;

const BAND_PIXELS: usize = LCD_W as usize * 32;
/// Brightness levels (server 0-15 scale) the BOOT button cycles through.
const LOCAL_BRIGHTNESS: [u8; 5] = [15, 10, 6, 3, 0];
/// Reboot after this many consecutive failed polls (~5 min at 5 s).
const MAX_CONSECUTIVE_FAILURES: u32 = 60;
const TELEMETRY_INTERVAL: Duration = Duration::from_secs(60);
const INFO_MS: u32 = 10_000;
const NOTICE_MS: u32 = 1500;

/// Changes from the network thread, taken by the display thread, and the
/// server link's health for the info screen.
#[derive(Default)]
struct Shared {
    instruction: Option<Instruction>,
    brightness: Option<u8>,
    config: Option<ConfigUpdate>,
    polling: bool,
    last_ok: Option<Instant>,
    failures: u32,
}

pub struct Hardware {
    pub lcd: Co5300,
    /// Shared with the input thread (touch, IMU).
    pub i2c: SharedI2c,
}

pub struct Engine {
    cfg: AppConfig,
    nvs: EspDefaultNvsPartition,
    hw: Hardware,
    buttons: Buttons,
    renderer: Renderer,
    widget: Box<dyn Widget>,
    is_clock: bool,
    /// Shown over `widget` until it finishes.
    overlay: Option<Box<dyn Widget>>,
    /// The URL `widget` (a `Hold`) is showing, to repaint it.
    image: Option<String>,
    format_24h: bool,
    /// Face chosen on the device (KEY), persisted in NVS.
    local_style: String,
    /// Face named by the server's current clock instruction, if any; it
    /// overrides `local_style`.
    server_style: Option<String>,
    /// Current brightness on the server's 0-15 scale.
    brightness: u8,
    panel_on: bool,
    boot: Instant,
    prof: Profile,
    slideshow: Option<Slideshow>,
    shared: Arc<Mutex<Shared>>,
    /// Follow the IMU (else `rotation` in config is fixed).
    auto_rotate: bool,
    /// The way the IMU last said the device is standing.
    standing: Option<Rotation>,
    applied: AppliedRotation,
}

/// An SD-card slideshow in progress.
struct Slideshow {
    images: Vec<String>,
    next: usize,
    interval_ms: u32,
    next_at_ms: u32,
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
        let local_style = cfg.clock_style.clone();
        let brightness = cfg.default_brightness.min(15);
        hw.lcd.set_brightness(panel_brightness(brightness))?;
        let fixed = Rotation::from_degrees(&cfg.rotation);
        let mut renderer = Renderer::new(BAND_PIXELS);
        renderer.set_rotation(fixed.unwrap_or_default());
        let applied = AppliedRotation::default();
        applied.set(renderer.rotation());
        Ok(Self {
            cfg,
            nvs,
            hw,
            buttons: Buttons::new(),
            renderer,
            widget: Box::new(Clock::new(format_24h, Some(&local_style))),
            is_clock: true,
            overlay: None,
            image: None,
            format_24h,
            local_style,
            server_style: None,
            brightness,
            panel_on: true,
            boot: Instant::now(),
            prof: Profile::default(),
            slideshow: None,
            shared: Arc::default(),
            auto_rotate: fixed.is_none(),
            standing: None,
            applied,
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
        self.set_widget(widget, false);
        self.step();
    }

    fn set_widget(&mut self, widget: Box<dyn Widget>, is_clock: bool) {
        self.widget = widget;
        self.is_clock = is_clock;
        self.image = None;
    }

    fn show_clock(&mut self) {
        self.slideshow = None;
        let style = self.server_style.as_deref().unwrap_or(&self.local_style);
        self.set_widget(Box::new(Clock::new(self.format_24h, Some(style))), true);
    }

    fn open_overlay(&mut self, overlay: Box<dyn Widget>) {
        self.overlay = Some(overlay);
    }

    fn close_overlay(&mut self) {
        if self.overlay.take().is_some() {
            self.repaint();
        }
    }

    /// Paint the active widget from scratch (after an overlay or rotation).
    fn repaint(&mut self) {
        match self.image.clone() {
            Some(url) => self.show_image(&url),
            None => self.widget.invalidate(),
        }
    }

    /// Switch to the next (`step` 1) or previous (-1) face and remember it.
    fn cycle_style(&mut self, step: isize) {
        let current = self.server_style.take().unwrap_or_else(|| self.local_style.clone());
        let i = STYLES.iter().position(|s| *s == current).unwrap_or(0) as isize;
        self.local_style = STYLES[(i + step).rem_euclid(STYLES.len() as isize) as usize].to_string();
        log::info!("clock style: {}", self.local_style);
        match config::open_nvs(self.nvs.clone()) {
            Ok(mut nvs) => {
                if let Err(e) = config::persist_str_opt(&mut nvs, "clock_style", Some(&self.local_style)) {
                    log::warn!("persist clock_style failed: {e}");
                }
            }
            Err(e) => log::warn!("persist clock_style failed: {e}"),
        }
    }

    /// Tick the active widget (or the overlay over it) once and paint what
    /// changed.
    fn step(&mut self) {
        let now = self.now_ms();
        let active = self.overlay.as_mut().unwrap_or(&mut self.widget);
        match active.tick(now, &clock::now_local()) {
            Step::Redraw(rects) if !rects.is_empty() => {
                let t0 = Instant::now();
                if let Err(e) = self.renderer.render(&mut self.hw.lcd, &**active, &rects) {
                    log::error!("render failed: {e}");
                }
                self.profile(now, t0.elapsed(), &rects);
            }
            Step::Redraw(_) => {}
            Step::Done if self.overlay.is_some() => self.close_overlay(),
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
        let shared = self.shared.clone();
        let (tx, inputs) = mpsc::channel();
        input::spawn(self.hw.i2c.clone(), self.applied.clone(), tx);
        if let Some(poller) = poller {
            shared.lock().unwrap().polling = true;
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
                // New content waits for an overlay to close.
                let instruction = if self.overlay.is_none() { s.instruction.take() } else { None };
                (instruction, s.brightness.take(), s.config.take())
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
                if let Err(e) = rtc::write(&mut self.hw.i2c.lock().unwrap(), &dt) {
                    log::warn!("RTC write-back failed: {e}");
                }
                // The time may have jumped; repaint everything.
                self.widget.invalidate();
            }

            let now = self.now_ms();
            let pressed: Vec<Press> = self.buttons.poll(now).collect();
            for p in pressed {
                self.on_press(p);
            }
            self.take_inputs(&inputs);

            if self.overlay.is_none() {
                self.step_slideshow();
            }
            self.step();
            let frame_ms = self.overlay.as_ref().unwrap_or(&self.widget).frame_ms();
            FreeRtos::delay_ms(frame_ms.clamp(1, 20));
        }
    }

    fn take_inputs(&mut self, inputs: &Receiver<Input>) {
        while let Ok(i) = inputs.try_recv() {
            match i {
                Input::Gesture(g) if self.panel_on => self.on_gesture(g),
                Input::Gesture(_) => {}
                Input::Orientation(r) => {
                    self.standing = Some(r);
                    if self.auto_rotate {
                        self.rotate(r);
                    }
                }
            }
        }
    }

    fn rotate(&mut self, r: Rotation) {
        if r == self.renderer.rotation() {
            return;
        }
        log::info!("rotation: {}°", r.degrees());
        self.renderer.set_rotation(r);
        self.applied.set(r);
        match &mut self.overlay {
            Some(o) => o.invalidate(),
            None => self.repaint(),
        }
    }

    /// One step along `LOCAL_BRIGHTNESS`: brighter (`step` < 0) or dimmer;
    /// `wrap` cycles past the ends.
    fn step_brightness(&mut self, step: isize, wrap: bool) {
        let n = LOCAL_BRIGHTNESS.len() as isize;
        let i = LOCAL_BRIGHTNESS.iter().position(|&l| l <= self.brightness).unwrap_or(0) as isize;
        let j = if wrap { (i + step).rem_euclid(n) } else { (i + step).clamp(0, n - 1) };
        self.set_brightness(LOCAL_BRIGHTNESS[j as usize]);
    }

    fn set_brightness(&mut self, level: u8) {
        self.brightness = level.min(15);
        if let Err(e) = self.hw.lcd.set_brightness(panel_brightness(self.brightness)) {
            log::warn!("set brightness failed: {e}");
        }
    }

    fn set_panel_on(&mut self, on: bool) {
        self.panel_on = on;
        if let Err(e) = self.hw.lcd.set_on(on) {
            log::warn!("panel on/off failed: {e}");
        }
    }

    fn on_press(&mut self, p: Press) {
        log::info!("button: {p:?}");
        if !self.panel_on {
            // Any button wakes the screen, and does nothing else.
            self.set_panel_on(true);
            return;
        }
        match p {
            Press::Short(Button::Key) => self.next_or_dismiss(1),
            Press::Long(Button::Key) => self.open_info(),
            Press::Short(Button::Boot) => self.step_brightness(1, true),
            Press::Long(Button::Boot) => self.toggle_rotation_lock(),
            Press::Short(Button::Power) => self.set_panel_on(false),
            Press::Long(Button::Power) => {}
        }
    }

    fn on_gesture(&mut self, g: Gesture) {
        match g {
            Gesture::Tap(..) if self.overlay.is_some() => self.close_overlay(),
            Gesture::Tap(..) => self.open_info(),
            Gesture::Swipe(Swipe::Left) => self.next_or_dismiss(1),
            Gesture::Swipe(Swipe::Right) => self.next_or_dismiss(-1),
            Gesture::Swipe(Swipe::Up) => self.step_brightness(-1, false),
            Gesture::Swipe(Swipe::Down) => self.step_brightness(1, false),
            Gesture::LongPress(..) => {}
        }
    }

    /// Close an overlay; else on the clock, change face; else dismiss to
    /// the clock.
    fn next_or_dismiss(&mut self, step: isize) {
        if self.overlay.is_some() {
            self.close_overlay();
            return;
        }
        if self.is_clock {
            self.cycle_style(step);
        }
        self.show_clock();
    }

    /// BOOT hold: pin the screen the way it's turned now, or go back to
    /// following the IMU. Persisted.
    fn toggle_rotation_lock(&mut self) {
        self.auto_rotate = !self.auto_rotate;
        let value = if self.auto_rotate { "auto".to_string() } else { self.renderer.rotation().degrees().to_string() };
        if let Err(e) = config::open_nvs(self.nvs.clone()).and_then(|mut nvs| config::persist_str_opt(&mut nvs, "rotation", Some(&value))) {
            log::warn!("persist rotation failed: {e}");
        }
        let notice = if self.auto_rotate {
            if let Some(r) = self.standing {
                self.rotate(r);
            }
            Status::new("ROTATE", "automatic", Some(NOTICE_MS))
        } else {
            Status::new("LOCKED", &format!("fixed at {}°", self.renderer.rotation().degrees()), Some(NOTICE_MS))
        };
        log::info!("rotation: {value}");
        self.open_overlay(Box::new(notice));
    }

    fn open_info(&mut self) {
        let (server, ago) = {
            let s = self.shared.lock().unwrap();
            let ago = s.last_ok.map(|t| t.elapsed().as_secs());
            (if !s.polling { "offline".to_string() } else if s.failures > 0 { format!("failing ({})", s.failures) } else { String::new() }, ago)
        };
        let server = match (server.is_empty(), ago) {
            (true, Some(a)) => format!("ok {a} s ago"),
            (true, None) => "connecting".into(),
            (false, _) => server,
        };
        let up = self.boot.elapsed().as_secs();
        let uptime = match up / 86400 {
            0 => format!("{:02}:{:02}:{:02}", up / 3600, up / 60 % 60, up % 60),
            d => format!("{d} d {:02}:{:02}", up / 3600 % 24, up / 60 % 60),
        };
        let rotation = match (self.auto_rotate, self.standing) {
            (true, Some(_)) => format!("auto ({})", self.renderer.rotation().degrees()),
            (true, None) => "auto (no IMU)".into(),
            (false, _) => format!("fixed ({})", self.renderer.rotation().degrees()),
        };
        let rows = [
            ("IP", wifi::sta_ip().unwrap_or_else(|| "-".into())),
            ("WiFi", wifi::sta_rssi().map_or("-".into(), |r| format!("{r} dBm"))),
            ("Server", server),
            ("Firmware", FIRMWARE_VERSION.replace("%2B", "+")),
            ("Uptime", uptime),
            ("Free RAM", format!("{} KB", unsafe { esp_idf_svc::sys::esp_get_free_heap_size() } / 1024)),
            ("Rotation", rotation),
        ];
        let rows = rows.into_iter().map(|(l, v)| (l.to_string(), v)).collect();
        self.open_overlay(Box::new(Info::new(&self.cfg.device_id, rows, INFO_MS)));
    }

    fn apply_instruction(&mut self, inst: Instruction) {
        self.slideshow = None;
        match inst {
            Instruction::Clock { format_24h, style } => {
                self.format_24h = format_24h;
                self.server_style = style.filter(|s| !s.is_empty());
                self.show_clock();
            }
            Instruction::Message { text, scroll_speed_ms, repeats, direction, color, size } => {
                let dir = Direction::parse(direction.as_deref());
                let color = color.as_deref().and_then(parse_color).unwrap_or(WHITE);
                let font = font_for(size.as_deref(), dir);
                let speed = scroll_px_per_ms(scroll_speed_ms);
                self.set_widget(Box::new(Message::new(&text, font, color, dir, speed, repeats)), false);
            }
            Instruction::Ota { url } => self.ota(&url),
            Instruction::Image { url } => self.show_image(&url),
            Instruction::Slideshow { path, interval_secs } => match media::list_images(&path) {
                Ok(images) if !images.is_empty() => {
                    log::info!("slideshow: {} images in {path}", images.len());
                    self.slideshow = Some(Slideshow {
                        images,
                        next: 0,
                        interval_ms: interval_secs.max(1) * 1000,
                        next_at_ms: self.now_ms(),
                    });
                }
                Ok(_) => self.show(Box::new(Status::new("NO IMAGES", &path, Some(5000)))),
                Err(e) => {
                    log::warn!("slideshow: {e}");
                    self.show(Box::new(Status::new("NO IMAGES", &path, Some(5000))));
                }
            },
            Instruction::Unsupported => {
                log::warn!("unsupported instruction type; showing clock");
                self.show_clock();
            }
        }
    }

    /// Stream an image onto the panel and hold it there. A URL starting
    /// with `/` (the server's image library) is relative to the server.
    fn show_image(&mut self, url: &str) {
        let started = Instant::now();
        let url = match url.starts_with('/') {
            true => format!("{}{url}", self.cfg.server_url.trim_end_matches('/')),
            false => url.to_string(),
        };
        let url = url.as_str();
        let result = media::open(url).and_then(|r| image::show(&mut self.renderer, &mut self.hw.lcd, r));
        match result {
            Ok(()) => {
                log::info!("image {url}: {} ms", started.elapsed().as_millis());
                self.set_widget(Box::new(Hold), false);
                self.image = Some(url.to_string());
            }
            Err(e) => {
                log::warn!("image {url}: {e}");
                self.show(Box::new(Status::new("IMAGE ERROR", &e.to_string(), Some(5000))));
            }
        }
    }

    /// Advance a running slideshow when its next image is due.
    fn step_slideshow(&mut self) {
        let now = self.now_ms();
        let Some(s) = &mut self.slideshow else { return };
        if now.wrapping_sub(s.next_at_ms) as i32 >= 0 {
            let url = s.images[s.next % s.images.len()].clone();
            s.next = (s.next + 1) % s.images.len();
            s.next_at_ms = now.wrapping_add(s.interval_ms);
            self.show_image(&url);
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
                s.last_ok = Some(Instant::now());
                s.failures = 0;
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
                shared.lock().unwrap().failures = failures;
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
