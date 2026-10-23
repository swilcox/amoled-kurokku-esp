//! Fonts and face artwork live in an asset pack (see
//! `build_support/assetpack.rs`) rather than in the 3 MB app image.
//!
//! On the device the pack is flashed to one of two partitions, `assets_0` or
//! `assets_1` (A/B, like the app's OTA slots), and memory-mapped at boot: the
//! firmware maps whichever slot holds the pack it was built with
//! (`ASSETS_HASH`). An OTA writes the new pack to the *other* slot, so the
//! running firmware keeps its assets if an update fails. On the host the
//! pack is embedded, for tests and previews.

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// A byte range of the asset pack payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blob {
    pub off: u32,
    pub len: u32,
}

impl Blob {
    /// The blob's bytes, or an empty slice if no pack is mapped.
    pub fn bytes(&self) -> &'static [u8] {
        payload().get(self.off as usize..(self.off + self.len) as usize).unwrap_or(&[])
    }
}

/// Parse a pack header; returns (payload hash, payload length).
pub fn parse_header(h: &[u8]) -> Option<(u64, usize)> {
    if h.len() < HEADER_LEN || &h[..4] != b"KAST" || u32::from_le_bytes(h[4..8].try_into().ok()?) != VERSION {
        return None;
    }
    let hash = u64::from_le_bytes(h[8..16].try_into().ok()?);
    let len = u32::from_le_bytes(h[16..20].try_into().ok()?) as usize;
    Some((hash, len))
}

#[cfg(not(target_os = "espidf"))]
fn payload() -> &'static [u8] {
    static IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/assets.bin"));
    &IMAGE[HEADER_LEN..]
}

#[cfg(target_os = "espidf")]
fn payload() -> &'static [u8] {
    device::PAYLOAD.get().copied().unwrap_or(&[])
}

#[cfg(target_os = "espidf")]
pub use device::*;

#[cfg(target_os = "espidf")]
mod device {
    use std::ffi::CString;
    use std::sync::OnceLock;

    use anyhow::{anyhow, bail, Result};
    use esp_idf_svc::sys::*;

    use super::{parse_header, ASSETS_HASH, ASSETS_LEN, HEADER_LEN};

    pub const SLOTS: [&str; 2] = ["assets_0", "assets_1"];

    pub(super) static PAYLOAD: OnceLock<&'static [u8]> = OnceLock::new();
    static ACTIVE: OnceLock<&'static str> = OnceLock::new();

    pub fn partition(label: &str) -> Result<*const esp_partition_t> {
        let c = CString::new(label)?;
        let p = unsafe {
            esp_partition_find_first(esp_partition_type_t_ESP_PARTITION_TYPE_DATA, esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_ANY, c.as_ptr())
        };
        if p.is_null() {
            bail!("partition {label} not found");
        }
        Ok(p)
    }

    /// Header (hash, len) of the pack in `label`, if it holds a valid one.
    pub fn slot_header(label: &str) -> Option<(u64, usize)> {
        let p = partition(label).ok()?;
        let mut h = [0u8; HEADER_LEN];
        let err = unsafe { esp_partition_read(p, 0, h.as_mut_ptr().cast(), HEADER_LEN) };
        (err == ESP_OK).then(|| parse_header(&h)).flatten()
    }

    /// Find the slot holding this firmware's pack and memory-map it.
    pub fn init() -> Result<&'static str> {
        for label in SLOTS {
            let Some((hash, len)) = slot_header(label) else { continue };
            if hash != ASSETS_HASH || len != ASSETS_LEN {
                log::info!("assets: {label} holds pack {hash:016x}, want {ASSETS_HASH:016x}");
                continue;
            }
            let p = partition(label)?;
            let mut ptr: *const core::ffi::c_void = core::ptr::null();
            let mut handle: esp_partition_mmap_handle_t = 0;
            let err = unsafe {
                esp_partition_mmap(p, 0, HEADER_LEN + len, esp_partition_mmap_memory_t_ESP_PARTITION_MMAP_DATA, &mut ptr, &mut handle)
            };
            EspError::convert(err).map_err(|e| anyhow!("mmap {label}: {e}"))?;
            // Mapped for the life of the program; never unmapped.
            let image = unsafe { core::slice::from_raw_parts(ptr as *const u8, HEADER_LEN + len) };
            let _ = PAYLOAD.set(&image[HEADER_LEN..]);
            let _ = ACTIVE.set(label);
            log::info!("assets: using {label} ({len} bytes, pack {hash:016x})");
            return Ok(label);
        }
        bail!("no assets partition holds pack {ASSETS_HASH:016x}; flash target/.../assets.bin")
    }

    /// The slot an update should write to: the one not in use.
    pub fn inactive_slot() -> &'static str {
        match ACTIVE.get() {
            Some(&a) if a == SLOTS[0] => SLOTS[1],
            _ => SLOTS[0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_pack_header_matches_build() {
        static IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/assets.bin"));
        assert_eq!(parse_header(IMAGE), Some((ASSETS_HASH, ASSETS_LEN)));
        assert_eq!(parse_header(b"nope"), None);
    }

    #[test]
    fn blobs_resolve_on_host() {
        assert!(!crate::fonts::CLOCK_BIG.data.bytes().is_empty());
        assert!(!crate::sprites::nixie::TUBE.bytes().is_empty());
    }
}
