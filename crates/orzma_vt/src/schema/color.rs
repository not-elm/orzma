//! Vocabulary for cell colors.
//!
//! Colors travel symbolically: [`Color::Indexed`] keeps its palette slot
//! instead of a resolved triple. That is what lets an OSC 4 / OSC 104
//! palette override or a theme change repaint without re-emitting every
//! row, and what lets a consumer tell the terminal default background —
//! which renders transparent so webview overlays show through — from an
//! explicitly-set background that happens to carry the same RGB.

#[cfg(feature = "alacritty")]
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Rgb as ARgb};

/// A cell color, carrying its source rather than a resolved value.
///
/// # Invariants
///
/// A resolver MUST consult the live palette (alacritty's `Term::colors()`)
/// before falling back to a built-in table. OSC 4 and OSC 104 overwrite
/// palette entries in place, so resolving [`Color::Indexed`] against a
/// fixed table silently discards them.
///
/// [`Color::DefaultBackground`] MUST NOT be resolved to the same value as
/// an equal [`Color::Rgb`]: the default background renders transparent so
/// webview overlays composite through it, while an explicitly-set
/// background occludes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// The terminal default foreground (`SGR 39`, recolored by OSC 10).
    DefaultForeground,
    /// The terminal default background (`SGR 49`, recolored by OSC 11).
    DefaultBackground,
    /// An xterm-256 palette slot (`SGR 38;5` / `SGR 48;5`, recolored by
    /// OSC 4 and reset by OSC 104).
    Indexed(u8),
    /// A direct color (`SGR 38;2` / `SGR 48;2`).
    Rgb(Rgb),
}

impl Color {
    /// Translates an alacritty cell color into this crate's form.
    ///
    /// `NamedColor` addresses alacritty's full 0..=268 color table, so the
    /// entries beyond the xterm-256 range have no [`Color::Indexed`]
    /// spelling. None of them is reachable from a cell: vte's SGR parser
    /// only ever produces `Black`..=`BrightWhite`, `Foreground`, and
    /// `Background`, and neither vte nor `alacritty_terminal` calls
    /// `NamedColor::to_bright` / `to_dim`. They are mapped defensively so
    /// that a future bright/dim resolver degrades toward the right hue
    /// rather than toward the default color.
    #[cfg(feature = "alacritty")]
    pub fn from_alacritty(color: AColor) -> Self {
        match color {
            AColor::Indexed(index) => Self::Indexed(index),
            AColor::Spec(rgb) => Self::Rgb(rgb.into()),
            AColor::Named(named) => Self::from_named(named),
        }
    }

    #[cfg(feature = "alacritty")]
    fn from_named(named: NamedColor) -> Self {
        match named {
            NamedColor::Black => Self::Indexed(0),
            NamedColor::Red => Self::Indexed(1),
            NamedColor::Green => Self::Indexed(2),
            NamedColor::Yellow => Self::Indexed(3),
            NamedColor::Blue => Self::Indexed(4),
            NamedColor::Magenta => Self::Indexed(5),
            NamedColor::Cyan => Self::Indexed(6),
            NamedColor::White => Self::Indexed(7),
            NamedColor::BrightBlack => Self::Indexed(8),
            NamedColor::BrightRed => Self::Indexed(9),
            NamedColor::BrightGreen => Self::Indexed(10),
            NamedColor::BrightYellow => Self::Indexed(11),
            NamedColor::BrightBlue => Self::Indexed(12),
            NamedColor::BrightMagenta => Self::Indexed(13),
            NamedColor::BrightCyan => Self::Indexed(14),
            NamedColor::BrightWhite => Self::Indexed(15),
            NamedColor::Foreground => Self::DefaultForeground,
            NamedColor::Background => Self::DefaultBackground,
            NamedColor::DimBlack => Self::Indexed(0),
            NamedColor::DimRed => Self::Indexed(1),
            NamedColor::DimGreen => Self::Indexed(2),
            NamedColor::DimYellow => Self::Indexed(3),
            NamedColor::DimBlue => Self::Indexed(4),
            NamedColor::DimMagenta => Self::Indexed(5),
            NamedColor::DimCyan => Self::Indexed(6),
            NamedColor::DimWhite => Self::Indexed(7),
            NamedColor::Cursor | NamedColor::BrightForeground | NamedColor::DimForeground => {
                Self::DefaultForeground
            }
        }
    }
}

#[cfg(feature = "alacritty")]
impl From<alacritty_terminal::vte::ansi::Color> for Color {
    fn from(value: alacritty_terminal::vte::ansi::Color) -> Self {
        Self::from_alacritty(value)
    }
}

/// A 24-bit sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

/// The live color table symbolic [`Color`]s resolve against.
///
/// Each slot is pre-resolved: the backend folds OSC 4 / OSC 104
/// overrides over its built-in xterm table before publishing, so a
/// consumer indexes this table directly instead of layering override
/// lookups over a fallback of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// The 256 xterm palette slots [`Color::Indexed`] addresses.
    pub indexed: Box<[Rgb; 256]>,
    /// The default foreground [`Color::DefaultForeground`] resolves
    /// to (recolored by OSC 10).
    pub foreground: Rgb,
    /// The default background [`Color::DefaultBackground`] resolves
    /// to (recolored by OSC 11).
    pub background: Rgb,
}

#[cfg(feature = "alacritty")]
impl From<ARgb> for Rgb {
    fn from(rgb: ARgb) -> Self {
        Self {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }
    }
}

#[cfg(all(test, feature = "alacritty"))]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn spec(r: u8, g: u8, b: u8) -> AColor {
        AColor::Spec(ARgb { r, g, b })
    }

    /// The 16 base ANSI names paired with the palette index they occupy.
    const ANSI_BASE: [(NamedColor, u8); 16] = [
        (NamedColor::Black, 0),
        (NamedColor::Red, 1),
        (NamedColor::Green, 2),
        (NamedColor::Yellow, 3),
        (NamedColor::Blue, 4),
        (NamedColor::Magenta, 5),
        (NamedColor::Cyan, 6),
        (NamedColor::White, 7),
        (NamedColor::BrightBlack, 8),
        (NamedColor::BrightRed, 9),
        (NamedColor::BrightGreen, 10),
        (NamedColor::BrightYellow, 11),
        (NamedColor::BrightBlue, 12),
        (NamedColor::BrightMagenta, 13),
        (NamedColor::BrightCyan, 14),
        (NamedColor::BrightWhite, 15),
    ];

    /// The 8 dim names paired with the base index they collapse onto.
    const DIM_TO_BASE: [(NamedColor, u8); 8] = [
        (NamedColor::DimBlack, 0),
        (NamedColor::DimRed, 1),
        (NamedColor::DimGreen, 2),
        (NamedColor::DimYellow, 3),
        (NamedColor::DimBlue, 4),
        (NamedColor::DimMagenta, 5),
        (NamedColor::DimCyan, 6),
        (NamedColor::DimWhite, 7),
    ];

    #[test]
    fn alacritty_rgb_converts_channelwise() {
        let rgb = Rgb::from(ARgb {
            r: 10,
            g: 20,
            b: 30,
        });
        assert_eq!(
            rgb,
            Rgb {
                r: 10,
                g: 20,
                b: 30
            }
        );
    }

    #[test]
    fn spec_becomes_rgb() {
        assert_eq!(
            Color::from_alacritty(spec(1, 2, 3)),
            Color::Rgb(Rgb { r: 1, g: 2, b: 3 })
        );
    }

    #[test]
    fn spec_preserves_channel_order() {
        // Guards against an r/b swap, which a symmetric fixture would miss.
        let Color::Rgb(rgb) = Color::from_alacritty(spec(255, 0, 0)) else {
            panic!("Spec must map to Color::Rgb");
        };
        assert_eq!(rgb.r, 255);
        assert_eq!(rgb.g, 0);
        assert_eq!(rgb.b, 0);
    }

    #[test]
    fn indexed_passes_through_every_slot() {
        for i in 0..=u8::MAX {
            assert_eq!(
                Color::from_alacritty(AColor::Indexed(i)),
                Color::Indexed(i),
                "Indexed({i}) must survive unresolved"
            );
        }
    }

    #[test]
    fn named_foreground_becomes_default_foreground() {
        assert_eq!(
            Color::from_alacritty(AColor::Named(NamedColor::Foreground)),
            Color::DefaultForeground
        );
    }

    #[test]
    fn named_background_becomes_default_background() {
        assert_eq!(
            Color::from_alacritty(AColor::Named(NamedColor::Background)),
            Color::DefaultBackground
        );
    }

    #[test]
    fn named_base_colors_become_their_palette_index() {
        for (named, index) in ANSI_BASE {
            assert_eq!(
                Color::from_alacritty(AColor::Named(named)),
                Color::Indexed(index),
                "{named:?} must map to palette slot {index}"
            );
        }
    }

    #[test]
    fn dim_named_colors_collapse_to_their_base_index() {
        for (named, index) in DIM_TO_BASE {
            assert_eq!(
                Color::from_alacritty(AColor::Named(named)),
                Color::Indexed(index),
                "{named:?} must collapse to base slot {index}, not the bright variant"
            );
        }
    }

    #[test]
    fn dim_named_colors_do_not_collapse_to_the_bright_variant() {
        // Regression guard: `NamedColor::to_bright` maps DimRed -> Red, so a
        // mapping written via the wrong direction would land on 9, not 1.
        assert_eq!(
            Color::from_alacritty(AColor::Named(NamedColor::DimRed)),
            Color::Indexed(1)
        );
    }

    #[test]
    fn bright_and_dim_foreground_become_default_foreground() {
        assert_eq!(
            Color::from_alacritty(AColor::Named(NamedColor::BrightForeground)),
            Color::DefaultForeground
        );
        assert_eq!(
            Color::from_alacritty(AColor::Named(NamedColor::DimForeground)),
            Color::DefaultForeground
        );
    }

    #[test]
    fn cursor_color_becomes_default_foreground() {
        assert_eq!(
            Color::from_alacritty(AColor::Named(NamedColor::Cursor)),
            Color::DefaultForeground
        );
    }

    #[test]
    fn only_named_background_yields_default_background() {
        // The transparency contract keys off DefaultBackground, so no other
        // name may reach it — a stray mapping would punch a hole in a cell
        // that should be opaque.
        let others = ANSI_BASE
            .iter()
            .chain(DIM_TO_BASE.iter())
            .map(|(named, _)| *named)
            .chain([
                NamedColor::Foreground,
                NamedColor::Cursor,
                NamedColor::BrightForeground,
                NamedColor::DimForeground,
            ]);
        for named in others {
            assert_ne!(
                Color::from_alacritty(AColor::Named(named)),
                Color::DefaultBackground,
                "{named:?} must not map to DefaultBackground"
            );
        }
    }

    #[test]
    fn indexed_stays_distinct_from_the_rgb_it_would_resolve_to() {
        // The whole point of staying symbolic: slot 1 must not compare equal
        // to the xterm default red, or an OSC 4 override could never change
        // an already-emitted cell.
        assert_ne!(Color::Indexed(1), Color::Rgb(Rgb { r: 205, g: 0, b: 0 }));
    }

    #[test]
    fn default_background_stays_distinct_from_an_equal_explicit_rgb() {
        // Distinguishes "transparent, let the webview through" from "a TUI
        // painted this exact color", which a resolved RGB value cannot.
        assert_ne!(
            Color::DefaultBackground,
            Color::Rgb(Rgb { r: 0, g: 0, b: 0 })
        );
        assert_ne!(Color::DefaultBackground, Color::DefaultForeground);
    }

    #[test]
    fn equal_colors_hash_equally() {
        // Run coalescing and the row-content hash both key off this.
        fn digest(c: Color) -> u64 {
            let mut h = DefaultHasher::new();
            c.hash(&mut h);
            h.finish()
        }
        assert_eq!(digest(Color::Indexed(7)), digest(Color::Indexed(7)));
        assert_ne!(digest(Color::Indexed(7)), digest(Color::Indexed(8)));
        assert_ne!(
            digest(Color::DefaultForeground),
            digest(Color::DefaultBackground)
        );
    }
}
