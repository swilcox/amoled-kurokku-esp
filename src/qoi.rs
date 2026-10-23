//! Streaming QOI ("Quite OK Image") decoder. Pure — host-testable.
//!
//! Decodes row by row from any `std::io::Read`, so a 480x480 image can go
//! from the network or SD card straight to the panel without ever being
//! held in RAM. See <https://qoiformat.org/qoi-specification.pdf>.

use std::io::Read;

use anyhow::{bail, Result};

use crate::color::rgb565;

const OP_INDEX: u8 = 0x00;
const OP_DIFF: u8 = 0x40;
const OP_LUMA: u8 = 0x80;
const OP_RUN: u8 = 0xC0;
const OP_RGB: u8 = 0xFE;
const OP_RGBA: u8 = 0xFF;
const MASK_2: u8 = 0xC0;

/// Largest image accepted (guards against garbage headers).
const MAX_DIM: u32 = 4096;

pub struct QoiReader<R: Read> {
    r: R,
    buf: [u8; 512],
    pos: usize,
    len: usize,
    pub width: u32,
    pub height: u32,
    px: [u8; 4],
    index: [[u8; 4]; 64],
    run: u32,
    rows_left: u32,
}

impl<R: Read> QoiReader<R> {
    /// Read and validate the 14-byte header.
    pub fn new(r: R) -> Result<Self> {
        let mut q = Self {
            r,
            buf: [0; 512],
            pos: 0,
            len: 0,
            width: 0,
            height: 0,
            px: [0, 0, 0, 255],
            index: [[0; 4]; 64],
            run: 0,
            rows_left: 0,
        };
        let mut h = [0u8; 14];
        for b in h.iter_mut() {
            *b = q.byte()?;
        }
        if &h[..4] != b"qoif" {
            bail!("not a QOI image");
        }
        q.width = u32::from_be_bytes(h[4..8].try_into()?);
        q.height = u32::from_be_bytes(h[8..12].try_into()?);
        if q.width == 0 || q.height == 0 || q.width > MAX_DIM || q.height > MAX_DIM {
            bail!("bad QOI size {}x{}", q.width, q.height);
        }
        q.rows_left = q.height;
        Ok(q)
    }

    fn byte(&mut self) -> Result<u8> {
        if self.pos == self.len {
            self.len = self.r.read(&mut self.buf)?;
            self.pos = 0;
            if self.len == 0 {
                bail!("QOI data truncated");
            }
        }
        self.pos += 1;
        Ok(self.buf[self.pos - 1])
    }

    fn next_pixel(&mut self) -> Result<[u8; 4]> {
        if self.run > 0 {
            self.run -= 1;
            return Ok(self.px);
        }
        let b = self.byte()?;
        match b {
            OP_RGB => {
                self.px[0] = self.byte()?;
                self.px[1] = self.byte()?;
                self.px[2] = self.byte()?;
            }
            OP_RGBA => {
                for i in 0..4 {
                    self.px[i] = self.byte()?;
                }
            }
            _ => match b & MASK_2 {
                OP_INDEX => self.px = self.index[b as usize],
                OP_DIFF => {
                    self.px[0] = self.px[0].wrapping_add((b >> 4 & 3).wrapping_sub(2));
                    self.px[1] = self.px[1].wrapping_add((b >> 2 & 3).wrapping_sub(2));
                    self.px[2] = self.px[2].wrapping_add((b & 3).wrapping_sub(2));
                }
                OP_LUMA => {
                    let b2 = self.byte()?;
                    let dg = (b & 0x3F).wrapping_sub(32);
                    self.px[0] = self.px[0].wrapping_add(dg.wrapping_add(b2 >> 4).wrapping_sub(8));
                    self.px[1] = self.px[1].wrapping_add(dg);
                    self.px[2] = self.px[2].wrapping_add(dg.wrapping_add(b2 & 0x0F).wrapping_sub(8));
                }
                _ => {
                    debug_assert_eq!(b & MASK_2, OP_RUN);
                    self.run = (b & 0x3F) as u32; // this pixel plus `run` more
                }
            },
        }
        let [r, g, b, a] = self.px;
        let h = (r as usize * 3 + g as usize * 5 + b as usize * 7 + a as usize * 11) % 64;
        self.index[h] = self.px;
        Ok(self.px)
    }

    /// Decode the next row into `out` as RGB565 (alpha composited onto
    /// black). `out` receives `min(out.len(), width)` pixels starting at
    /// column `skip`; the rest of the row is consumed and dropped.
    pub fn read_row(&mut self, skip: usize, out: &mut [u16]) -> Result<()> {
        if self.rows_left == 0 {
            bail!("no more rows");
        }
        self.rows_left -= 1;
        let mut n = 0;
        for x in 0..self.width as usize {
            let [r, g, b, a] = self.next_pixel()?;
            if x >= skip && n < out.len() {
                out[n] = if a == 255 {
                    rgb565(r, g, b)
                } else {
                    let m = |c: u8| (c as u16 * a as u16 / 255) as u8;
                    rgb565(m(r), m(g), m(b))
                };
                n += 1;
            }
        }
        Ok(())
    }

    pub fn rows_left(&self) -> u32 {
        self.rows_left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test image exercising every op: gradients (DIFF/LUMA), repeats
    /// (RUN/INDEX), jumps (RGB) and alpha (RGBA).
    fn sample(w: u32, h: u32) -> Vec<u8> {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let (r, g, b, a) = match (x / 8 + y / 8) % 4 {
                    0 => ((x * 2) as u8, (y * 2) as u8, 128, 255),
                    1 => (255, 0, 0, 255),
                    2 => ((x * 37 % 251) as u8, (y * 53 % 241) as u8, ((x ^ y) * 7) as u8, 255),
                    _ => (200, 100, 50, (x * 8 % 256) as u8),
                };
                px.extend_from_slice(&[r, g, b, a]);
            }
        }
        px
    }

    #[test]
    fn decodes_reference_encoder_output() {
        let (w, h) = (61, 37);
        let rgba = sample(w, h);
        let encoded = qoi::encode_to_vec(&rgba, w, h).unwrap();
        let mut q = QoiReader::new(&encoded[..]).unwrap();
        assert_eq!((q.width, q.height), (w, h));
        let mut row = vec![0u16; w as usize];
        for y in 0..h as usize {
            q.read_row(0, &mut row).unwrap();
            for x in 0..w as usize {
                let p = &rgba[(y * w as usize + x) * 4..][..4];
                let m = |c: u8| (c as u16 * p[3] as u16 / 255) as u8;
                assert_eq!(row[x], rgb565(m(p[0]), m(p[1]), m(p[2])), "({x}, {y})");
            }
        }
        assert_eq!(q.rows_left(), 0);
        assert!(q.read_row(0, &mut row).is_err());
    }

    #[test]
    fn crops_columns() {
        let (w, h) = (20, 2);
        let rgba = sample(w, h);
        let encoded = qoi::encode_to_vec(&rgba, w, h).unwrap();
        let mut q = QoiReader::new(&encoded[..]).unwrap();
        let mut row = vec![0u16; 5];
        q.read_row(10, &mut row).unwrap();
        let p = &rgba[10 * 4..][..4];
        assert_eq!(row[0], rgb565(p[0], p[1], p[2]));
        // The dropped columns were still consumed: row 2 decodes correctly.
        q.read_row(0, &mut row).unwrap();
        let p = &rgba[w as usize * 4..][..4];
        assert_eq!(row[0], rgb565(p[0], p[1], p[2]));
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert!(QoiReader::new(&b"not an image at all"[..]).is_err());
        let rgba = sample(16, 16);
        let encoded = qoi::encode_to_vec(&rgba, 16, 16).unwrap();
        let mut q = QoiReader::new(&encoded[..40]).unwrap();
        let mut row = vec![0u16; 16];
        assert!((0..16).try_for_each(|_| q.read_row(0, &mut row)).is_err());
    }
}
