//! WAV headers, for playing sounds as they stream in. Pure — host-testable.
//!
//! Only plain 16-bit PCM (mono or stereo) is accepted; that's what the
//! audio path plays.

use std::io::Read;

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub channels: u16,
    pub sample_rate: u32,
    /// Bytes of sample data that follow the header.
    pub data_len: u32,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Read up to the start of the sample data, leaving `r` there.
pub fn read_header(r: &mut impl Read) -> Result<Format> {
    let mut riff = [0u8; 12];
    r.read_exact(&mut riff).context("WAV header")?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    loop {
        let mut chunk = [0u8; 8];
        r.read_exact(&mut chunk).context("WAV chunk")?;
        let len = u32_at(&chunk, 4);
        match &chunk[0..4] {
            b"fmt " => {
                let mut body = vec![0u8; len as usize];
                r.read_exact(&mut body).context("WAV fmt chunk")?;
                if body.len() < 16 {
                    bail!("short fmt chunk");
                }
                fmt = Some((u16_at(&body, 0), u16_at(&body, 2), u32_at(&body, 4), u16_at(&body, 14)));
            }
            b"data" => {
                let Some((tag, channels, sample_rate, bits)) = fmt else { bail!("WAV data before fmt") };
                // 1 = PCM; 0xFFFE = WAVE_FORMAT_EXTENSIBLE, which is PCM at 16 bits here.
                if !(tag == 1 || tag == 0xFFFE) || bits != 16 {
                    bail!("unsupported WAV: format {tag:#x}, {bits}-bit (need 16-bit PCM)");
                }
                if !(1..=2).contains(&channels) {
                    bail!("unsupported WAV: {channels} channels");
                }
                return Ok(Format { channels, sample_rate, data_len: len });
            }
            _ => {
                // Skip LIST, fact, etc. (chunks are padded to even length).
                std::io::copy(&mut r.take((len + (len & 1)) as u64), &mut std::io::sink())?;
            }
        }
    }
}

/// Turn little-endian 16-bit samples into stereo frames, scaled by
/// `gain` (Q8, 256 = unchanged): mono is copied to both channels.
pub fn to_stereo(pcm: &[u8], channels: u16, gain: i32, out: &mut Vec<u8>) {
    out.clear();
    let scale = |s: i16| ((s as i32 * gain) >> 8).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    for s in pcm.chunks_exact(2).map(|b| scale(i16::from_le_bytes([b[0], b[1]]))) {
        out.extend_from_slice(&s.to_le_bytes());
        if channels == 1 {
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(channels: u16, rate: u32, extra: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WAVE".to_vec();
        v.extend_from_slice(extra);
        v.extend_from_slice(b"fmt \x10\0\0\0\x01\0");
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * 2 * channels as u32).to_le_bytes());
        v.extend_from_slice(&(2 * channels).to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn reads_the_header_and_stops_at_the_samples() {
        let bytes = wav(1, 24000, b"LIST\x03\0\0\0abc\0", &[1, 0, 2, 0]);
        let mut r = &bytes[..];
        let f = read_header(&mut r).unwrap();
        assert_eq!(f, Format { channels: 1, sample_rate: 24000, data_len: 4 });
        assert_eq!(r, &[1, 0, 2, 0]);
    }

    #[test]
    fn rejects_what_it_cant_play() {
        assert!(read_header(&mut &b"RIFX\0\0\0\0WAVE"[..]).is_err());
        let mut eight_bit = wav(1, 8000, b"", &[]);
        eight_bit[34] = 8; // bits per sample
        assert!(read_header(&mut &eight_bit[..]).is_err());
    }

    #[test]
    fn mono_becomes_stereo_and_gain_saturates() {
        let mut out = Vec::new();
        to_stereo(&[0x00, 0x40, 0x00, 0xC0], 1, 512, &mut out);
        let s: Vec<i16> = out.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(s, [32767, 32767, -32768, -32768]);
        to_stereo(&[0x10, 0x00, 0x20, 0x00], 2, 256, &mut out);
        assert_eq!(out, [0x10, 0x00, 0x20, 0x00]);
    }
}
