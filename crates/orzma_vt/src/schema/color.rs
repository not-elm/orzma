//! Vocabulary for cell colors.
//!
//! Colors travel symbolically: [`Color::Indexed`] keeps its palette slot
//! instead of a resolved triple. That is what lets an OSC 4 / OSC 104
//! palette override or a theme change repaint without re-emitting every
//! row, and what lets a consumer tell the terminal default background —
//! which renders transparent so webview overlays show through — from an
//! explicitly-set background that happens to carry the same RGB.

#[cfg(feature = "alacritty")]
use alacritty_terminal::term::color::Colors;
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

impl Default for Palette {
    fn default() -> Self {
        Self {
            indexed: Box::new(XTERM_INDEXED),
            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,
        }
    }
}

impl Palette {
    /// Resolves a symbolic cell color against this table.
    ///
    /// [`Color::DefaultBackground`] resolves to [`Palette::background`]
    /// like any other slot. A consumer that must keep the transparent
    /// default background distinguishable from an equal explicit RGB
    /// (see the invariant on [`Color`]) branches on the variant before
    /// calling this.
    pub fn resolve(&self, color: Color) -> Rgb {
        match color {
            Color::DefaultForeground => self.foreground,
            Color::DefaultBackground => self.background,
            Color::Indexed(index) => self.indexed[usize::from(index)],
            Color::Rgb(rgb) => rgb,
        }
    }
}

#[cfg(feature = "alacritty")]
impl Palette {
    /// Folds the terminal's live OSC overrides over the xterm defaults.
    ///
    /// A `Some` slot in `colors` is an active OSC 4 / 10 / 11 override
    /// and wins; a `None` slot resolves to the built-in xterm value,
    /// which is also how an OSC 104 reset takes effect — alacritty
    /// clears the slot back to `None`.
    pub fn from_alacritty_colors(colors: &Colors) -> Self {
        let mut palette = Self::default();
        for (index, slot) in palette.indexed.iter_mut().enumerate() {
            if let Some(rgb) = colors[index] {
                *slot = rgb.into();
            }
        }
        if let Some(foreground) = colors[NamedColor::Foreground] {
            palette.foreground = foreground.into();
        }
        if let Some(background) = colors[NamedColor::Background] {
            palette.background = background.into();
        }
        palette
    }
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

/// The default foreground [`Palette`] carries before any OSC 10
/// override.
const DEFAULT_FOREGROUND: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};

/// The default background [`Palette`] carries before any OSC 11
/// override.
const DEFAULT_BACKGROUND: Rgb = Rgb { r: 0, g: 0, b: 0 };

/// Channel ramp for the 6x6x6 cube portion of the xterm table.
const CUBE_RAMP: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The 16 ANSI base slots as xterm defaults, black through bright
/// white.
const ANSI_16: [Rgb; 16] = [
    Rgb { r: 0, g: 0, b: 0 },
    Rgb { r: 205, g: 0, b: 0 },
    Rgb { r: 0, g: 205, b: 0 },
    Rgb {
        r: 205,
        g: 205,
        b: 0,
    },
    Rgb { r: 0, g: 0, b: 238 },
    Rgb {
        r: 205,
        g: 0,
        b: 205,
    },
    Rgb {
        r: 0,
        g: 205,
        b: 205,
    },
    Rgb {
        r: 229,
        g: 229,
        b: 229,
    },
    Rgb {
        r: 127,
        g: 127,
        b: 127,
    },
    Rgb { r: 255, g: 0, b: 0 },
    Rgb { r: 0, g: 255, b: 0 },
    Rgb {
        r: 255,
        g: 255,
        b: 0,
    },
    Rgb {
        r: 92,
        g: 92,
        b: 255,
    },
    Rgb {
        r: 255,
        g: 0,
        b: 255,
    },
    Rgb {
        r: 0,
        g: 255,
        b: 255,
    },
    Rgb {
        r: 255,
        g: 255,
        b: 255,
    },
];

/// The full 256-slot xterm table [`Color::Indexed`] resolves to before
/// any OSC 4 override: [`ANSI_16`], the 6x6x6 cube on [`CUBE_RAMP`],
/// and the grayscale ramp from 8 to 238 in steps of 10.
const XTERM_INDEXED: [Rgb; 256] = build_xterm_indexed();

const fn build_xterm_indexed() -> [Rgb; 256] {
    let mut table = [Rgb { r: 0, g: 0, b: 0 }; 256];
    let mut i = 0;
    while i < 16 {
        table[i] = ANSI_16[i];
        i += 1;
    }
    while i < 232 {
        let cube = i - 16;
        table[i] = Rgb {
            r: CUBE_RAMP[cube / 36],
            g: CUBE_RAMP[(cube / 6) % 6],
            b: CUBE_RAMP[cube % 6],
        };
        i += 1;
    }
    while i < 256 {
        let gray = 8 + (i as u8 - 232) * 10;
        table[i] = Rgb {
            r: gray,
            g: gray,
            b: gray,
        };
        i += 1;
    }
    table
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

    /// Asserts that the default palette seeds the 16 ANSI base slots
    /// with the xterm defaults.
    ///
    /// Case: a fresh terminal renders `ls --color` output before any
    /// OSC 4 override arrives, so indexed cells must resolve against
    /// the stock xterm colors.
    #[test]
    fn the_default_palette_seeds_the_ansi_base_slots() {
        let palette = Palette::default();
        assert_eq!(palette.indexed[0], Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(palette.indexed[1], Rgb { r: 205, g: 0, b: 0 });
        assert_eq!(
            palette.indexed[15],
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }

    /// Asserts that the default palette builds slots 16..=231 from the
    /// 6x6x6 cube with the xterm channel ramp.
    ///
    /// Case: a TUI picks `SGR 38;5;196` for an error marker and the
    /// renderer must show the canonical cube red, not an interpolated
    /// approximation.
    #[test]
    fn the_default_palette_builds_the_color_cube_from_the_channel_ramp() {
        let palette = Palette::default();
        assert_eq!(palette.indexed[16], Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(palette.indexed[196], Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(
            palette.indexed[231],
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }

    /// Asserts that the default palette fills slots 232..=255 with the
    /// grayscale ramp from 8 to 238.
    ///
    /// Case: a diff pager shades context lines with high grayscale
    /// slots, which must land on the xterm gray steps.
    #[test]
    fn the_default_palette_ends_with_the_grayscale_ramp() {
        let palette = Palette::default();
        assert_eq!(palette.indexed[232], Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            palette.indexed[255],
            Rgb {
                r: 238,
                g: 238,
                b: 238
            }
        );
    }

    /// Asserts that an overridden slot wins over the xterm default
    /// while untouched slots keep theirs.
    ///
    /// Case: a theming tool recolors slot 1 with OSC 4; every other
    /// slot must keep resolving to the stock table.
    #[test]
    fn an_overridden_slot_wins_over_the_xterm_default() {
        let mut colors = Colors::default();
        colors[1] = Some(ARgb { r: 255, g: 0, b: 0 });
        let palette = Palette::from_alacritty_colors(&colors);
        assert_eq!(palette.indexed[1], Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(palette.indexed[2], Rgb { r: 0, g: 205, b: 0 });
    }

    /// Asserts that a fully unset color table folds to the default
    /// palette.
    ///
    /// Case: OSC 104 resets a themed slot by clearing it to `None`, so
    /// an all-`None` table must be indistinguishable from a fresh
    /// terminal's palette.
    #[test]
    fn an_unset_table_folds_to_the_default_palette() {
        let palette = Palette::from_alacritty_colors(&Colors::default());
        assert_eq!(palette, Palette::default());
    }

    /// Asserts that OSC 10 / OSC 11 overrides reach the palette's
    /// default foreground and background.
    ///
    /// Case: a theme switcher recolors the terminal defaults, and
    /// cells painted with `SGR 39` / `SGR 49` must resolve to the new
    /// values.
    #[test]
    fn foreground_and_background_overrides_reach_the_palette() {
        let mut colors = Colors::default();
        colors[NamedColor::Foreground] = Some(ARgb {
            r: 170,
            g: 187,
            b: 204,
        });
        colors[NamedColor::Background] = Some(ARgb {
            r: 17,
            g: 34,
            b: 51,
        });
        let palette = Palette::from_alacritty_colors(&colors);
        assert_eq!(
            palette.foreground,
            Rgb {
                r: 170,
                g: 187,
                b: 204
            }
        );
        assert_eq!(
            palette.background,
            Rgb {
                r: 17,
                g: 34,
                b: 51
            }
        );
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

    /// Asserts that each color variant resolves against its designated
    /// palette slot.
    ///
    /// Case: a renderer packs cell colors for the GPU while OSC 4 / 10
    /// / 11 overrides are active, so symbolic colors must follow the
    /// live table rather than a built-in default.
    #[test]
    fn resolve_follows_the_live_table() {
        let mut palette = Palette {
            foreground: Rgb { r: 1, g: 2, b: 3 },
            background: Rgb { r: 4, g: 5, b: 6 },
            ..Palette::default()
        };
        palette.indexed[42] = Rgb { r: 7, g: 8, b: 9 };
        assert_eq!(
            palette.resolve(Color::DefaultForeground),
            Rgb { r: 1, g: 2, b: 3 }
        );
        assert_eq!(
            palette.resolve(Color::DefaultBackground),
            Rgb { r: 4, g: 5, b: 6 }
        );
        assert_eq!(
            palette.resolve(Color::Indexed(42)),
            Rgb { r: 7, g: 8, b: 9 }
        );
        assert_eq!(
            palette.resolve(Color::Rgb(Rgb {
                r: 10,
                g: 11,
                b: 12
            })),
            Rgb {
                r: 10,
                g: 11,
                b: 12
            }
        );
    }
}
