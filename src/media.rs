//! Image sources on the device: HTTP(S) URLs and the microSD card.
//!
//! The card is on the display's SPI bus (SPI2: CLK=0, MOSI=1, MISO=2) with
//! its own chip select, so it is mounted as a second device on the bus the
//! display driver already initialized.

use std::io::Read;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::sys::*;

use crate::board::SD_CS;

pub const SD_MOUNT: &str = "/sdcard";
const SD_PREFIX: &str = "sd:";

// sdmmc_host_t flags (SDMMC_HOST_FLAG_SPI / _DEINIT_ARG are BIT() macros
// that bindgen doesn't translate).
const HOST_FLAG_SPI: u32 = 1 << 3;
const HOST_FLAG_DEINIT_ARG: u32 = 1 << 5;
const SD_FREQ_KHZ: i32 = 20_000;

/// Mount the card's FAT filesystem at `/sdcard`. Call after the display
/// (which owns the SPI bus) is initialized. Fails if no card is inserted.
pub fn mount_sd() -> Result<()> {
    // Mirrors ESP-IDF's SDSPI_HOST_DEFAULT() (cf. esp-idf-hal's sd.rs).
    let mut host: sdmmc_host_t = Default::default();
    host.flags = HOST_FLAG_SPI | HOST_FLAG_DEINIT_ARG;
    host.slot = spi_host_device_t_SPI2_HOST as _;
    host.max_freq_khz = SD_FREQ_KHZ;
    host.io_voltage = 3.3;
    host.init = Some(sdspi_host_init);
    host.set_card_clk = Some(sdspi_host_set_card_clk);
    host.do_transaction = Some(sdspi_host_do_transaction);
    host.__bindgen_anon_1.deinit_p = Some(sdspi_host_remove_device);
    host.io_int_enable = Some(sdspi_host_io_int_enable);
    host.io_int_wait = Some(sdspi_host_io_int_wait);
    host.get_real_freq = Some(sdspi_host_get_real_freq);
    host.get_dma_info = Some(sdspi_host_get_dma_info);
    host.check_buffer_alignment = Some(sdspi_host_check_buffer_alignment);

    let mut slot: sdspi_device_config_t = Default::default();
    slot.host_id = spi_host_device_t_SPI2_HOST;
    slot.gpio_cs = SD_CS;
    slot.gpio_cd = -1;
    slot.gpio_wp = -1;
    slot.gpio_int = -1;

    let mount = esp_vfs_fat_sdmmc_mount_config_t {
        format_if_mount_failed: false,
        max_files: 4,
        allocation_unit_size: 16 * 1024,
        ..Default::default()
    };
    let base = std::ffi::CString::new(SD_MOUNT)?;
    let mut card: *mut sdmmc_card_t = core::ptr::null_mut();
    EspError::convert(unsafe { esp_vfs_fat_sdspi_mount(base.as_ptr(), &host, &slot, &mount, &mut card) })
        .map_err(|e| anyhow!("SD mount failed: {e}"))?;
    let (sectors, size) = unsafe { ((*card).csd.capacity as u64, (*card).csd.sector_size as u64) };
    log::info!("SD card mounted at {SD_MOUNT}: {} MB", sectors * size / (1024 * 1024));
    Ok(())
}

fn sd_path(url: &str) -> Option<String> {
    let path = url.strip_prefix(SD_PREFIX)?;
    Some(format!("{SD_MOUNT}/{}", path.trim_start_matches('/')))
}

/// A response body as `std::io::Read`.
struct HttpBody(EspHttpConnection);

impl Read for HttpBody {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf).map_err(|e| std::io::Error::other(e.to_string()))
    }
}

/// Open an image source: `http(s)://…` or `sd:/path/on/card`.
pub fn open(url: &str) -> Result<Box<dyn Read>> {
    if let Some(path) = sd_path(url) {
        let f = std::fs::File::open(&path).map_err(|e| anyhow!("{path}: {e}"))?;
        return Ok(Box::new(f));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("unsupported URL {url:?}");
    }
    let mut conn = EspHttpConnection::new(&HttpConfig {
        timeout: Some(Duration::from_secs(15)),
        crt_bundle_attach: Some(esp_crt_bundle_attach),
        buffer_size: Some(2048),
        ..Default::default()
    })
    .map_err(|e| anyhow!("HTTP connection failed: {e}"))?;
    conn.initiate_request(esp_idf_svc::http::Method::Get, url, &[])
        .map_err(|e| anyhow!("HTTP request failed: {e}"))?;
    conn.initiate_response().map_err(|e| anyhow!("HTTP response failed: {e}"))?;
    match conn.status() {
        200 => Ok(Box::new(HttpBody(conn))),
        s => bail!("{url}: HTTP {s}"),
    }
}

/// The `.qoi` files in an `sd:/dir`, sorted, as `sd:` URLs.
pub fn list_images(dir_url: &str) -> Result<Vec<String>> {
    let dir = sd_path(dir_url).ok_or_else(|| anyhow!("slideshow path must start with sd:"))?;
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| anyhow!("{dir}: {e}"))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.to_ascii_lowercase().ends_with(".qoi") && !n.starts_with('.'))
        .collect();
    names.sort();
    let base = dir_url.trim_end_matches('/');
    Ok(names.into_iter().map(|n| format!("{base}/{n}")).collect())
}
