use anyhow::Result;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::http::Method;
use esp_idf_svc::ota::EspOta;
use std::time::Duration;

fn http_config() -> HttpConfig {
    HttpConfig {
        timeout: Some(Duration::from_secs(60)),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        // GitHub and CDN responses have large headers; 512-byte default causes ESP_FAIL.
        buffer_size: Some(4096),
        buffer_size_tx: Some(1024),
        ..Default::default()
    }
}

/// Download firmware from the given URL and flash it to the inactive OTA partition.
/// EspHttpConnection follows redirects automatically (e.g. GitHub releases → CDN).
/// Reboots the device on success.
pub fn perform_ota(url: &str) -> Result<()> {
    log::info!("OTA: starting download from {}", url);

    // Assets first: they go to the slot the running firmware isn't using,
    // so a failure here (or later) leaves the current firmware intact.
    let assets = assets_url(url);
    match update_assets(&assets) {
        Ok(true) => log::info!("OTA: asset pack written from {}", assets),
        Ok(false) => {}
        Err(e) => anyhow::bail!("asset pack update failed: {}", e),
    }

    // Validate the OTA slot before spending time on the HTTP download.
    // Doing this first avoids holding a live TLS connection open while the
    // flash is being erased, and produces a clear error if the partition
    // table isn't set up correctly.
    let mut ota = EspOta::new()
        .map_err(|e| anyhow::anyhow!("OTA init failed: {}", e))?;

    match ota.get_update_slot() {
        Ok(slot) => log::info!("OTA: update slot: label={}", slot.label),
        Err(e) => anyhow::bail!("OTA: no writable slot found (partition table issue?): {}", e),
    }

    let mut update = ota
        .initiate_update()
        .map_err(|e| anyhow::anyhow!("OTA initiate update failed: {}", e))?;

    let mut connection = EspHttpConnection::new(&http_config())
        .map_err(|e| anyhow::anyhow!("OTA HTTP connection failed: {}", e))?;

    connection
        .initiate_request(Method::Get, url, &[])
        .map_err(|e| anyhow::anyhow!("OTA HTTP request failed: {}", e))?;

    connection
        .initiate_response()
        .map_err(|e| anyhow::anyhow!("OTA HTTP response failed: {}", e))?;

    let status = connection.status();
    if status != 200 {
        anyhow::bail!("OTA server returned status {}", status);
    }

    let mut buf = [0u8; 4096];
    let mut total_bytes: usize = 0;

    loop {
        let bytes_read = connection
            .read(&mut buf)
            .map_err(|e| anyhow::anyhow!("OTA read failed at {} bytes: {}", total_bytes, e))?;

        if bytes_read == 0 {
            break;
        }

        update
            .write(&buf[..bytes_read])
            .map_err(|e| anyhow::anyhow!("OTA write failed at {} bytes: {}", total_bytes, e))?;

        total_bytes += bytes_read;

        if total_bytes % (64 * 1024) < 4096 {
            log::info!("OTA: {}KB downloaded", total_bytes / 1024);
        }
    }

    log::info!("OTA: download complete, {} bytes total. Finalizing...", total_bytes);

    update
        .complete()
        .map_err(|e| anyhow::anyhow!("OTA complete failed: {}", e))?;

    log::info!("OTA: update complete, rebooting...");
    esp_idf_svc::hal::reset::restart();
}

/// Get information about the currently running firmware.
pub fn running_partition_info() -> Result<String> {
    let ota = EspOta::new()
        .map_err(|e| anyhow::anyhow!("OTA init failed: {}", e))?;

    let running = ota.get_running_slot()
        .map_err(|e| anyhow::anyhow!("Failed to get running slot: {}", e))?;

    Ok(format!(
        "partition={}, firmware={}",
        running.label,
        running.firmware.as_ref().map(|f| f.version.as_str()).unwrap_or("unknown"),
    ))
}

/// Confirm the running app is good, cancelling the bootloader's rollback
/// after an OTA. A no-op for apps flashed over serial.
pub fn mark_running_valid() -> Result<()> {
    let mut ota = EspOta::new().map_err(|e| anyhow::anyhow!("OTA init failed: {}", e))?;
    ota.mark_running_slot_valid()
        .map_err(|e| anyhow::anyhow!("mark valid failed: {}", e))
}

/// If the running app is a fresh, unverified OTA, roll back to the previous
/// app (reboots). Otherwise returns.
pub fn rollback_if_unverified() {
    let Ok(mut ota) = EspOta::new() else { return };
    let unverified = ota
        .get_running_slot()
        .map(|s| s.state == esp_idf_svc::ota::SlotState::Unverified)
        .unwrap_or(false);
    if unverified {
        log::error!("OTA: new app is not usable; rolling back");
        let e = ota.mark_running_slot_invalid_and_reboot();
        log::error!("OTA: rollback failed: {}", e);
    }
}

/// Where the asset pack for a firmware URL lives: `x.bin` -> `x.assets.bin`.
fn assets_url(firmware_url: &str) -> String {
    match firmware_url.strip_suffix(".bin") {
        Some(base) => format!("{base}.assets.bin"),
        None => format!("{firmware_url}.assets"),
    }
}

/// Download an asset pack into the inactive assets slot. Returns Ok(false)
/// when there is nothing to do (no pack published, or it's the pack already
/// in use). The header is written last, so an interrupted write never leaves
/// a slot that looks valid.
fn update_assets(url: &str) -> Result<bool> {
    use crate::assets::{self, parse_header, HEADER_LEN};
    use esp_idf_svc::sys::*;

    let mut conn = EspHttpConnection::new(&http_config())
        .map_err(|e| anyhow::anyhow!("HTTP connection failed: {}", e))?;
    conn.initiate_request(Method::Get, url, &[])
        .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;
    conn.initiate_response()
        .map_err(|e| anyhow::anyhow!("HTTP response failed: {}", e))?;
    match conn.status() {
        200 => {}
        404 => {
            log::info!("OTA: no asset pack at {}; keeping current assets", url);
            return Ok(false);
        }
        s => anyhow::bail!("server returned status {}", s),
    }

    let mut read_full = |buf: &mut [u8]| -> Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            let got = conn.read(&mut buf[n..]).map_err(|e| anyhow::anyhow!("read failed: {}", e))?;
            if got == 0 {
                break;
            }
            n += got;
        }
        Ok(n)
    };

    let mut header = [0u8; HEADER_LEN];
    if read_full(&mut header)? != HEADER_LEN {
        anyhow::bail!("asset pack truncated");
    }
    let (hash, len) = parse_header(&header).ok_or_else(|| anyhow::anyhow!("not an asset pack"))?;
    if hash == assets::ASSETS_HASH {
        log::info!("OTA: asset pack {:016x} already installed", hash);
        return Ok(false);
    }

    let slot = assets::inactive_slot();
    let part = assets::partition(slot)?;
    let size = unsafe { (*part).size } as usize;
    if HEADER_LEN + len > size {
        anyhow::bail!("asset pack ({} bytes) larger than {} ({} bytes)", len, slot, size);
    }
    let check = |err: esp_err_t, what: &str| EspError::convert(err).map_err(|e| anyhow::anyhow!("{}: {}", what, e));
    log::info!("OTA: writing asset pack {:016x} ({} bytes) to {}", hash, len, slot);
    let erase = (HEADER_LEN + len).div_ceil(4096) * 4096;
    check(unsafe { esp_partition_erase_range(part, 0, erase) }, "erase")?;

    let mut buf = [0u8; 4096];
    let mut written = 0;
    let mut fnv = 0xcbf2_9ce4_8422_2325u64; // same FNV-1a as build_support/assetpack.rs
    while written < len {
        let want = buf.len().min(len - written);
        let n = read_full(&mut buf[..want])?;
        if n == 0 {
            anyhow::bail!("asset pack truncated at {} of {} bytes", written, len);
        }
        check(unsafe { esp_partition_write(part, HEADER_LEN + written, buf.as_ptr().cast(), n) }, "write")?;
        fnv = buf[..n].iter().fold(fnv, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
        written += n;
    }
    if fnv != hash {
        anyhow::bail!("asset pack corrupt: hash {:016x}, header says {:016x}", fnv, hash);
    }
    check(unsafe { esp_partition_write(part, 0, header.as_ptr().cast(), HEADER_LEN) }, "write header")?;
    Ok(true)
}
