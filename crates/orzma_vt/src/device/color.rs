//! The color vocabulary SGR writes into cells and the palette the
//! device resolves it against.

/// A cell color, carrying its source rather than a resolved value.
///
/// A resolver MUST consult the live [`Palette`] before falling back to a
/// built-in table.
///
/// [`Color::DefaultBackground`] MUST NOT be resolved to the same value as
/// an equal [`Color::Rgb`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// The terminal default foreground (`SGR 39`).
    DefaultForeground,
    /// The terminal default background (`SGR 49`).
    DefaultBackground,
    /// An xterm-256 palette slot (`SGR 38;5` / `SGR 48;5`).
    Indexed(u8),
    /// A direct color (`SGR 38;2` / `SGR 48;2`).
    Rgb(Rgb),
}

impl Color {
    /// The colour an `SGR 38` / `48` / `58` selector and its operands
    /// spell; `None` when the selector is not one this terminal answers
    /// or a component does not fit a byte.
    ///
    /// A component is rejected rather than clamped.
    pub fn from_sgr(selector: u16, operands: &[u16]) -> Option<Self> {
        match (selector, operands) {
            (5, [index]) => Some(Self::Indexed(u8::try_from(*index).ok()?)),
            (2, [r, g, b]) => Some(Self::Rgb(Rgb {
                r: u8::try_from(*r).ok()?,
                g: u8::try_from(*g).ok()?,
                b: u8::try_from(*b).ok()?,
            })),
            _ => None,
        }
    }

    /// The colour one `SGR` selector group's own `:` subparameters
    /// spell, everything after the `38` / `48` / `58` itself.
    ///
    /// The colour-space slot is optional: a group of five or more
    /// subparameters reads as `2:Pi:r:g:b`, the spelling the standard
    /// gives, and a four-element group reads as `2:r:g:b`. Subparameters
    /// after blue are the tolerance tail the standard permits and are
    /// ignored.
    pub fn from_sgr_group(subs: &[Option<u16>]) -> Option<Self> {
        let selector = subs.first().copied().flatten()?;
        let mut operands = [0u16; 3];
        let taken = match (selector, subs.len()) {
            (5, _) => {
                operands[0] = subs.get(1).copied().flatten()?;
                1
            }
            (2, 4) => {
                Self::fill(&mut operands, &subs[1..4]);
                3
            }
            (2, len) if len >= 5 => {
                Self::fill(&mut operands, &subs[2..5]);
                3
            }
            _ => return None,
        };
        Self::from_sgr(selector, &operands[..taken])
    }

    /// Copies `subs` into `operands`, an omitted subparameter reading
    /// as zero.
    fn fill(operands: &mut [u16; 3], subs: &[Option<u16>]) {
        for (slot, sub) in operands.iter_mut().zip(subs) {
            *slot = sub.unwrap_or(0);
        }
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
/// Each slot is pre-resolved, so a consumer indexes this table directly
/// instead of layering override lookups over a fallback of its own.
///
/// The table holds the built-in defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// The 256 xterm palette slots [`Color::Indexed`] addresses.
    pub indexed: Box<[Rgb; 256]>,
    /// The default foreground [`Color::DefaultForeground`] resolves to.
    pub foreground: Rgb,
    /// The default background [`Color::DefaultBackground`] resolves to.
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
    /// branches on the variant before calling this.
    pub fn resolve(&self, color: Color) -> Rgb {
        match color {
            Color::DefaultForeground => self.foreground,
            Color::DefaultBackground => self.background,
            Color::Indexed(index) => self.indexed[usize::from(index)],
            Color::Rgb(rgb) => rgb,
        }
    }
}

/// The default foreground [`Palette`] carries.
const DEFAULT_FOREGROUND: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};

/// The default background [`Palette`] carries.
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

/// The full 256-slot xterm table [`Color::Indexed`] resolves to:
/// [`ANSI_16`], the 6x6x6 cube on [`CUBE_RAMP`], and the grayscale ramp
/// from 8 to 238 in steps of 10.
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

    /// Asserts that an indexed color stays distinct from the RGB value
    /// it currently resolves to, so equality tracks the palette slot
    /// rather than the resolved pixel.
    ///
    /// Case: one escape sequence selects palette slot 1 with SGR
    /// 38;5;1, and another explicitly requests the same red that slot
    /// currently resolves to with SGR 38;2;205;0;0.
    #[test]
    fn indexed_stays_distinct_from_the_rgb_it_would_resolve_to() {
        assert_ne!(Color::Indexed(1), Color::Rgb(Rgb { r: 205, g: 0, b: 0 }));
    }

    /// Asserts that the default background stays distinct from an
    /// explicit RGB color carrying the same resolved value, so identity
    /// does not collapse into the resolved pixel.
    ///
    /// Case: the current palette resolves the default background to
    /// black, and a TUI in the same session explicitly paints a cell
    /// with that same black RGB value.
    #[test]
    fn default_background_stays_distinct_from_an_equal_explicit_rgb() {
        assert_ne!(
            Color::DefaultBackground,
            Color::Rgb(Rgb { r: 0, g: 0, b: 0 })
        );
        assert_ne!(Color::DefaultBackground, Color::DefaultForeground);
    }

    /// Asserts that the default palette seeds the 16 ANSI base slots
    /// with the xterm defaults.
    ///
    /// Case: a fresh terminal renders `ls --color` output, which picks
    /// its colors from the 16 ANSI base slots.
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
    /// Case: a TUI picks `SGR 38;5;196` for an error marker.
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
    /// slots.
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

    /// Asserts that Color values that compare equal also hash equally,
    /// and that distinct colors hash differently.
    ///
    /// Case: a row of terminal output repeats the same indexed color
    /// across adjacent cells, while a separate row switches between the
    /// default foreground and background.
    #[test]
    fn equal_colors_hash_equally() {
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
    /// Case: a renderer packs cell colors for the GPU from a palette
    /// whose foreground, background, and indexed slot 42 already hold
    /// colors distinct from the defaults.
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
