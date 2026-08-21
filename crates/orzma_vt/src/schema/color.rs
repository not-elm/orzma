//! Vocabulary for cell colors.
//!
//! Colors travel symbolically: [`Color::Indexed`] keeps its palette slot
//! instead of a resolved triple. That is what lets an OSC 4 / OSC 104
//! palette override or a theme change repaint without re-emitting every
//! row, and what lets a consumer tell the terminal default background —
//! which renders transparent so webview overlays show through — from an
//! explicitly-set background that happens to carry the same RGB.

/// A cell color, carrying its source rather than a resolved value.
///
/// # Invariants
///
/// A resolver MUST consult the live [`Palette`] before falling back to a
/// built-in table. OSC 4 and OSC 104 overwrite palette entries in place,
/// so resolving [`Color::Indexed`] against a fixed table silently
/// discards them.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

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
    /// / 11 overrides are active.
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
