//! Render a clock face to PNG on the host, without flashing the device.
//!
//!     just preview                      # 12:34:56, 24h -> target/preview/modern.png
//!     just preview 09:05:07 12h out.png

use std::fs::File;
use std::io::BufWriter;

use amoled_kurokku_esp::board::{LCD_H, LCD_W};
use amoled_kurokku_esp::clock::{modern::ModernFace, Face, LocalTime};
use amoled_kurokku_esp::gfx::{MemPanel, Renderer};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let time = args.first().map(String::as_str).unwrap_or("12:34:56");
    let format_24h = !args.iter().any(|a| a == "12h");
    let out = args
        .iter()
        .find(|a| a.ends_with(".png"))
        .cloned()
        .unwrap_or_else(|| "target/preview/modern.png".into());

    let hms: Vec<u8> = time.split(':').map(|p| p.parse()).collect::<Result<_, _>>()?;
    let [hour, minute, second] = hms[..] else { anyhow::bail!("time must be HH:MM:SS") };
    let t = LocalTime { year: 2026, month: 10, day: 22, weekday: 4, hour, minute, second };

    let mut face = ModernFace::new(format_24h);
    let mut panel = MemPanel::default();
    let rects = face.update(&t);
    Renderer::new(LCD_W as usize * 32).render(&mut panel, &face, &rects)?;

    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut enc = png::Encoder::new(BufWriter::new(File::create(&out)?), LCD_W as u32, LCD_H as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&panel.to_rgb8())?;
    println!("wrote {out}");
    Ok(())
}
