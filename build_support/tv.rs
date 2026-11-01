//! Stations for the `static` animation: every `assets/tv/*.pgm` (160×160,
//! 8-bit grey, made by `tools/tv2pgm.py`), packed as raw rows.

use std::fmt::Write as _;
use std::path::Path;

use super::assetpack::Pack;

pub const SIZE: usize = 160;

/// Pixels of a binary 8-bit PGM, which must be SIZE×SIZE.
fn pgm_pixels(path: &Path) -> Vec<u8> {
    let data = std::fs::read(path).unwrap();
    // Header: magic, width, height, maxval, each followed by whitespace.
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while data[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while !data[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(std::str::from_utf8(&data[start..i]).unwrap().to_string());
    }
    let pixels = &data[i + 1..];
    assert!(
        fields == ["P5", &SIZE.to_string(), &SIZE.to_string(), "255"] && pixels.len() == SIZE * SIZE,
        "{}: want a {SIZE}x{SIZE} 8-bit binary PGM",
        path.display()
    );
    pixels.to_vec()
}

pub fn generate(manifest_dir: &Path, out_dir: &Path, pack: &mut Pack) {
    let dir = manifest_dir.join("assets/tv");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pgm"))
        .collect();
    files.sort();

    let mut blobs = Vec::new();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        blobs.push(pack.add(&pgm_pixels(f)));
    }
    let mut src = String::new();
    writeln!(src, "pub const SIZE: usize = {SIZE};").unwrap();
    writeln!(src, "/// Greyscale pictures, SIZE×SIZE bytes, row-major.\npub static PICTURES: [crate::assets::Blob; {}] = [{}];", blobs.len(), blobs.join(", ")).unwrap();
    std::fs::write(out_dir.join("tv.rs"), src).unwrap();
}
