//! `AColor` → `bevy::Color` conversion via the standard xterm
//! 256-color palette.
//!
//! Layout:
//!   0..=15   — 16 named ANSI base colors (xterm defaults)
//!   16..=231 — 6×6×6 RGB cube, channel ramp `[0, 95, 135, 175, 215, 255]`
//!   232..=255 — 24 grayscale ramp from value 8 to 238 in steps of 10
//!
//! `AColor::Named(Foreground)` maps to `Color::WHITE` as a sentinel.
//! `AColor::Named(Background)` maps to `Color::NONE` (transparent) so that
//! cells with the terminal default background let the pane background
//! (`bg_padding_color`) and any webview overlays show through.
//! Cells with an explicit ANSI color are fully opaque and occlude overlays.

use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor};
use bevy::prelude::Color;

/// Channel ramp for the 6×6×6 cube portion of the xterm 256 palette.
const CUBE_RAMP: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// 16 named ANSI base colors as xterm defaults (R, G, B tuples).
const ANSI_16: [(u8, u8, u8); 16] = [
    (0, 0, 0),       // 0  Black
    (205, 0, 0),     // 1  Red
    (0, 205, 0),     // 2  Green
    (205, 205, 0),   // 3  Yellow
    (0, 0, 238),     // 4  Blue
    (205, 0, 205),   // 5  Magenta
    (0, 205, 205),   // 6  Cyan
    (229, 229, 229), // 7  White
    (127, 127, 127), // 8  Bright Black
    (255, 0, 0),     // 9  Bright Red
    (0, 255, 0),     // 10 Bright Green
    (255, 255, 0),   // 11 Bright Yellow
    (92, 92, 255),   // 12 Bright Blue
    (255, 0, 255),   // 13 Bright Magenta
    (0, 255, 255),   // 14 Bright Cyan
    (255, 255, 255), // 15 Bright White
];

/// Convert an alacritty `AColor` into a `bevy::prelude::Color`.
pub(crate) fn acolor_to_bevy(c: AColor) -> Color {
    match c {
        AColor::Spec(rgb) => Color::srgb_u8(rgb.r, rgb.g, rgb.b),
        AColor::Indexed(i) => palette_index_to_color(i),
        AColor::Named(named) => named_color_to_bevy(named),
    }
}

/// Convert a `NamedColor` (whose discriminants jump past 255 starting
/// at `Foreground = 256`) into a `bevy::Color`.
///
/// `Foreground` / `Cursor` / `BrightForeground` / `DimForeground` fall
/// back to `Color::WHITE`. `Background` maps to `Color::NONE` (transparent)
/// so that default-background cells let webview overlays and the pane
/// background show through; cells with explicit ANSI colors are opaque
/// and occlude overlays. `Dim*` base colors map to the regular palette
/// index (faint is approximated by the base intensity rather than
/// the bright variant); without this, every `\e[2;Nm`-styled cell
/// would render as solid white because the Dim* discriminants exceed
/// 255 and miss the palette lookup.
fn named_color_to_bevy(named: NamedColor) -> Color {
    match named {
        NamedColor::Foreground => Color::WHITE,
        NamedColor::Background => Color::NONE,
        NamedColor::Cursor => Color::WHITE,
        NamedColor::BrightForeground => Color::WHITE,
        NamedColor::DimForeground => Color::WHITE,
        NamedColor::DimBlack => palette_index_to_color(0),
        NamedColor::DimRed => palette_index_to_color(1),
        NamedColor::DimGreen => palette_index_to_color(2),
        NamedColor::DimYellow => palette_index_to_color(3),
        NamedColor::DimBlue => palette_index_to_color(4),
        NamedColor::DimMagenta => palette_index_to_color(5),
        NamedColor::DimCyan => palette_index_to_color(6),
        NamedColor::DimWhite => palette_index_to_color(7),
        _ => {
            // Black..BrightWhite (0..=15): discriminant matches palette index.
            let idx = named as u32;
            if idx < 16 {
                palette_index_to_color(idx as u8)
            } else {
                Color::WHITE
            }
        }
    }
}

fn palette_index_to_color(i: u8) -> Color {
    let i = i as usize;
    if i < 16 {
        let (r, g, b) = ANSI_16[i];
        return Color::srgb_u8(r, g, b);
    }
    if i < 232 {
        let n = i - 16;
        let r = CUBE_RAMP[n / 36];
        let g = CUBE_RAMP[(n / 6) % 6];
        let b = CUBE_RAMP[n % 6];
        return Color::srgb_u8(r, g, b);
    }
    let gray = 8 + (i as u8 - 232) * 10;
    Color::srgb_u8(gray, gray, gray)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Rgb;

    #[test]
    fn spec_maps_to_srgb_u8() {
        let c = acolor_to_bevy(AColor::Spec(Rgb {
            r: 10,
            g: 20,
            b: 30,
        }));
        assert_eq!(c, Color::srgb_u8(10, 20, 30));
    }

    #[test]
    fn indexed_0_is_black() {
        assert_eq!(acolor_to_bevy(AColor::Indexed(0)), Color::srgb_u8(0, 0, 0));
    }

    #[test]
    fn indexed_15_is_bright_white() {
        assert_eq!(
            acolor_to_bevy(AColor::Indexed(15)),
            Color::srgb_u8(255, 255, 255)
        );
    }

    #[test]
    fn indexed_16_starts_cube_at_origin() {
        assert_eq!(acolor_to_bevy(AColor::Indexed(16)), Color::srgb_u8(0, 0, 0));
    }

    #[test]
    fn indexed_231_ends_cube_at_white() {
        assert_eq!(
            acolor_to_bevy(AColor::Indexed(231)),
            Color::srgb_u8(255, 255, 255)
        );
    }

    #[test]
    fn indexed_232_starts_grayscale_at_8() {
        assert_eq!(
            acolor_to_bevy(AColor::Indexed(232)),
            Color::srgb_u8(8, 8, 8)
        );
    }

    #[test]
    fn indexed_255_ends_grayscale_at_238() {
        assert_eq!(
            acolor_to_bevy(AColor::Indexed(255)),
            Color::srgb_u8(238, 238, 238)
        );
    }

    #[test]
    fn named_foreground_is_white_sentinel() {
        assert_eq!(
            acolor_to_bevy(AColor::Named(NamedColor::Foreground)),
            Color::WHITE
        );
    }

    #[test]
    fn named_background_is_transparent() {
        assert_eq!(
            acolor_to_bevy(AColor::Named(NamedColor::Background)),
            Color::NONE
        );
    }

    #[test]
    fn dim_red_maps_to_regular_red_not_white() {
        // Regression: NamedColor::DimRed discriminant is 260, which
        // used to fall through the `idx < 256` check and render as
        // Color::WHITE — making every `\e[2;31m`-styled cell solid
        // white instead of red.
        assert_eq!(
            acolor_to_bevy(AColor::Named(NamedColor::DimRed)),
            Color::srgb_u8(205, 0, 0)
        );
    }

    #[test]
    fn dim_black_maps_to_ansi_black() {
        assert_eq!(
            acolor_to_bevy(AColor::Named(NamedColor::DimBlack)),
            Color::srgb_u8(0, 0, 0)
        );
    }

    #[test]
    fn dim_white_maps_to_ansi_white() {
        assert_eq!(
            acolor_to_bevy(AColor::Named(NamedColor::DimWhite)),
            Color::srgb_u8(229, 229, 229)
        );
    }
}
