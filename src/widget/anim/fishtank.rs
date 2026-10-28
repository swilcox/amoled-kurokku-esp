//! `fishtank`: a reef tank. A pair of clownfish potter about their
//! bubble-tip anemone while two yellow-and-black butterflyfish roam the
//! tank; seaweed sways, bubbles rise from an airstone, and light falls in
//! soft beams onto the sand and live rock.
//!
//! The fish are pixel-art sprites worked out once at startup. They turn by
//! narrowing to nothing and widening the other way round, and their tails
//! beat by foreshortening. Only what moved is redrawn: the fish, each
//! bubble, and a plant whenever its sway reaches another pixel.

use super::super::{Step, Widget};
use crate::board::{LCD_H, LCD_W};
use crate::clock::LocalTime;
use crate::color::{rgb565, to_wire};
use crate::gfx::{add565, blend565_8, Canvas, Rect, Scene, SCREEN};
use crate::rng::Rng;

const W: i32 = LCD_W as i32;
const H: i32 = LCD_H as i32;
/// Sand: mean top and the airstone's column.
const SAND_Y: i32 = 412;
const AIRSTONE_X: i32 = 182;
/// Light beams: slant (px across per 256 down) and lookup offset.
const BEAM_SLANT: i32 = 90;
const BEAM_OFF: i32 = 170;
const MAX_BUBBLES: usize = 14;
/// Tail foreshortening: steps, and how much narrower each makes it (/256).
const TAIL_STEPS: f32 = 7.0;
const TAIL_SQUASH: i32 = 12;
/// Sprite palette index 0 is clear; 1 is the outline.
const CLEAR: u8 = 0;
const OUTLINE: u8 = 1;

fn wire(r: u8, g: u8, b: u8) -> u16 {
    to_wire(rgb565(r, g, b))
}

fn hash(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^ h >> 12
}

/// Fill `[a, b]` of a row starting at column `x0`, clipped to it.
fn span(row: &mut [u16], x0: i32, a: i32, b: i32, color: u16) {
    let (a, b) = ((a - x0).max(0), (b - x0).min(row.len() as i32 - 1));
    if a <= b {
        row[a as usize..=b as usize].fill(color);
    }
}

/// `((x-cx)/rx)² + ((y-cy)/ry)²`: under 1 inside the ellipse.
fn ell(x: f32, y: f32, cx: f32, cy: f32, rx: f32, ry: f32) -> f32 {
    ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2)
}

/// A fish picture facing right, as palette indices.
struct Sprite {
    w: i32,
    h: i32,
    px: Vec<u8>,
    /// Wire order; index 0 unused (clear).
    palette: [u16; 16],
    /// Columns left of this are the tail, which beats.
    tail: i32,
    /// Cruising speed, px/s, and how hard the tail beats (0-1).
    speed: f32,
    beat: f32,
}

/// Rasterize `f` (pixel center to palette index) and outline it.
fn rasterize(w: i32, h: i32, f: impl Fn(f32, f32) -> u8) -> Vec<u8> {
    let (wu, hu) = (w as usize, h as usize);
    let src: Vec<u8> = (0..wu * hu).map(|i| f((i % wu) as f32 + 0.5, (i / wu) as f32 + 0.5)).collect();
    let clear = |x: i32, y: i32| x < 0 || y < 0 || x >= w || y >= h || src[(y * w + x) as usize] == CLEAR;
    (0..wu * hu)
        .map(|i| {
            let (x, y) = ((i % wu) as i32, (i / wu) as i32);
            let edge = clear(x - 1, y) || clear(x + 1, y) || clear(x, y - 1) || clear(x, y + 1);
            if src[i] != CLEAR && edge {
                OUTLINE
            } else {
                src[i]
            }
        })
        .collect()
}

/// Ocellaris clownfish: orange, three white bands edged in black, black
/// fin margins.
fn clownfish() -> Sprite {
    const ORANGE_DK: u8 = 2;
    const ORANGE: u8 = 3;
    const ORANGE_LT: u8 = 4;
    const WHITE_DK: u8 = 5;
    const WHITE: u8 = 6;
    const FIN: u8 = 7;
    const IRIS: u8 = 8;
    const PUPIL: u8 = 9;
    const SHINE: u8 = 10;
    let mut palette = [0; 16];
    for (i, c) in [
        (OUTLINE, (16, 10, 8)),
        (ORANGE_DK, (196, 72, 6)),
        (ORANGE, (244, 112, 20)),
        (ORANGE_LT, (255, 158, 60)),
        (WHITE_DK, (196, 200, 212)),
        (WHITE, (248, 248, 252)),
        (FIN, (228, 98, 14)),
        (IRIS, (236, 128, 36)),
        (PUPIL, (4, 4, 4)),
        (SHINE, (255, 255, 255)),
    ] {
        palette[i as usize] = wire(c.0, c.1, c.2);
    }
    let px = rasterize(80, 44, |x, y| {
        let dy = y - 22.0;
        let body = ell(x, y, 46.0, 22.0, 28.0, 13.5) < 1.0 || ((15.0..=26.0).contains(&x) && dy.abs() <= 5.5 + (x - 15.0) * 0.35);
        let tail = ell(x, y, 18.0, 22.0, 15.0, 14.0);
        let dorsal = ell(x, y, 42.0, 11.0, 21.0, 6.5);
        let anal = ell(x, y, 34.0, 33.5, 8.0, 5.0);
        let pelvic = ell(x, y, 53.0, 33.0, 5.0, 4.5);
        let tail = (x < 18.0 && tail < 1.0).then_some(tail);
        // Eye, then the pectoral fin over the flank.
        let e = ((x - 66.0).powi(2) + (y - 18.5).powi(2)).sqrt();
        if e < 3.7 {
            return if (x - 65.2).abs() < 0.8 && (y - 17.6).abs() < 0.8 {
                SHINE
            } else if e < 2.0 {
                PUPIL
            } else if e < 3.0 {
                IRIS
            } else {
                OUTLINE
            };
        }
        let pec = ell(x, y, 54.0, 27.0, 5.5, 3.2);
        if pec < 1.0 {
            return if pec > 0.6 { OUTLINE } else { ORANGE_LT };
        }
        // White bands with black edges: behind the eye, mid-body (bulging
        // forward, and up into the dorsal fin), and at the tail's root.
        let white = if dy > 6.0 { WHITE_DK } else { WHITE };
        let band = |c: f32, hw: f32| {
            let d = (x - c).abs();
            (d <= hw + 1.6).then_some(if d <= hw { white } else { OUTLINE })
        };
        if body {
            let mid = 39.0 + 4.5 * (1.0 - (dy / 14.0).powi(2)).max(0.0);
            if let Some(c) = band(60.5 + 0.02 * dy * dy, 3.5).or_else(|| band(mid, 3.5)).or_else(|| band(21.5, 2.2)) {
                return c;
            }
            let light = -dy / 13.5;
            return if light > 0.4 {
                ORANGE_LT
            } else if light < -0.45 {
                ORANGE_DK
            } else {
                ORANGE
            };
        }
        let fin = |e: f32, margin: f32| if e > margin { OUTLINE } else { FIN };
        if dorsal < 1.0 {
            return band(39.0 + 4.5, 3.5).unwrap_or_else(|| fin(dorsal, 0.55));
        }
        if let Some(t) = tail {
            return fin(t, 0.6);
        }
        if anal < 1.0 {
            return fin(anal, 0.5);
        }
        if pelvic < 1.0 {
            return fin(pelvic, 0.5);
        }
        CLEAR
    });
    Sprite { w: 80, h: 44, px, palette, tail: 18, speed: 34.0, beat: 1.0 }
}

/// A yellow butterflyfish: a lemon-yellow disc with a black eye band
/// edged in white, a black saddle, gold fins lined in black with pale
/// margins, and a pointed snout.
fn butterflyfish() -> Sprite {
    const YELLOW_DK: u8 = 2;
    const YELLOW: u8 = 3;
    const YELLOW_LT: u8 = 4;
    const BLACK: u8 = 5;
    const PALE: u8 = 6;
    const FIN: u8 = 7;
    const LINE: u8 = 8;
    const RING: u8 = 9;
    const SHINE: u8 = 10;
    let mut palette = [0; 16];
    for (i, c) in [
        (OUTLINE, (22, 16, 4)),
        (YELLOW_DK, (214, 160, 0)),
        (YELLOW, (255, 216, 30)),
        (YELLOW_LT, (255, 240, 110)),
        (BLACK, (14, 12, 12)),
        (PALE, (236, 236, 222)),
        (FIN, (236, 156, 14)),
        (LINE, (228, 178, 12)),
        (RING, (70, 64, 56)),
        (SHINE, (255, 255, 255)),
    ] {
        palette[i as usize] = wire(c.0, c.1, c.2);
    }
    let px = rasterize(72, 66, |x, y| {
        let dy = y - 33.0;
        let body = ell(x, y, 40.0, 33.0, 20.0, 18.0);
        let peduncle = (10.0..=24.0).contains(&x) && dy.abs() <= 4.5;
        let snout = (58.0..=66.0).contains(&x) && (y - 35.0).abs() <= 3.2 - (x - 58.0) * 0.22;
        // Dorsal fin arcing back over the top, anal fin under.
        let fin = if dy < 0.0 { ell(x, y, 31.0, 31.0, 20.0, 26.0) } else { ell(x, y, 31.0, 35.0, 18.0, 22.0) };
        let fin = (x > 13.0 && fin < 1.0).then_some(fin);
        let tail = (1.0..12.0).contains(&x) && dy.abs() <= 3.5 + (12.0 - x) * 0.85;
        let saddle = ell(x, y, 26.0, 16.0, 9.0, 8.0) < 1.0;
        // Eye inside the eye band.
        let e = ((x - 54.0).powi(2) + (y - 29.0).powi(2)).sqrt();
        if e < 3.2 {
            return if (x - 53.2).abs() < 0.8 && (y - 28.2).abs() < 0.8 {
                SHINE
            } else if e < 2.0 {
                BLACK
            } else {
                RING
            };
        }
        if body < 1.0 || peduncle {
            let d = x - (53.0 + dy * 0.15);
            if d.abs() <= 3.0 {
                return BLACK;
            }
            if d.abs() <= 4.2 {
                return PALE;
            }
            if saddle {
                return BLACK;
            }
            if ell(x, y, 45.0, 38.0, 5.0, 3.0) < 1.0 {
                return FIN; // pectoral
            }
            if body > 0.84 && fin.is_some() {
                return YELLOW_DK; // where the fins join
            }
            // Faint diagonal rows of scales.
            if (x as i32 + y as i32) % 6 == 0 && x < 48.0 {
                return LINE;
            }
            return if dy < -10.0 {
                YELLOW_LT
            } else if dy > 11.0 {
                YELLOW_DK
            } else {
                YELLOW
            };
        }
        if snout {
            return YELLOW;
        }
        if let Some(e) = fin {
            return if e > 0.88 {
                PALE
            } else if e > 0.76 || saddle {
                BLACK
            } else {
                FIN
            };
        }
        if tail {
            return if x >= 9.5 {
                BLACK
            } else if x < 4.0 {
                PALE
            } else {
                FIN
            };
        }
        CLEAR
    });
    Sprite { w: 72, h: 66, px, palette, tail: 12, speed: 28.0, beat: 0.7 }
}

/// A fish as drawn: top-left, width now (it narrows as it turns), facing
/// left, and how foreshortened its tail is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pose {
    x0: i32,
    y0: i32,
    w: i32,
    flip: bool,
    squash: i32,
}

struct Fish {
    sprite: usize,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// -1 facing left .. 1 facing right.
    facing: f32,
    target: (f32, f32),
    rest_until: u32,
    /// Where it picks places to swim to, and how long it lingers (ms).
    range: Rect,
    rest: (i32, i32),
    beat: f32,
    pose: Option<Pose>,
}

/// A frond of seaweed or an anemone tentacle, rooted at (x, y) and bending
/// over as `bend` (px at the tip).
struct Blade {
    x: i32,
    y: i32,
    /// Half-width by row up from the root.
    hw: Vec<u8>,
    lean: i32,
    amp: f32,
    /// Sway, radians per ms, and phase.
    rate: f32,
    phase: f32,
    /// Edge, body, middle and tip colors (wire), and how many rows of tip.
    colors: [u16; 4],
    tip: i32,
    bend: i32,
    /// Widest half-width.
    wmax: i32,
}

impl Blade {
    fn height(&self) -> i32 {
        self.hw.len() as i32
    }

    fn bounds(&self, bend: i32) -> Rect {
        let w = self.wmax + 1;
        Rect::new(self.x + bend.min(0) - w, self.y - self.height() + 1, self.x + bend.max(0) + w, self.y)
    }

    fn row(&self, y: i32, x0: i32, row: &mut [u16]) {
        let (r, h) = (self.y - y, self.height());
        if r < 0 || r >= h {
            return;
        }
        let hw = self.hw[r as usize] as i32;
        let cx = self.x + self.bend * r * r / (h * h);
        let [edge, body, mid, tip] = self.colors;
        let inner = if r >= h - self.tip { tip } else { body };
        span(row, x0, cx - hw, cx + hw, edge);
        span(row, x0, cx - hw + 1, cx + hw - 1, inner);
        if r < h - self.tip {
            span(row, x0, cx - hw / 3, cx + hw / 3, mid);
        }
    }
}

/// A rock (or the anemone's foot): an ellipse shaded lighter on top.
struct Lump {
    cx: i32,
    cy: i32,
    ry: i32,
    hw: Vec<i16>,
    /// Dark to light (wire), and pink coralline patches if any.
    shades: [u16; 8],
    spots: Option<[u16; 8]>,
}

impl Lump {
    fn new(cx: i32, cy: i32, rx: i32, ry: i32, shades: [u16; 8], spots: Option<[u16; 8]>) -> Self {
        let hw = (-ry..=ry).map(|dy| (rx as f32 * (1.0 - (dy as f32 / ry as f32).powi(2)).max(0.0).sqrt()) as i16).collect();
        Self { cx, cy, ry, hw, shades, spots }
    }

    fn row(&self, y: i32, x0: i32, row: &mut [u16]) {
        let dy = y - self.cy;
        if dy.abs() > self.ry {
            return;
        }
        let hw = self.hw[(dy + self.ry) as usize] as i32;
        let (a, b) = ((self.cx - hw).max(x0), (self.cx + hw).min(x0 + row.len() as i32 - 1));
        let base = 7 - (dy + self.ry) * 6 / (2 * self.ry);
        for x in a..=b {
            let h = hash(x, y);
            let mut l = base + (h & 3) as i32 - 1;
            if x - (self.cx - hw) < 2 || (self.cx + hw) - x < 2 {
                l -= 2;
            }
            let l = l.clamp(0, 7) as usize;
            let shades = match self.spots {
                Some(s) if hash(x >> 3, y >> 2) & 7 == 0 => s,
                _ => self.shades,
            };
            row[(x - x0) as usize] = shades[l];
        }
    }
}

struct Bubble {
    /// Position in 1/16 px; rise speed, px/s.
    x16: i32,
    y16: i32,
    rise: i32,
    r: i32,
    phase: f32,
    /// Center as drawn (with its wobble).
    at: Option<(i32, i32)>,
}

impl Bubble {
    fn bounds(r: i32, (x, y): (i32, i32)) -> Rect {
        Rect::new(x - r, y - r, x + r, y + r)
    }
}

pub struct Fishtank {
    rng: Rng,
    sprites: [Sprite; 2],
    fish: Vec<Fish>,
    weeds: Vec<Blade>,
    tentacles: Vec<Blade>,
    rocks: Vec<Lump>,
    bubbles: Vec<Bubble>,
    next_bubble: u32,
    /// Water by row and beam brightness (0-3), wire order.
    water: Vec<[u16; 4]>,
    beams: Vec<u8>,
    sand_top: Vec<i16>,
    sand: [u16; 8],
    start_ms: Option<u32>,
    last_ms: Option<u32>,
    first: bool,
}

/// A ramp of 8 shades from `dark` to `light`, wire order.
fn shades(dark: (f32, f32, f32), light: (f32, f32, f32)) -> [u16; 8] {
    core::array::from_fn(|i| {
        let k = i as f32 / 7.0;
        let ch = |a: f32, b: f32| (a + (b - a) * k) as u8;
        wire(ch(dark.0, light.0), ch(dark.1, light.1), ch(dark.2, light.2))
    })
}

impl Fishtank {
    pub fn new(mut rng: Rng) -> Self {
        let sand_top: Vec<i16> = (0..W)
            .map(|x| {
                let x = x as f32;
                (SAND_Y as f32 + 6.0 * (x / 48.0 + 1.0).sin() + 2.5 * (x / 15.0).sin()) as i16
            })
            .collect();
        // Water: teal near the surface to deep blue, beams fading with depth.
        let water = (0..H)
            .map(|y| {
                let k = y as f32 / SAND_Y as f32;
                let base = (14.0 + (2.0 - 14.0) * k, 84.0 + (16.0 - 84.0) * k, 126.0 + (44.0 - 126.0) * k);
                let beam = (1.0 - k).max(0.0).powf(1.3) * 9.0;
                let surface = if y < 4 { 18.0 } else { 0.0 };
                core::array::from_fn(|l| {
                    let add = l as f32 * beam + surface;
                    wire((base.0 + add * 0.8) as u8, (base.1 + add * 1.2) as u8, (base.2 + add * 1.1) as u8)
                })
            })
            .collect();
        let beams = (0..W + BEAM_OFF + 20)
            .map(|u| {
                let u = u - BEAM_OFF;
                [(40, 36), (175, 20), (300, 46), (420, 16)]
                    .iter()
                    .map(|&(c, w)| {
                        let d = (u - c).abs();
                        if d < w / 2 {
                            3
                        } else if d < w / 2 + 6 {
                            2
                        } else if d < w / 2 + 14 {
                            1
                        } else {
                            0
                        }
                    })
                    .max()
                    .unwrap_or(0)
            })
            .collect();

        let rock = shades((34.0, 30.0, 34.0), (122.0, 112.0, 104.0));
        let coralline = Some(shades((60.0, 22.0, 52.0), (196.0, 92.0, 150.0)));
        let ground = |x: i32| sand_top[x as usize] as i32;
        let rocks = vec![
            Lump::new(70, ground(70) + 10, 52, 30, rock, coralline),
            Lump::new(330, ground(330) + 6, 74, 32, rock, coralline),
            Lump::new(204, ground(204) + 18, 18, 10, rock, None),
            Lump::new(440, ground(440) + 22, 26, 13, rock, None),
            Lump::new(AIRSTONE_X, ground(AIRSTONE_X) + 2, 7, 4, shades((40.0, 40.0, 44.0), (110.0, 110.0, 120.0)), None),
            // The anemone's foot, on the big rock.
            Lump::new(330, ground(330) - 22, 34, 10, shades((70.0, 20.0, 50.0), (170.0, 70.0, 120.0)), None),
        ];

        let green = [wire(10, 44, 20), wire(30, 118, 44), wire(76, 170, 72), 0];
        let olive = [wire(30, 40, 8), wire(92, 116, 26), wire(150, 166, 52), 0];
        let mut weeds = Vec::new();
        for &(x, h, w, lean, colors) in &[
            (24, 250, 6.0, -14, green),
            (48, 300, 7.0, 10, green),
            (78, 210, 5.0, 22, olive),
            (104, 160, 5.0, -8, green),
            (238, 120, 4.0, 10, olive),
            (432, 270, 7.0, -18, green),
            (458, 200, 5.0, 6, olive),
        ] {
            let hw = (0..h)
                .map(|r| {
                    let s = r as f32 / h as f32;
                    (1.0 + w * (1.0 - s * s) * (0.8 + 0.2 * (s * 22.0).sin())) as u8
                })
                .collect();
            let (amp, rate) = (rng.range(8, 15) as f32, rng.range(100, 160) as f32 / 100_000.0);
            let phase = rng.range(0, 628) as f32 / 100.0;
            weeds.push(Blade { x, y: ground(x) + 4, hw, lean, amp, rate, phase, colors, tip: 0, bend: lean, wmax: 0 });
        }

        // Bubble-tip anemone: tentacles fanned out from its foot, each with
        // a pale bulb.
        let mut tentacles = Vec::new();
        let foot = ground(330) - 26;
        let pink = [wire(110, 36, 78), wire(206, 104, 150), wire(232, 144, 182), wire(250, 206, 226)];
        for i in 0..17 {
            let x = 300 + i * 60 / 16 + rng.range(-2, 3);
            let h = rng.range(34, 52);
            const R: i32 = 4;
            let hw = (0..h)
                .map(|r| {
                    let into = r - (h - R);
                    if into > -R {
                        ((R * R - into * into).max(0) as f32).sqrt().round().max(1.0) as u8
                    } else {
                        2 + (r < 8) as u8
                    }
                })
                .collect();
            let lean = (x - 330) * 3 / 2 + rng.range(-6, 7);
            let (amp, rate) = (rng.range(3, 6) as f32, rng.range(150, 230) as f32 / 100_000.0);
            let phase = rng.range(0, 628) as f32 / 100.0;
            tentacles.push(Blade { x, y: foot + rng.range(-3, 3), hw, lean, amp, rate, phase, colors: pink, tip: 2 * R, bend: lean, wmax: 0 });
        }

        for b in weeds.iter_mut().chain(tentacles.iter_mut()) {
            b.wmax = *b.hw.iter().max().unwrap_or(&0) as i32;
        }

        let mut fish = Vec::new();
        let anemone = Rect::new(250, 300, 410, foot - 30);
        let tank = Rect::new(70, 70, W - 70, 320);
        for (sprite, range, rest, x, y) in
            [(0, anemone, (600, 2500), 280, 330), (0, anemone, (400, 2000), 380, 350), (1, tank, (0, 800), 120, 140), (1, tank, (0, 800), 160, 190)]
        {
            let (x, y) = (x as f32, y as f32);
            fish.push(Fish {
                sprite,
                x,
                y,
                vx: 0.0,
                vy: 0.0,
                facing: if rng.below(2) == 0 { 1.0 } else { -1.0 },
                target: (x, y),
                rest_until: 0,
                range,
                rest,
                beat: rng.range(0, 628) as f32 / 100.0,
                pose: None,
            });
        }

        Self {
            rng,
            sprites: [clownfish(), butterflyfish()],
            fish,
            weeds,
            tentacles,
            rocks,
            bubbles: Vec::new(),
            next_bubble: 0,
            water,
            beams,
            sand_top,
            sand: shades((44.0, 36.0, 26.0), (178.0, 156.0, 112.0)),
            start_ms: None,
            last_ms: None,
            first: true,
        }
    }

    fn sprite_pose(&self, f: &Fish) -> Pose {
        let s = &self.sprites[f.sprite];
        let w = ((s.w as f32 * f.facing.abs()).round() as i32).max(4);
        let squash = (f.beat.sin().abs() * TAIL_STEPS * s.beat).round() as i32;
        Pose { x0: f.x.round() as i32 - w / 2, y0: f.y.round() as i32 - s.h / 2, w, flip: f.facing < 0.0, squash }
    }

    fn pose_rect(&self, f: &Fish, p: &Pose) -> Rect {
        Rect::from_size(p.x0, p.y0, p.w, self.sprites[f.sprite].h)
    }

    /// Swim `f` towards where it's going for `dt` seconds.
    fn swim(&mut self, i: usize, dt: f32, t: u32) {
        let f = &mut self.fish[i];
        let speed = self.sprites[f.sprite].speed;
        let (dx, dy) = (f.target.0 - f.x, f.target.1 - f.y);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 8.0 && t >= f.rest_until {
            // Arrived: linger a moment, then off somewhere else.
            let r = f.range;
            f.target = (self.rng.range(r.x0, r.x1) as f32, self.rng.range(r.y0, r.y1) as f32);
            f.rest_until = t + self.rng.range(f.rest.0, f.rest.1 + 1) as u32;
        }
        let (want_vx, want_vy) = if t < f.rest_until || dist < 8.0 {
            (0.0, 0.0)
        } else {
            let s = speed * (dist / 40.0).min(1.0) / dist;
            (dx * s, dy * s * 0.6)
        };
        let k = (dt * 1.5).min(1.0);
        f.vx += (want_vx - f.vx) * k;
        f.vy += (want_vy - f.vy) * k;
        f.x += f.vx * dt;
        f.y += f.vy * dt;
        let heading = if f.vx > 3.0 {
            1.0
        } else if f.vx < -3.0 {
            -1.0
        } else {
            f.facing.signum()
        };
        f.facing += (heading - f.facing).clamp(-dt * 3.5, dt * 3.5);
        let pace = (f.vx * f.vx + f.vy * f.vy).sqrt();
        f.beat = (f.beat + dt * (3.0 + pace * 0.18)) % std::f32::consts::TAU;
    }

    fn background(&self, y: i32, x0: i32, row: &mut [u16]) {
        let water = &self.water[y as usize];
        let u0 = x0 - (y * BEAM_SLANT >> 8) + BEAM_OFF;
        for (i, px) in row.iter_mut().enumerate() {
            let x = x0 + i as i32;
            let top = self.sand_top[x as usize] as i32;
            *px = if y < top {
                water[self.beams[(u0 + i as i32) as usize] as usize]
            } else {
                // Lighter along the crest, darker deeper down, grainy.
                let d = y - top;
                let base = if d < 2 {
                    7
                } else if d < 5 {
                    6
                } else {
                    5 - (d / 22).min(3)
                };
                let l = base + (hash(x, y) >> 29) as i32 - 3;
                self.sand[l.clamp(0, 7) as usize]
            };
        }
    }

    fn bubble_row(b: &Bubble, y: i32, x0: i32, row: &mut [u16]) {
        let Some((cx, cy)) = b.at else { return };
        let dy = y - cy;
        if dy.abs() > b.r {
            return;
        }
        let hw = (0..=b.r).rev().find(|k| k * k + dy * dy <= b.r * b.r + b.r / 2).unwrap_or(0);
        let rim = rgb565(190, 228, 255);
        for x in (cx - hw).max(x0)..=(cx + hw).min(x0 + row.len() as i32 - 1) {
            let px = &mut row[(x - x0) as usize];
            let bg = to_wire(*px);
            let dx = x - cx;
            let c = if b.r >= 3 && dx == -b.r / 2 && dy == -b.r / 2 {
                blend565_8(bg, rgb565(255, 255, 255), 230)
            } else if dx.abs() == hw || dy.abs() == b.r {
                blend565_8(bg, rim, 150)
            } else {
                add565(bg, rgb565(8, 16, 24))
            };
            *px = to_wire(c);
        }
    }

    fn fish_row(&self, f: &Fish, p: &Pose, y: i32, x0: i32, row: &mut [u16]) {
        let s = &self.sprites[f.sprite];
        let sy = y - p.y0;
        if sy < 0 || sy >= s.h {
            return;
        }
        let line = &s.px[(sy * s.w) as usize..][..s.w as usize];
        let scale = 256 - p.squash * TAIL_SQUASH;
        for x in p.x0.max(x0)..=(p.x0 + p.w - 1).min(x0 + row.len() as i32 - 1) {
            let mut sx = (x - p.x0) * s.w / p.w;
            if p.flip {
                sx = s.w - 1 - sx;
            }
            if sx < s.tail {
                // The tail, foreshortened as it sweeps to one side.
                sx = s.tail - (s.tail - sx) * 256 / scale;
                if sx < 0 {
                    continue;
                }
            }
            let idx = line[sx as usize];
            if idx != CLEAR {
                row[(x - x0) as usize] = s.palette[idx as usize];
            }
        }
    }
}

impl Widget for Fishtank {
    fn tick(&mut self, now_ms: u32, _local: &LocalTime) -> Step {
        let t = now_ms.wrapping_sub(*self.start_ms.get_or_insert(now_ms));
        let dt_ms = now_ms.wrapping_sub(self.last_ms.unwrap_or(now_ms)).min(100);
        self.last_ms = Some(now_ms);
        let dt = dt_ms as f32 / 1000.0;
        let mut dirty = Vec::new();

        for i in 0..self.fish.len() {
            self.swim(i, dt, t);
            let pose = self.sprite_pose(&self.fish[i]);
            let f = &self.fish[i];
            if f.pose != Some(pose) {
                let new = self.pose_rect(f, &pose);
                dirty.push(f.pose.map_or(new, |p| self.pose_rect(f, &p).union(&new)));
                self.fish[i].pose = Some(pose);
            }
        }

        for b in self.weeds.iter_mut().chain(self.tentacles.iter_mut()) {
            let bend = b.lean + (b.amp * (t as f32 * b.rate + b.phase).sin()).round() as i32;
            if bend != b.bend {
                dirty.push(b.bounds(b.bend).union(&b.bounds(bend)));
                b.bend = bend;
            }
        }

        if t >= self.next_bubble && self.bubbles.len() < MAX_BUBBLES {
            let r = self.rng.range(2, 6);
            let y = self.sand_top[AIRSTONE_X as usize] as i32 - 4;
            let rise = self.rng.range(40, 64);
            let phase = self.rng.range(0, 628) as f32 / 100.0;
            self.bubbles.push(Bubble { x16: AIRSTONE_X * 16, y16: y * 16, rise, r, phase, at: None });
            // Now and then a quick string of them.
            self.next_bubble = t + if self.rng.below(4) == 0 { 90 } else { self.rng.range(300, 900) as u32 };
        }
        for b in &mut self.bubbles {
            b.y16 -= b.rise * 16 * dt_ms as i32 / 1000;
            b.phase += dt * 5.0;
            let wobble = (2.5 * b.phase.sin()).round() as i32;
            let at = Some(((b.x16 >> 4) + wobble, b.y16 >> 4));
            if at != b.at {
                let new = Bubble::bounds(b.r, at.unwrap());
                dirty.push(b.at.map_or(new, |old| Bubble::bounds(b.r, old).union(&new)));
                b.at = at;
            }
        }
        self.bubbles.retain(|b| b.at.is_some_and(|(_, y)| y >= -b.r - 1));

        let first = std::mem::replace(&mut self.first, false);
        Step::Redraw(if first { vec![SCREEN] } else { dirty })
    }

    fn invalidate(&mut self) {
        self.first = true;
    }

    fn frame_ms(&self) -> u32 {
        30
    }
}

impl Scene for Fishtank {
    fn draw(&self, c: &mut Canvas) {
        let a = c.area();
        // Back to front: water and sand, seaweed, rocks, the anemone,
        // bubbles, fish.
        let weeds: Vec<&Blade> = self.weeds.iter().filter(|b| b.bounds(b.bend).intersect(&a).is_some()).collect();
        let tentacles: Vec<&Blade> = self.tentacles.iter().filter(|b| b.bounds(b.bend).intersect(&a).is_some()).collect();
        let fish: Vec<(&Fish, Pose)> =
            self.fish.iter().filter_map(|f| f.pose.filter(|p| self.pose_rect(f, p).intersect(&a).is_some()).map(|p| (f, p))).collect();
        c.rows(a, |y, x0, row| {
            self.background(y, x0, row);
            for b in &weeds {
                b.row(y, x0, row);
            }
            for r in &self.rocks {
                r.row(y, x0, row);
            }
            for b in &tentacles {
                b.row(y, x0, row);
            }
            for b in &self.bubbles {
                Self::bubble_row(b, y, x0, row);
            }
            for (f, p) in &fish {
                self.fish_row(f, p, y, x0, row);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::check_incremental;
    use super::*;

    #[test]
    fn sprites_have_their_markings() {
        for (s, colors) in [(clownfish(), 2..=10u8), (butterflyfish(), 2..=10u8)] {
            assert_eq!(s.px.len(), (s.w * s.h) as usize);
            for c in colors {
                assert!(s.px.contains(&c), "color {c} unused");
            }
            // Clear all round the edge.
            assert!((0..s.w).all(|x| s.px[x as usize] == CLEAR && s.px[((s.h - 1) * s.w + x) as usize] == CLEAR));
        }
    }

    #[test]
    fn fish_turn_by_narrowing() {
        let mut tank = Fishtank::new(Rng::new(3));
        tank.fish[0].facing = 1.0;
        tank.fish[0].target = (tank.fish[0].x - 150.0, tank.fish[0].y);
        tank.fish[0].rest_until = 0;
        let mut widths = Vec::new();
        for i in 0..60 {
            tank.swim(0, 0.03, 10_000 + i * 30);
            widths.push(tank.sprite_pose(&tank.fish[0]));
        }
        assert!(widths.iter().any(|p| p.w < 20));
        let last = widths.last().unwrap();
        assert!(last.flip && last.w == 80);
    }

    #[test]
    fn incremental_redraws_match() {
        let t = LocalTime::default();
        check_incremental(Box::new(Fishtank::new(Rng::new(5))), (0..300u32).map(|i| (i * 30, t)));
    }
}
