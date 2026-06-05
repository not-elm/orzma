//! `AColor` → `RgbaColor` conversion (xterm 256-color palette).
//!
//! Layout:
//!   0..=15   — 16 named ANSI base colors (xterm defaults)
//!   16..=231 — 6×6×6 RGB cube, channel ramp `[0, 95, 135, 175, 215, 255]`
//!   232..=255 — 24 grayscale ramp from value 8 to 238 in steps of 10
//!
//! Default fg / bg (`AColor::Named(Foreground | Background)`) maps to
//! white / black sentinels (provisional until theme support is added).

use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};
use serde::{Deserialize, Serialize};

/// sRGB 8-bit/channel color (wire representation). A flat 4-byte form
/// that avoids `bevy::Color`'s tagged enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbaColor {
    /// Red channel (sRGB).
    pub r: u8,
    /// Green channel (sRGB).
    pub g: u8,
    /// Blue channel (sRGB).
    pub b: u8,
    /// Alpha channel. Currently always 255 (opaque).
    pub a: u8,
}

impl RgbaColor {
    /// Constructs an opaque sRGB color.
    pub const fn srgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Opaque white.
    pub const WHITE: Self = Self::srgb(255, 255, 255);
    /// Opaque black.
    pub const BLACK: Self = Self::srgb(0, 0, 0);
}

/// Channel ramp for the 6×6×6 cube portion of the xterm 256 palette.
const CUBE_RAMP: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// 16 named ANSI base colors as xterm defaults (R, G, B tuples).
const ANSI_16: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (205, 0, 0),
    (0, 205, 0),
    (205, 205, 0),
    (0, 0, 238),
    (205, 0, 205),
    (0, 205, 205),
    (229, 229, 229),
    (127, 127, 127),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (92, 92, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

/// Converts an alacritty `AColor` to `RgbaColor`.
pub fn acolor_to_rgba(c: AColor) -> RgbaColor {
    match c {
        AColor::Spec(rgb) => RgbaColor::srgb(rgb.r, rgb.g, rgb.b),
        AColor::Indexed(i) => palette_index_to_rgba(i),
        AColor::Named(named) => named_color_to_rgba(named),
    }
}

fn named_color_to_rgba(named: NamedColor) -> RgbaColor {
    match named {
        NamedColor::Foreground => RgbaColor::WHITE,
        NamedColor::Background => RgbaColor::BLACK,
        NamedColor::Cursor => RgbaColor::WHITE,
        NamedColor::BrightForeground => RgbaColor::WHITE,
        NamedColor::DimForeground => RgbaColor::WHITE,
        NamedColor::DimBlack => palette_index_to_rgba(0),
        NamedColor::DimRed => palette_index_to_rgba(1),
        NamedColor::DimGreen => palette_index_to_rgba(2),
        NamedColor::DimYellow => palette_index_to_rgba(3),
        NamedColor::DimBlue => palette_index_to_rgba(4),
        NamedColor::DimMagenta => palette_index_to_rgba(5),
        NamedColor::DimCyan => palette_index_to_rgba(6),
        NamedColor::DimWhite => palette_index_to_rgba(7),
        NamedColor::Black => palette_index_to_rgba(0),
        NamedColor::Red => palette_index_to_rgba(1),
        NamedColor::Green => palette_index_to_rgba(2),
        NamedColor::Yellow => palette_index_to_rgba(3),
        NamedColor::Blue => palette_index_to_rgba(4),
        NamedColor::Magenta => palette_index_to_rgba(5),
        NamedColor::Cyan => palette_index_to_rgba(6),
        NamedColor::White => palette_index_to_rgba(7),
        NamedColor::BrightBlack => palette_index_to_rgba(8),
        NamedColor::BrightRed => palette_index_to_rgba(9),
        NamedColor::BrightGreen => palette_index_to_rgba(10),
        NamedColor::BrightYellow => palette_index_to_rgba(11),
        NamedColor::BrightBlue => palette_index_to_rgba(12),
        NamedColor::BrightMagenta => palette_index_to_rgba(13),
        NamedColor::BrightCyan => palette_index_to_rgba(14),
        NamedColor::BrightWhite => palette_index_to_rgba(15),
    }
}

fn palette_index_to_rgba(i: u8) -> RgbaColor {
    let i = i as usize;
    if i < 16 {
        let (r, g, b) = ANSI_16[i];
        return RgbaColor::srgb(r, g, b);
    }
    if i < 232 {
        let n = i - 16;
        let r = CUBE_RAMP[n / 36];
        let g = CUBE_RAMP[(n / 6) % 6];
        let b = CUBE_RAMP[n % 6];
        return RgbaColor::srgb(r, g, b);
    }
    let gray = 8 + (i as u8 - 232) * 10;
    RgbaColor::srgb(gray, gray, gray)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Rgb;

    #[test]
    fn spec_maps_to_srgb() {
        assert_eq!(
            acolor_to_rgba(AColor::Spec(Rgb {
                r: 10,
                g: 20,
                b: 30
            })),
            RgbaColor::srgb(10, 20, 30)
        );
    }

    #[test]
    fn indexed_0_is_black() {
        assert_eq!(acolor_to_rgba(AColor::Indexed(0)), RgbaColor::srgb(0, 0, 0));
    }

    #[test]
    fn indexed_15_is_bright_white() {
        assert_eq!(
            acolor_to_rgba(AColor::Indexed(15)),
            RgbaColor::srgb(255, 255, 255)
        );
    }

    #[test]
    fn indexed_16_starts_cube_at_origin() {
        assert_eq!(
            acolor_to_rgba(AColor::Indexed(16)),
            RgbaColor::srgb(0, 0, 0)
        );
    }

    #[test]
    fn indexed_231_ends_cube_at_white() {
        assert_eq!(
            acolor_to_rgba(AColor::Indexed(231)),
            RgbaColor::srgb(255, 255, 255)
        );
    }

    #[test]
    fn indexed_232_starts_grayscale_at_8() {
        assert_eq!(
            acolor_to_rgba(AColor::Indexed(232)),
            RgbaColor::srgb(8, 8, 8)
        );
    }

    #[test]
    fn indexed_255_ends_grayscale_at_238() {
        assert_eq!(
            acolor_to_rgba(AColor::Indexed(255)),
            RgbaColor::srgb(238, 238, 238)
        );
    }

    #[test]
    fn named_foreground_is_white_sentinel() {
        assert_eq!(
            acolor_to_rgba(AColor::Named(NamedColor::Foreground)),
            RgbaColor::WHITE
        );
    }

    #[test]
    fn named_background_is_black_sentinel() {
        assert_eq!(
            acolor_to_rgba(AColor::Named(NamedColor::Background)),
            RgbaColor::BLACK
        );
    }

    #[test]
    fn dim_red_maps_to_regular_red_not_white() {
        assert_eq!(
            acolor_to_rgba(AColor::Named(NamedColor::DimRed)),
            RgbaColor::srgb(205, 0, 0)
        );
    }

    #[test]
    fn dim_white_maps_to_ansi_white() {
        assert_eq!(
            acolor_to_rgba(AColor::Named(NamedColor::DimWhite)),
            RgbaColor::srgb(229, 229, 229)
        );
    }

    #[test]
    fn named_bright_red_maps_to_ansi_index_9() {
        assert_eq!(
            acolor_to_rgba(AColor::Named(NamedColor::BrightRed)),
            RgbaColor::srgb(255, 0, 0)
        );
    }
}
