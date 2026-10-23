//! RGB565 helpers. Pure — host-testable.

/// Pack 8-bit RGB into RGB565.
pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

/// The panel expects RGB565 MSB-first, but buffers are little-endian `u16`s,
/// so pixel data is stored byte-swapped ("wire order").
pub const fn to_wire(c: u16) -> u16 {
    c.swap_bytes()
}

pub const BLACK: u16 = rgb565(0, 0, 0);
pub const WHITE: u16 = rgb565(255, 255, 255);
pub const RED: u16 = rgb565(255, 0, 0);
pub const GREEN: u16 = rgb565(0, 255, 0);
pub const BLUE: u16 = rgb565(0, 0, 255);
pub const YELLOW: u16 = rgb565(255, 255, 0);
pub const CYAN: u16 = rgb565(0, 255, 255);
pub const MAGENTA: u16 = rgb565(255, 0, 255);
pub const ORANGE: u16 = rgb565(255, 128, 0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries() {
        assert_eq!(RED, 0xF800);
        assert_eq!(GREEN, 0x07E0);
        assert_eq!(BLUE, 0x001F);
        assert_eq!(WHITE, 0xFFFF);
    }

    #[test]
    fn wire_order_is_msb_first() {
        assert_eq!(to_wire(0xF800).to_le_bytes(), [0xF8, 0x00]);
    }
}
