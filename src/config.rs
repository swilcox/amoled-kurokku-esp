//! NVS-backed configuration (same keys as led-kurokku-esp), provisioned via
//! `tools/provision.py`.

use anyhow::Result;
use esp_idf_svc::nvs::{EspNvs, NvsDefault};

const NVS_NAMESPACE: &str = "kurokku";

/// Application configuration, loaded exclusively from NVS.
///
/// Firmware binaries are generic: per-device values (WiFi credentials,
/// server URL, device ID, etc.) must be provisioned into NVS via
/// `tools/provision.py` before the device can do useful work. On an
/// unprovisioned device, `has_wifi_config()` returns false and the engine
/// shows a "NO CONFIG" status and then runs the clock offline.
#[derive(Clone)]
pub struct AppConfig {
    pub wifi_ssid: String,
    pub wifi_password: String,
    pub server_url: String,
    pub device_id: String,
    pub format_24h: bool,
    pub default_brightness: u8,
    pub poll_interval_ms: u64,
    /// POSIX TZ string, e.g. "CST6CDT,M3.2.0,M11.1.0" for US Central.
    pub tz: String,
    /// UDP syslog target as "host:port", e.g. "192.168.1.50:5514". None = disabled.
    pub syslog_host: Option<String>,
    /// Maximum log level: "off", "error", "warn", "info", "debug", or "trace".
    pub log_level: String,
    /// NTP server override (host or IP). None = ESP-IDF's pool.ntp.org defaults.
    pub ntp_server: Option<String>,
    /// Default clock face (see `clock::STYLES`), chosen with the KEY button.
    /// A `style` sent by the server takes precedence.
    pub clock_style: String,
}

/// Parse a log level string into a `LevelFilter`. Unrecognized values (including
/// the empty string) fall back to `Info`.
pub fn parse_level(s: &str) -> log::LevelFilter {
    match s.trim().to_ascii_lowercase().as_str() {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    }
}

impl AppConfig {
    /// Load config from NVS. Missing keys fall back to placeholder defaults.
    pub fn load(nvs: &EspNvs<NvsDefault>) -> Self {
        let wifi_ssid = nvs_get_str(nvs, "wifi_ssid").unwrap_or_default();
        let wifi_password = nvs_get_str(nvs, "wifi_pass").unwrap_or_default();
        let server_url = nvs_get_str(nvs, "server_url").unwrap_or_default();
        let device_id =
            nvs_get_str(nvs, "device_id").unwrap_or_else(|| "unprovisioned".to_string());
        let format_24h = nvs_get_u8(nvs, "format_24h").map(|v| v != 0).unwrap_or(true);
        let default_brightness = nvs_get_u8(nvs, "brightness").unwrap_or(12);
        let poll_interval_ms = nvs_get_u32(nvs, "poll_ms").map(|v| v as u64).unwrap_or(5000);
        let tz = nvs_get_str(nvs, "tz").unwrap_or_else(|| "UTC0".to_string());
        let syslog_host = nvs_get_str(nvs, "syslog_host");
        let log_level = nvs_get_str(nvs, "log_level").unwrap_or_else(|| "info".to_string());
        let ntp_server = nvs_get_str(nvs, "ntp_server");
        let clock_style = nvs_get_str(nvs, "clock_style").unwrap_or_else(|| "modern".to_string());

        let cfg = Self {
            wifi_ssid,
            wifi_password,
            server_url,
            device_id,
            format_24h,
            default_brightness,
            poll_interval_ms,
            tz,
            syslog_host,
            log_level,
            ntp_server,
            clock_style,
        };

        log::info!(
            "Config loaded: server={}, device={}, 24h={}, brightness={}, poll={}ms, tz={}, syslog={:?}, log_level={}, ntp={:?}, clock_style={}",
            cfg.server_url,
            cfg.device_id,
            cfg.format_24h,
            cfg.default_brightness,
            cfg.poll_interval_ms,
            cfg.tz,
            cfg.syslog_host,
            cfg.log_level,
            cfg.ntp_server,
            cfg.clock_style,
        );

        cfg
    }

    /// Save current config to NVS.
    pub fn save(&self, nvs: &mut EspNvs<NvsDefault>) -> Result<()> {
        nvs_set_str(nvs, "wifi_ssid", &self.wifi_ssid)?;
        nvs_set_str(nvs, "wifi_pass", &self.wifi_password)?;
        nvs_set_str(nvs, "server_url", &self.server_url)?;
        nvs_set_str(nvs, "device_id", &self.device_id)?;
        nvs_set_u8(nvs, "format_24h", if self.format_24h { 1 } else { 0 })?;
        nvs_set_u8(nvs, "brightness", self.default_brightness)?;
        nvs_set_u32(nvs, "poll_ms", self.poll_interval_ms as u32)?;
        nvs_set_str(nvs, "tz", &self.tz)?;
        nvs_set_str(nvs, "log_level", &self.log_level)?;
        match &self.syslog_host {
            Some(h) => nvs_set_str(nvs, "syslog_host", h)?,
            None => {
                let _ = nvs.remove("syslog_host");
            }
        }
        nvs_set_str(nvs, "clock_style", &self.clock_style)?;
        match &self.ntp_server {
            Some(s) => nvs_set_str(nvs, "ntp_server", s)?,
            None => {
                let _ = nvs.remove("ntp_server");
            }
        }
        log::info!("Config saved to NVS");
        Ok(())
    }

    /// Check if WiFi credentials are configured.
    ///
    /// Only the SSID is required. An empty password is a legitimate
    /// configuration, not a missing one: open networks — common on the campus
    /// and IoT SSIDs that authenticate by registered MAC address instead of a
    /// shared key — have no password to store. Requiring one here would make a
    /// correctly provisioned device refuse to even attempt the join.
    pub fn has_wifi_config(&self) -> bool {
        !self.wifi_ssid.is_empty()
    }
}

/// Open the kurokku NVS namespace.
pub fn open_nvs(
    partition: esp_idf_svc::nvs::EspDefaultNvsPartition,
) -> Result<EspNvs<NvsDefault>> {
    EspNvs::new(partition, NVS_NAMESPACE, true)
        .map_err(|e| anyhow::anyhow!("NVS open failed: {}", e))
}

/// Persist a single string key to NVS, or remove it when `value` is None.
///
/// Used by remote config updates (the `config` field in a server response) so
/// settings like the syslog target can change without re-provisioning a device
/// over serial. ESP-IDF NVS skips the flash write when the stored value already
/// matches, so re-applying the same value is cheap.
pub fn persist_str_opt(
    nvs: &mut EspNvs<NvsDefault>,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    match value {
        Some(v) => nvs_set_str(nvs, key, v),
        None => {
            let _ = nvs.remove(key);
            Ok(())
        }
    }
}

// --- NVS helpers ---

fn nvs_get_str(nvs: &EspNvs<NvsDefault>, key: &str) -> Option<String> {
    let len = nvs.str_len(key).ok().flatten()?;
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    nvs.get_str(key, &mut buf)
        .ok()
        .flatten()
        .map(|s| s.trim_end_matches('\0').to_string())
        .filter(|s| !s.is_empty())
}

fn nvs_get_u8(nvs: &EspNvs<NvsDefault>, key: &str) -> Option<u8> {
    nvs.get_u8(key).ok().flatten()
}

fn nvs_get_u32(nvs: &EspNvs<NvsDefault>, key: &str) -> Option<u32> {
    // esp-idf-svc's NVS wrapper doesn't expose u32; store as i32 and cast.
    nvs.get_i32(key).ok().flatten().map(|v| v as u32)
}

fn nvs_set_str(nvs: &mut EspNvs<NvsDefault>, key: &str, value: &str) -> Result<()> {
    nvs.set_str(key, value)
        .map_err(|e| anyhow::anyhow!("NVS set_str({}) failed: {}", key, e))
}

fn nvs_set_u8(nvs: &mut EspNvs<NvsDefault>, key: &str, value: u8) -> Result<()> {
    nvs.set_u8(key, value)
        .map_err(|e| anyhow::anyhow!("NVS set_u8({}) failed: {}", key, e))
}

fn nvs_set_u32(nvs: &mut EspNvs<NvsDefault>, key: &str, value: u32) -> Result<()> {
    nvs.set_i32(key, value as i32)
        .map_err(|e| anyhow::anyhow!("NVS set_i32({}) failed: {}", key, e))
}
