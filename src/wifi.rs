use anyhow::Result;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
pub use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Epoch seconds of the most recent completed NTP sync, published by the SNTP
/// notification callback. `0` means "nothing new since the last drain".
///
/// The callback runs in the lwIP tcpip thread, so it must do **no** IO or
/// blocking work — a UDP syslog send from that context deadlocks the tcpip
/// thread (and with it all networking and the display). It therefore only
/// stores this value; a normal thread drains it via [`take_ntp_sync`] and logs.
/// (riscv32imac has no 64-bit atomics; `u32` epoch seconds suffice until 2106.)
static LAST_NTP_SYNC: AtomicU32 = AtomicU32::new(0);

/// Epoch-seconds of an NTP sync that completed since the previous call, or
/// `None` if none has. Draining the slot ensures each sync is reported once.
pub fn take_ntp_sync() -> Option<u32> {
    match LAST_NTP_SYNC.swap(0, Ordering::AcqRel) {
        0 => None,
        secs => Some(secs),
    }
}

/// Pick the authentication threshold for a station configuration.
///
/// ESP-IDF treats `auth_method` as the *minimum* mode it will associate with,
/// so an empty password has to be paired with `AuthMethod::None`: the crate
/// default of WPA2-Personal filters out exactly the network we're trying to
/// join. Open SSIDs are common on campus and IoT networks, which authenticate
/// by registered MAC address instead of a shared key.
fn auth_for(password: &str) -> AuthMethod {
    if password.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    }
}

/// Create the WiFi driver, apply the station configuration, and start the
/// radio — without associating. Call [`join`] to associate.
///
/// The split matters: a failed association must not destroy the driver. The
/// caller keeps the handle and can retry in place (and, once a setup portal
/// exists, hand it over) instead of rebooting to get a fresh one.
pub fn init(
    ssid: &str,
    password: &str,
    modem: esp_idf_svc::hal::modem::Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> Result<BlockingWifi<EspWifi<'static>>> {
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sysloop.clone(), Some(nvs))
            .map_err(|e| anyhow::anyhow!("WiFi driver init failed: {}", e))?,
        sysloop,
    )
    .map_err(|e| anyhow::anyhow!("BlockingWifi wrap failed: {}", e))?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid.try_into().map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: password.try_into().map_err(|_| anyhow::anyhow!("Password too long"))?,
        auth_method: auth_for(password),
        ..Default::default()
    }))
    .map_err(|e| anyhow::anyhow!("WiFi config failed: {}", e))?;

    wifi.start().map_err(|e| anyhow::anyhow!("WiFi start failed: {}", e))?;
    log::info!(
        "WiFi started: ssid={} ({})",
        ssid,
        if password.is_empty() { "open" } else { "psk" }
    );

    Ok(wifi)
}

/// Associate with the configured network and wait for an IP. Blocks until
/// connected or fails; safe to call again after a failure.
pub fn join(wifi: &mut BlockingWifi<EspWifi<'static>>) -> Result<()> {
    wifi.connect().map_err(|e| anyhow::anyhow!("WiFi connect failed: {}", e))?;
    log::info!("WiFi associated, waiting for IP...");

    wifi.wait_netif_up().map_err(|e| anyhow::anyhow!("WiFi netif up failed: {}", e))?;

    let ip_info = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .map_err(|e| anyhow::anyhow!("Failed to get IP info: {}", e))?;
    log::info!("WiFi connected: IP={}", ip_info.ip);

    Ok(())
}

/// The station interface's MAC address, formatted `AA:BB:CC:DD:EE:FF`.
///
/// This is the address a network's device-registration portal wants. Read it
/// explicitly rather than off whichever interface happens to be up: the ESP32
/// derives the SoftAP MAC from the same base address with a different offset,
/// so the two differ, and registering the AP address yields a device that
/// silently never associates.
///
/// Backed by `esp_read_mac`, which reads eFuse — so this works before the WiFi
/// driver has been created, which is exactly when you need it on a network
/// that requires registration before association.
pub fn sta_mac() -> Result<String> {
    let mut mac = [0u8; 6];
    let err = unsafe {
        esp_idf_svc::sys::esp_read_mac(
            mac.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_WIFI_STA,
        )
    };
    if err != esp_idf_svc::sys::ESP_OK {
        anyhow::bail!("esp_read_mac failed: {}", err);
    }
    Ok(format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    ))
}

/// Read the current AP signal strength (RSSI) in dBm, or None if the station
/// isn't associated. Backed by the global `esp_wifi_sta_get_rssi`, so it does
/// not need the WiFi handle locked — the parameter just documents that a
/// connection is expected.
pub fn get_rssi(_wifi: &BlockingWifi<EspWifi<'static>>) -> Option<i32> {
    let mut rssi: core::ffi::c_int = 0;
    let err = unsafe { esp_idf_svc::sys::esp_wifi_sta_get_rssi(&mut rssi) };
    if err == esp_idf_svc::sys::ESP_OK {
        Some(rssi as i32)
    } else {
        None
    }
}

/// Get the station IP address as a string, or None if not connected.
pub fn get_ip(wifi: &BlockingWifi<EspWifi<'static>>) -> Option<String> {
    wifi.wifi()
        .sta_netif()
        .get_ip_info()
        .ok()
        .map(|info| format!("{}", info.ip))
}

/// Start SNTP and block up to `wait` for the first sync to complete.
///
/// Returns the SNTP handle, which the caller **must keep alive for the lifetime
/// of the program**: dropping it calls `sntp_stop()` and kills periodic re-sync
/// (ESP-IDF re-syncs every hour by default via `CONFIG_LWIP_SNTP_UPDATE_DELAY`).
///
/// A timeout is *not* fatal — the returned handle keeps polling in the
/// background, so a device that boots before its NTP server is reachable will
/// still sync once the network recovers, instead of staying on a wrong time
/// until reboot. Pass `Duration::ZERO` to skip the wait entirely, which is what
/// you want when the station hasn't associated yet: there is nothing to wait
/// for, and blocking would just leave the display dark.
///
/// `ntp_server`, when set, overrides the primary pool server (NVS `ntp_server`).
pub fn sync_ntp(ntp_server: Option<&str>, wait: Duration) -> Result<EspSntp<'static>> {
    use esp_idf_svc::sntp::{EspSntp, SntpConf};

    let mut conf = SntpConf::default();
    match ntp_server.map(str::trim).filter(|s| !s.is_empty()) {
        Some(server) => {
            // Override the primary server; any remaining slots keep their
            // pool.ntp.org defaults as fallback (lwIP tries them in order).
            conf.servers[0] = server;
            log::info!("Starting NTP sync (server override: {})...", server);
        }
        None => log::info!("Starting NTP sync..."),
    }

    // The callback fires on every completed sync, including the hourly
    // background re-syncs that ESP-IDF performs on its own. It runs in the lwIP
    // tcpip thread, so it must not log or do any IO here (that deadlocks the
    // tcpip thread); it only publishes the timestamp for a normal thread to log.
    let sntp = EspSntp::new_with_callback(&conf, |synced| {
        LAST_NTP_SYNC.store(synced.as_secs() as u32, Ordering::Release);
    })
    .map_err(|e| anyhow::anyhow!("SNTP init failed: {}", e))?;

    if wait.is_zero() {
        log::info!("NTP started; first sync will complete in the background");
        return Ok(sntp);
    }

    // Block until the first sync lands so the engine starts with correct time.
    let start = std::time::Instant::now();
    while sntp.get_sync_status() != esp_idf_svc::sntp::SyncStatus::Completed {
        if start.elapsed() > wait {
            // Hand the still-running handle back so background re-sync continues.
            log::warn!(
                "NTP first sync not completed within {}s; continuing in background",
                wait.as_secs()
            );
            return Ok(sntp);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(sntp)
}

/// If the STA is disconnected, try to reconnect once. Returns Ok(true) if a
/// reconnect was attempted and succeeded, Ok(false) if already connected.
/// On reconnect failure, returns Err — caller should keep running and retry
/// on the next poll interval rather than panicking.
pub fn reconnect_if_down(wifi: &mut BlockingWifi<EspWifi<'static>>) -> Result<bool> {
    let connected = wifi
        .is_connected()
        .map_err(|e| anyhow::anyhow!("is_connected failed: {}", e))?;
    if connected {
        return Ok(false);
    }

    log::warn!("WiFi disconnected — attempting reconnect");
    wifi.connect()
        .map_err(|e| anyhow::anyhow!("reconnect failed: {}", e))?;
    wifi.wait_netif_up()
        .map_err(|e| anyhow::anyhow!("netif up failed after reconnect: {}", e))?;
    log::info!("WiFi reconnected");
    Ok(true)
}

/// Apply a POSIX TZ string to the C runtime so `localtime_r` honors it.
/// Call this after NTP sync.
pub fn set_timezone(tz: &str) -> Result<()> {
    use std::ffi::CString;
    let key = CString::new("TZ").unwrap();
    let val = CString::new(tz).map_err(|_| anyhow::anyhow!("invalid TZ string"))?;
    unsafe {
        esp_idf_svc::sys::setenv(key.as_ptr(), val.as_ptr(), 1);
        esp_idf_svc::sys::tzset();
    }
    log::info!("Timezone set: {}", tz);
    Ok(())
}
