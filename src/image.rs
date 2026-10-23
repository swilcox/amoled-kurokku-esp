//! Showing QOI images: decode straight onto the panel, band by band. The
//! panel keeps the picture afterwards, so nothing is held in RAM. Pure —
//! host-testable.

use std::io::Read;

use anyhow::Result;

use crate::color::{to_wire, BLACK};
use crate::gfx::{Panel, Rect, Renderer, Solid, SCREEN};
use crate::qoi::QoiReader;

/// Where a `w x h` image lands: centered, cropped to the screen.
struct Placement {
    /// On-screen rectangle the visible part covers.
    dest: Rect,
    /// Image columns / rows cut off on the left / top.
    skip_x: usize,
    skip_y: u32,
}

fn placement(w: u32, h: u32) -> Placement {
    let (w, h) = (w as i32, h as i32);
    let (x0, y0) = ((SCREEN.width() - w) / 2, (SCREEN.height() - h) / 2);
    Placement {
        dest: Rect::from_size(x0.max(0), y0.max(0), w.min(SCREEN.width()), h.min(SCREEN.height())),
        skip_x: (-x0).max(0) as usize,
        skip_y: (-y0).max(0) as u32,
    }
}

/// Decode the QOI image from `reader` onto `panel`, centered (and cropped
/// if larger than the screen; on black if smaller). On a decode error the
/// image is left partially drawn.
pub fn show(renderer: &mut Renderer, panel: &mut impl Panel, reader: impl Read) -> Result<()> {
    let mut q = QoiReader::new(reader)?;
    let p = placement(q.width, q.height);
    if p.dest != SCREEN {
        renderer.render(panel, &Solid(BLACK), &[SCREEN])?;
    }
    for _ in 0..p.skip_y {
        q.read_row(0, &mut [])?;
    }
    let mut row = vec![0u16; p.dest.width() as usize];
    let black = to_wire(BLACK);
    renderer.stream(panel, p.dest, |band, buf| {
        let w = band.width() as usize;
        for y in band.y0..=band.y1 {
            let line = &mut buf[(y - band.y0) as usize * w..][..w];
            line.fill(black);
            // Alignment can widen the band a pixel past the image.
            if y < p.dest.y0 || y > p.dest.y1 {
                continue;
            }
            q.read_row(p.skip_x, &mut row)?;
            let off = (p.dest.x0 - band.x0) as usize;
            for (d, s) in line[off..off + row.len()].iter_mut().zip(&row) {
                *d = to_wire(*s);
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::rgb565;
    use crate::gfx::MemPanel;

    fn image(w: u32, h: u32) -> (Vec<u8>, Vec<u8>) {
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 77, 255])
            .collect();
        let qoi = qoi::encode_to_vec(&rgba, w, h).unwrap();
        (rgba, qoi)
    }

    fn at(rgba: &[u8], w: u32, x: u32, y: u32) -> u16 {
        let p = &rgba[((y * w + x) * 4) as usize..][..3];
        rgb565(p[0], p[1], p[2])
    }

    #[test]
    fn full_screen_image_maps_one_to_one() {
        let (rgba, qoi) = image(480, 480);
        let mut panel = MemPanel::default();
        show(&mut Renderer::new(480 * 32), &mut panel, &qoi[..]).unwrap();
        for (x, y) in [(0, 0), (479, 0), (123, 456), (479, 479)] {
            assert_eq!(panel.pixels[(y * 480 + x) as usize], at(&rgba, 480, x, y));
        }
    }

    #[test]
    fn small_image_is_centered_on_black() {
        let (rgba, qoi) = image(101, 51);
        let mut panel = MemPanel::default();
        panel.pixels.fill(0xFFFF);
        show(&mut Renderer::new(480 * 32), &mut panel, &qoi[..]).unwrap();
        let (x0, y0) = ((480 - 101) / 2, (480 - 51) / 2);
        assert_eq!(panel.pixels[(y0 * 480 + x0) as usize], at(&rgba, 101, 0, 0));
        assert_eq!(panel.pixels[((y0 + 50) * 480 + x0 + 100) as usize], at(&rgba, 101, 100, 50));
        assert_eq!(panel.pixels[0], BLACK);
        assert_eq!(panel.pixels[((y0 - 1) * 480 + x0) as usize], BLACK);
    }

    #[test]
    fn large_image_is_center_cropped() {
        let (rgba, qoi) = image(600, 520);
        let mut panel = MemPanel::default();
        show(&mut Renderer::new(480 * 32), &mut panel, &qoi[..]).unwrap();
        assert_eq!(panel.pixels[0], at(&rgba, 600, 60, 20));
        assert_eq!(panel.pixels[479 * 480 + 479], at(&rgba, 600, 539, 499));
    }
}
