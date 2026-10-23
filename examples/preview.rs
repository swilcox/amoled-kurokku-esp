//! Render a widget to PNG on the host, without flashing the device.
//!
//!     just preview                               # clock 12:34:56, 24h
//!     just preview 09:05:07 12h out.png          # clock
//!     just preview message "72°F" [up] [small|medium|large] [@ms] [#rrggbb]
//!     just preview status "KUROKKU" "192.168.1.50"
//!
//! Output defaults to target/preview/<kind>.png. `@ms` renders a scrolling
//! message as it looks that many milliseconds in.

use std::fs::File;
use std::io::BufWriter;

use amoled_kurokku_esp::board::{LCD_H, LCD_W};
use amoled_kurokku_esp::clock::LocalTime;
use amoled_kurokku_esp::color::WHITE;
use amoled_kurokku_esp::gfx::{MemPanel, Renderer};
use amoled_kurokku_esp::protocol::{parse_color, scroll_px_per_ms};
use amoled_kurokku_esp::widget::{clock::Clock, message, status::Status, Step, Widget};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.iter().find(|a| a.ends_with(".png")).cloned();
    let words: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.ends_with(".png")).collect();
    let flag = |f: &str| words.iter().any(|w| *w == f);
    let at_ms: u32 = words.iter().find_map(|w| w.strip_prefix('@')?.parse().ok()).unwrap_or(0);

    let mut t = LocalTime { year: 2026, month: 10, day: 22, weekday: 4, hour: 12, minute: 34, second: 56 };
    let (kind, mut widget): (&str, Box<dyn Widget>) = match words.first().copied() {
        Some("message") => {
            let text = words.get(1).copied().unwrap_or("Hello, world!");
            let dir = message::Direction::parse(flag("up").then_some("up"));
            let size = ["small", "medium", "large"].into_iter().find(|s| flag(s));
            let color = words.iter().find_map(|w| w.starts_with('#').then(|| parse_color(w)).flatten());
            let font = message::font_for(size, dir);
            let m = message::Message::new(text, font, color.unwrap_or(WHITE), dir, scroll_px_per_ms(50), -1);
            ("message", Box::new(m))
        }
        Some("status") => {
            let title = words.get(1).copied().unwrap_or("KUROKKU");
            let detail = words.get(2).copied().unwrap_or("");
            ("status", Box::new(Status::new(title, detail, None)))
        }
        first => {
            if let Some(hms) = first.filter(|s| s.contains(':')) {
                let v: Vec<u8> = hms.split(':').map(|p| p.parse()).collect::<Result<_, _>>()?;
                let [h, m, s] = v[..] else { anyhow::bail!("time must be HH:MM:SS") };
                (t.hour, t.minute, t.second) = (h, m, s);
            }
            ("clock", Box::new(Clock::new(!flag("12h"), None)))
        }
    };

    // Tick from 0 to the requested time, rendering every step like the device.
    let mut panel = MemPanel::default();
    let mut r = Renderer::new(LCD_W as usize * 32);
    let mut ms = 0;
    loop {
        if let Step::Redraw(rects) = widget.tick(ms, &t) {
            r.render(&mut panel, &*widget, &rects)?;
        }
        if ms >= at_ms {
            break;
        }
        ms = (ms + widget.frame_ms().max(1)).min(at_ms);
    }

    let out = out.unwrap_or_else(|| format!("target/preview/{kind}.png"));
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
