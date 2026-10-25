//! Device <-> kurokku-esp-server wire format. Pure — host-testable.
//!
//! Same contract as led-kurokku-esp (`GET /api/v1/devices/{id}/instruction`),
//! with optional AMOLED-only fields that the LED firmware ignores.

use serde::Deserialize;

use crate::color::rgb565;

/// Reported as `display_type` on every poll.
pub const DISPLAY_TYPE: &str = "amoled480";

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Instruction {
    Clock {
        #[serde(default = "default_true")]
        format_24h: bool,
        /// Face name; only "modern" exists so far. Unknown names fall back.
        #[serde(default)]
        style: Option<String>,
    },
    Message {
        text: String,
        /// Milliseconds per LED-matrix column, as on led-kurokku-esp. Scaled
        /// so a message crosses this screen as fast as it crosses a 32-column
        /// matrix (see `scroll_px_per_ms`).
        #[serde(default = "default_scroll_speed")]
        scroll_speed_ms: u32,
        /// Times to show the message; -1 = until the next instruction.
        #[serde(default = "default_repeats")]
        repeats: i32,
        /// "left" (default, horizontal marquee) or "up" (vertical crawl).
        #[serde(default)]
        direction: Option<String>,
        /// "#RRGGBB"; default white.
        #[serde(default)]
        color: Option<String>,
        /// "small", "medium" or "large".
        #[serde(default)]
        size: Option<String>,
    },
    Ota {
        url: String,
    },
    /// Show a QOI image: `url` is http(s)://…, sd:/path/on/card.qoi, or a
    /// path relative to the server (its image library: /api/v1/images/…).
    /// Images larger than the screen are center-cropped; smaller ones are
    /// centered on black.
    Image {
        url: String,
    },
    /// Cycle through the .qoi images in a directory on the SD card.
    Slideshow {
        #[serde(default = "default_slideshow_path")]
        path: String,
        #[serde(default = "default_slideshow_interval")]
        interval_secs: u32,
    },
    /// One of `widget::anim::ANIMATIONS`, or "random"; runs until the next
    /// instruction (`duration_secs` is the playlist's business).
    Animation {
        #[serde(default)]
        animation: String,
    },
    /// Instruction types this display can't show (raw_pixel, raw_segment,
    /// ...). The device falls back to the clock.
    #[serde(other)]
    Unsupported,
}

fn default_true() -> bool {
    true
}
fn default_scroll_speed() -> u32 {
    50
}
fn default_repeats() -> i32 {
    1
}
fn default_slideshow_path() -> String {
    "sd:/images".into()
}
fn default_slideshow_interval() -> u32 {
    10
}

/// Remote config update delivered alongside an instruction. Omitted fields
/// are left unchanged; see led-kurokku-esp's docs/server-api.md.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct ConfigUpdate {
    #[serde(default)]
    pub syslog_host: Option<String>,
    #[serde(default)]
    pub tz: Option<String>,
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub ntp_server: Option<String>,
}

/// An instruction as sent: what to show, plus a sound to play once as it
/// appears (any instruction may carry one). `sound` is a WAV URL in the
/// same forms as image URLs: http(s)://, sd:/path, or server-relative.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct Cue {
    #[serde(flatten)]
    pub instruction: Instruction,
    #[serde(default)]
    pub sound: Option<String>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct ServerResponse {
    pub instruction: Option<Cue>,
    /// 0-15, as on the LED displays.
    pub brightness: Option<u8>,
    pub poll_interval_ms: Option<u64>,
    pub config: Option<ConfigUpdate>,
    /// Quiet hours: the screen should be off.
    #[serde(default)]
    pub display_off: bool,
}

/// Map server brightness (0-15) to the panel's 0-255 register along a
/// roughly perceptual curve. 0 is very dim, not off.
pub fn panel_brightness(level: u8) -> u8 {
    let l = level.min(15) as u32 + 1;
    (4 + 251 * l * l / 256) as u8
}

/// Scroll speed in pixels per millisecond for a `scroll_speed_ms` value.
pub fn scroll_px_per_ms(scroll_speed_ms: u32) -> f32 {
    const LED_COLUMNS: f32 = 32.0;
    crate::board::LCD_W as f32 / (LED_COLUMNS * scroll_speed_ms.max(1) as f32)
}

/// Parse "#RRGGBB" or "RRGGBB" into RGB565.
pub fn parse_color(s: &str) -> Option<u16> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(rgb565((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ServerResponse {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn parses_server_clock_and_message() {
        let r = parse(r#"{"instruction":{"type":"clock","format_24h":false,"duration_secs":30},"brightness":8,"poll_interval_ms":5000}"#);
        assert_eq!(r.instruction.map(|c| c.instruction), Some(Instruction::Clock { format_24h: false, style: None }));
        assert_eq!(r.brightness, Some(8));

        let r = parse(r#"{"instruction":{"type":"message","text":"72°F","scroll_speed_ms":50,"repeats":-1,"duration_secs":10}}"#);
        assert_eq!(
            r.instruction.map(|c| c.instruction),
            Some(Instruction::Message {
                text: "72°F".into(),
                scroll_speed_ms: 50,
                repeats: -1,
                direction: None,
                color: None,
                size: None,
            })
        );
    }

    #[test]
    fn amoled_fields_and_defaults() {
        let r = parse(r##"{"instruction":{"type":"message","text":"hi","direction":"up","color":"#ff8800","size":"large"}}"##);
        let Some(Instruction::Message { scroll_speed_ms, repeats, direction, color, size, .. }) = r.instruction.map(|c| c.instruction) else {
            panic!("not a message");
        };
        assert_eq!((scroll_speed_ms, repeats), (50, 1));
        assert_eq!(direction.as_deref(), Some("up"));
        assert_eq!(color.as_deref().and_then(parse_color), Some(rgb565(255, 136, 0)));
        assert_eq!(size.as_deref(), Some("large"));
    }

    #[test]
    fn unsupported_types_and_empty_response() {
        let r = parse(r#"{"instruction":{"type":"raw_pixel","data":[1,2,3]}}"#);
        assert_eq!(r.instruction.map(|c| c.instruction), Some(Instruction::Unsupported));
        let r = parse(r#"{"brightness":3}"#);
        assert!(!r.display_off);
        assert!(parse(r#"{"display_off":true}"#).display_off);
        assert_eq!(r.instruction.map(|c| c.instruction), None);
        let r = parse(r#"{"config":{"tz":"EST5EDT,M3.2.0,M11.1.0"}}"#);
        assert_eq!(r.config.unwrap().tz.as_deref(), Some("EST5EDT,M3.2.0,M11.1.0"));
    }

    #[test]
    fn any_instruction_can_carry_a_sound() {
        let r = parse(r#"{"instruction":{"type":"image","url":"/api/v1/images/fm.qoi","sound":"/api/v1/sounds/fm.wav"}}"#);
        let cue = r.instruction.unwrap();
        assert_eq!(cue.instruction, Instruction::Image { url: "/api/v1/images/fm.qoi".into() });
        assert_eq!(cue.sound.as_deref(), Some("/api/v1/sounds/fm.wav"));
        // Still fine without one, and on types this display can't show.
        assert_eq!(parse(r#"{"instruction":{"type":"clock"}}"#).instruction.unwrap().sound, None);
        let r = parse(r#"{"instruction":{"type":"raw_pixel","sound":"sd:/x.wav"}}"#).instruction.unwrap();
        assert_eq!((r.instruction, r.sound.as_deref()), (Instruction::Unsupported, Some("sd:/x.wav")));
    }

    #[test]
    fn animation() {
        let r = parse(r#"{"instruction":{"type":"animation","animation":"pong","duration_secs":30}}"#);
        assert_eq!(r.instruction.map(|c| c.instruction), Some(Instruction::Animation { animation: "pong".into() }));
    }

    #[test]
    fn image_and_slideshow() {
        let r = parse(r#"{"instruction":{"type":"image","url":"http://x/cat.qoi","duration_secs":30}}"#);
        assert_eq!(r.instruction.map(|c| c.instruction), Some(Instruction::Image { url: "http://x/cat.qoi".into() }));
        let r = parse(r#"{"instruction":{"type":"slideshow"}}"#);
        assert_eq!(r.instruction.map(|c| c.instruction), Some(Instruction::Slideshow { path: "sd:/images".into(), interval_secs: 10 }));
    }

    #[test]
    fn brightness_curve() {
        assert_eq!(panel_brightness(15), 255);
        assert_eq!(panel_brightness(200), 255);
        assert!(panel_brightness(0) >= 4 && panel_brightness(0) < 10);
        assert!((1..=15).all(|l| panel_brightness(l) > panel_brightness(l - 1)));
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#FFFFFF"), Some(0xFFFF));
        assert_eq!(parse_color("00ff00"), Some(0x07E0));
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn default_scroll_speed_crosses_screen_like_a_matrix() {
        // 50 ms/column on a 32-column matrix = 1.6 s per screen width.
        let ms_per_screen = 480.0 / scroll_px_per_ms(50);
        assert!((ms_per_screen - 1600.0).abs() < 1.0);
    }
}
