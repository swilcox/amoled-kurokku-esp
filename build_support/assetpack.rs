//! Build-time asset pack: fonts and face artwork, flashed to an `assets_N`
//! partition instead of being linked into the app (the app slot is 3 MB).
//!
//! Layout (little-endian):
//!   0  magic  b"KAST"
//!   4  u32    format version
//!   8  u64    FNV-1a hash of the payload
//!   16 u32    payload length
//!   20 ..32   reserved (zero)
//!   32 payload: blobs, each 4-byte aligned
//!
//! Generated code refers to blobs by `Blob { off, len }` (payload-relative).
//! The firmware embeds `ASSETS_HASH` and maps whichever slot matches it.

use std::path::Path;

pub const HEADER_LEN: usize = 32;
const VERSION: u32 = 1;

#[derive(Default)]
pub struct Pack {
    payload: Vec<u8>,
}

impl Pack {
    /// Append `bytes`; returns `Blob { off: .., len: .. }` source for codegen.
    pub fn add(&mut self, bytes: &[u8]) -> String {
        while self.payload.len() % 4 != 0 {
            self.payload.push(0);
        }
        let off = self.payload.len();
        self.payload.extend_from_slice(bytes);
        format!("crate::assets::Blob {{ off: {off}, len: {} }}", bytes.len())
    }

    /// Write `assets.bin` and `assets.rs` to `out_dir`; also copy the image
    /// next to the firmware binary (the profile dir) for flashing.
    pub fn finish(self, out_dir: &Path) {
        let hash = self.payload.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
        });
        let mut image = Vec::with_capacity(HEADER_LEN + self.payload.len());
        image.extend_from_slice(b"KAST");
        image.extend_from_slice(&VERSION.to_le_bytes());
        image.extend_from_slice(&hash.to_le_bytes());
        image.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        image.resize(HEADER_LEN, 0);
        image.extend_from_slice(&self.payload);
        std::fs::write(out_dir.join("assets.bin"), &image).unwrap();

        std::fs::write(
            out_dir.join("assets.rs"),
            format!(
                "pub const ASSETS_HASH: u64 = 0x{hash:016x};\npub const ASSETS_LEN: usize = {};\npub const HEADER_LEN: usize = {HEADER_LEN};\npub const VERSION: u32 = {VERSION};\n",
                self.payload.len()
            ),
        )
        .unwrap();

        // OUT_DIR is .../<profile>/build/<pkg>[-<hash>|/<hash>]/out.
        if let Some(profile) = out_dir.ancestors().find(|p| p.file_name().is_some_and(|n| n == "build")).and_then(Path::parent) {
            let _ = std::fs::write(profile.join("assets.bin"), &image);
        }
    }
}
