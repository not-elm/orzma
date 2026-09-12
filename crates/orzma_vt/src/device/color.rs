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

impl Rgb {
    /// The colour an Xlib color string names in one of its two RGB
    /// Device forms, `rgb:<red>/<green>/<blue>` and `#RGB`; `None` for
    /// every other form and for a malformed one.
    pub fn from_color_spec(spec: &[u8]) -> Option<Self> {
        let [r, g, b] = rgb_device_components(spec)?.map(|component| component.to_be_bytes()[0]);
        Some(Self { r, g, b })
    }
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
            indexed: Box::new(Self::XTERM_INDEXED),
            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,
        }
    }
}

impl Palette {
    /// The full 256-slot xterm table [`Color::Indexed`] resolves to:
    /// [`ANSI_16`], the 6x6x6 cube on [`CUBE_RAMP`], and the grayscale
    /// ramp from 8 to 238 in steps of 10.
    const XTERM_INDEXED: [Rgb; 256] = build_xterm_indexed();

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

    /// Sets slot `index` to `color`; returns whether the slot changed.
    pub fn set_indexed(&mut self, index: u8, color: Rgb) -> bool {
        let slot = &mut self.indexed[usize::from(index)];
        if *slot == color {
            return false;
        }
        *slot = color;
        true
    }

    /// Returns slot `index` to its built-in default; returns whether the
    /// slot changed.
    pub fn reset_indexed(&mut self, index: u8) -> bool {
        self.set_indexed(index, Self::XTERM_INDEXED[usize::from(index)])
    }

    /// Returns every slot to its built-in default, leaving
    /// [`Palette::foreground`] and [`Palette::background`] alone;
    /// returns whether any slot changed.
    pub fn reset_all_indexed(&mut self) -> bool {
        let indexed = &mut *self.indexed;
        if *indexed == Self::XTERM_INDEXED {
            return false;
        }
        *indexed = Self::XTERM_INDEXED;
        true
    }

    /// Returns every color the palette holds — the indexed slots, the
    /// foreground, and the background — to its built-in default; returns
    /// whether anything changed.
    pub fn reset(&mut self) -> bool {
        let default = Self::default();
        if *self == default {
            return false;
        }
        *self = default;
        true
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

/// The 16-bit components an Xlib RGB Device string names, before they
/// are narrowed to a byte; `None` for any other string.
fn rgb_device_components(spec: &[u8]) -> Option<[u16; 3]> {
    if let Some(body) = strip_prefix_ignoring_case(spec, b"rgb:") {
        let mut parts = body.split(|byte| *byte == b'/');
        let components = [
            scaled_component(parts.next()?)?,
            scaled_component(parts.next()?)?,
            scaled_component(parts.next()?)?,
        ];
        return parts.next().is_none().then_some(components);
    }
    let digits = spec.strip_prefix(b"#")?;
    if digits.is_empty() || digits.len() % 3 != 0 {
        return None;
    }
    let mut parts = digits.chunks_exact(digits.len() / 3);
    Some([
        shifted_component(parts.next()?)?,
        shifted_component(parts.next()?)?,
        shifted_component(parts.next()?)?,
    ])
}

/// An `rgb:` component, whose one to four hex digits are scaled to the
/// full 16-bit range.
fn scaled_component(digits: &[u8]) -> Option<u16> {
    let value = u32::from(hex_value(digits)?);
    let max = (1u32 << (4 * digits.len())) - 1;
    u16::try_from(value * 0xffff / max).ok()
}

/// A `#` component, whose one to four hex digits are placed in the most
/// significant bits of 16.
fn shifted_component(digits: &[u8]) -> Option<u16> {
    Some(hex_value(digits)? << (16 - 4 * digits.len()))
}

/// The value of one to four hex digits; `None` for an empty run, a
/// longer one, or a byte that is not a hex digit, a sign included.
fn hex_value(digits: &[u8]) -> Option<u16> {
    // NOTE: the charset is checked here rather than left to
    // `from_str_radix`, which also accepts a leading `+`, so `rgb:+f/0/0`
    // would read as a colour. Uppercase is accepted on purpose: xlib.pdf
    // p.90 makes the digits case insignificant.
    if !(1..=4).contains(&digits.len()) || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u16::from_str_radix(str::from_utf8(digits).ok()?, 16).ok()
}

/// `bytes` without `prefix`, compared without regard to ASCII case;
/// `None` when `bytes` does not start with it.
fn strip_prefix_ignoring_case<'a>(bytes: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    let (head, rest) = bytes.split_at_checked(prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then_some(rest)
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

    /// Asserts that setting a slot reports the change and reads back.
    ///
    /// Case: a theme script recolors ANSI red.
    #[test]
    fn setting_a_slot_reports_the_change_and_reads_back() {
        let mut palette = Palette::default();
        let color = rgb(0x12, 0x34, 0x56);
        assert!(palette.set_indexed(1, color));
        assert_eq!(palette.indexed[1], color);
    }

    /// Asserts that setting a slot to the colour it already holds
    /// reports no change, so no repaint is staged.
    ///
    /// Case: a theme script re-applies the stock xterm red on every
    /// prompt.
    #[test]
    fn setting_a_slot_to_its_current_color_reports_no_change() {
        let mut palette = Palette::default();
        let current = palette.indexed[1];
        assert!(!palette.set_indexed(1, current));
    }

    /// Asserts that resetting a slot restores its xterm default and
    /// leaves the other slots alone.
    ///
    /// Case: a program restores the one slot it recolored, while a
    /// second slot a theme script recolored earlier is left alone.
    #[test]
    fn resetting_a_slot_restores_its_default_and_leaves_the_others() {
        let mut palette = Palette::default();
        let color = rgb(0x12, 0x34, 0x56);
        palette.set_indexed(1, color);
        palette.set_indexed(2, color);
        assert!(palette.reset_indexed(1));
        assert_eq!(palette.indexed[1], Palette::XTERM_INDEXED[1]);
        assert_eq!(palette.indexed[2], color);
    }

    /// Asserts that resetting a slot that holds its default reports no
    /// change.
    ///
    /// Case: a program restores a slot it never recolored.
    #[test]
    fn resetting_an_untouched_slot_reports_no_change() {
        assert!(!Palette::default().reset_indexed(1));
    }

    /// Asserts that resetting every slot restores the whole xterm table
    /// and reports the change only once.
    ///
    /// Case: `tput init` sends a bare `OSC 104` twice in a row after a
    /// theme script recolored the palette.
    #[test]
    fn resetting_every_slot_restores_the_whole_table() {
        let mut palette = Palette::default();
        palette.set_indexed(0, rgb(1, 2, 3));
        palette.set_indexed(255, rgb(1, 2, 3));
        assert!(palette.reset_all_indexed());
        assert_eq!(*palette.indexed, Palette::XTERM_INDEXED);
        assert!(!palette.reset_all_indexed());
    }

    /// Asserts that resetting every slot leaves the foreground and the
    /// background as they were.
    ///
    /// Case: the terminal is running with a customized default
    /// background when a theme script sends a bare `OSC 104` to restore
    /// the indexed table.
    #[test]
    fn resetting_every_slot_leaves_the_default_colors_alone() {
        let mut palette = Palette {
            foreground: rgb(1, 2, 3),
            background: rgb(4, 5, 6),
            ..Palette::default()
        };
        palette.set_indexed(0, rgb(7, 8, 9));
        assert!(palette.reset_all_indexed());
        assert_eq!(palette.foreground, rgb(1, 2, 3));
        assert_eq!(palette.background, rgb(4, 5, 6));
    }

    /// Asserts that a full reset restores the foreground and the
    /// background alongside the indexed slots, and reports no change on
    /// a palette that already holds the defaults.
    ///
    /// Case: the user hits the shortcut that sends `RIS` after a theme
    /// script recolored both a slot and the default background.
    #[test]
    fn a_full_reset_restores_every_color() {
        let mut palette = Palette {
            foreground: rgb(1, 2, 3),
            background: rgb(4, 5, 6),
            ..Palette::default()
        };
        palette.set_indexed(7, rgb(8, 9, 10));
        assert!(palette.reset());
        assert_eq!(palette, Palette::default());
        assert!(!palette.reset());
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

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Asserts that the two RGB Device forms reach the 16-bit values
    /// Xlib gives them before narrowing: `rgb:` scales a short
    /// component, and `#` places it in the high bits.
    ///
    /// Case: one theme writes its red as `rgb:3/a/7` and another as the
    /// older `#3a7`.
    #[test]
    fn the_two_forms_reach_the_16_bit_values_xlib_gives_them() {
        assert_eq!(
            rgb_device_components(b"rgb:3/a/7"),
            Some([0x3333, 0xaaaa, 0x7777])
        );
        assert_eq!(
            rgb_device_components(b"#3a7"),
            Some([0x3000, 0xa000, 0x7000])
        );
    }

    /// Asserts that an `rgb:` string with two-digit components narrows
    /// to exactly those bytes.
    ///
    /// Case: terminfo's `initc` recolors a slot with `rgb:ff/80/0a`.
    #[test]
    fn a_two_digit_rgb_string_reads_its_components_directly() {
        assert_eq!(
            Rgb::from_color_spec(b"rgb:ff/80/0a"),
            Some(rgb(0xff, 0x80, 0x0a))
        );
    }

    /// Asserts that a one-digit `rgb:` component is scaled across the
    /// channel rather than placed in its high bits.
    ///
    /// Case: a theme writes its colours in the shorthand `rgb:f/8/0`.
    #[test]
    fn a_one_digit_rgb_component_is_scaled_across_the_channel() {
        assert_eq!(
            Rgb::from_color_spec(b"rgb:f/8/0"),
            Some(rgb(0xff, 0x88, 0x00))
        );
    }

    /// Asserts that three- and four-digit `rgb:` components narrow to
    /// the high byte of their 16-bit value.
    ///
    /// Case: a colour picker exports its colours at twelve and sixteen
    /// bits per channel.
    #[test]
    fn wide_rgb_components_narrow_to_their_high_byte() {
        for (spec, expected) in [
            (b"rgb:fff/800/000".as_slice(), rgb(0xff, 0x80, 0x00)),
            (b"rgb:ffff/8080/0000", rgb(0xff, 0x80, 0x00)),
            (b"rgb:cdcd/0000/0000", rgb(0xcd, 0x00, 0x00)),
        ] {
            assert_eq!(Rgb::from_color_spec(spec), Some(expected), "{spec:?}");
        }
    }

    /// Asserts that components of different widths mix in one `rgb:`
    /// string, each scaled by its own width.
    ///
    /// Case: a hand-written theme spells its colours as `rgb:ff/a5/0` and
    /// `rgb:ccc/32/0`, and a third mixes three widths in one spec with
    /// `rgb:f/ed1/cb23`.
    #[test]
    fn components_of_different_widths_mix_in_one_string() {
        for (spec, expected) in [
            (b"rgb:ff/a5/0".as_slice(), rgb(0xff, 0xa5, 0x00)),
            (b"rgb:ccc/32/0", rgb(0xcc, 0x32, 0x00)),
            (b"rgb:f/ed1/cb23", rgb(0xff, 0xed, 0xcb)),
        ] {
            assert_eq!(Rgb::from_color_spec(spec), Some(expected), "{spec:?}");
        }
    }

    /// Asserts that a `#` string places each component in the high
    /// bits, at every width the grammar allows.
    ///
    /// Case: an older theme writes its orange at each width the `#` form
    /// allows.
    #[test]
    fn a_sharp_string_places_each_component_in_the_high_bits() {
        for (spec, expected) in [
            (b"#f80".as_slice(), rgb(0xf0, 0x80, 0x00)),
            (b"#ff8000", rgb(0xff, 0x80, 0x00)),
            (b"#fff800000", rgb(0xff, 0x80, 0x00)),
            (b"#ffff80000000", rgb(0xff, 0x80, 0x00)),
        ] {
            assert_eq!(Rgb::from_color_spec(spec), Some(expected), "{spec:?}");
        }
    }

    /// Asserts that the `#` example the Xlib manual gives reads as the
    /// 16-bit value it names, narrowed to a byte.
    ///
    /// Case: a theme carried over from an X resource file writes its
    /// colour as `#3a7`.
    #[test]
    fn the_manual_sharp_example_reads_as_its_16_bit_equivalent() {
        assert_eq!(Rgb::from_color_spec(b"#3a7"), Some(rgb(0x30, 0xa0, 0x70)));
    }

    /// Asserts that the prefix and the hex digits are read without
    /// regard to case.
    ///
    /// Case: one program writes `RGB:FF/A5/0` and another `#FFA500`.
    #[test]
    fn the_prefix_and_the_digits_ignore_case() {
        assert_eq!(
            Rgb::from_color_spec(b"RGB:FF/A5/0"),
            Some(rgb(0xff, 0xa5, 0x00))
        );
        assert_eq!(
            Rgb::from_color_spec(b"#FFA500"),
            Some(rgb(0xff, 0xa5, 0x00))
        );
    }

    /// Asserts that a `#` string whose digits do not split into three
    /// equal components of one to four digits is refused.
    ///
    /// Case: a typo leaves a theme colour with four or thirteen hex
    /// digits, or none at all.
    #[test]
    fn a_sharp_string_of_another_length_is_refused() {
        for spec in [b"#".as_slice(), b"#12", b"#1234", b"#1234567890abc"] {
            assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
        }
    }

    /// Asserts that an `rgb:` string with more or fewer than three
    /// components is refused, a trailing fourth included.
    ///
    /// Case: a script appends an alpha component, writing
    /// `rgb:ff/ff/ff/ff`, or drops the blue one.
    #[test]
    fn an_rgb_string_without_exactly_three_components_is_refused() {
        for spec in [b"rgb:".as_slice(), b"rgb:ff/ff", b"rgb:ff/ff/ff/ff"] {
            assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
        }
    }

    /// Asserts that an empty or five-digit `rgb:` component is refused.
    ///
    /// Case: a script loses a component to an empty variable, or pads
    /// one to five digits.
    #[test]
    fn an_empty_or_over_long_rgb_component_is_refused() {
        for spec in [b"rgb:/ff/ff".as_slice(), b"rgb:ff//ff", b"rgb:fffff/0/0"] {
            assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
        }
    }

    /// Asserts that a byte other than a hex digit is refused in either
    /// form, a sign included.
    ///
    /// Case: a typo puts a `g` or a `+` into a theme colour.
    #[test]
    fn a_non_hex_digit_is_refused() {
        for spec in [b"rgb:fg/00/00".as_slice(), b"rgb:+f/0/0", b"#ggg"] {
            assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
        }
    }

    /// Asserts that colour names, the other colour spaces, and an empty
    /// spec are refused.
    ///
    /// Case: a script written for xterm names its colours, as in `red`,
    /// or uses Xlib's device-independent spellings such as
    /// `rgbi:1.0/0.0/0.0`.
    #[test]
    fn color_names_and_other_color_spaces_are_refused() {
        for spec in [
            b"red".as_slice(),
            b"rgbi:1.0/0.0/0.0",
            b"CIEXYZ:0.3227/0.28133/0.2493",
            b"",
        ] {
            assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
        }
    }

    /// Asserts that surrounding whitespace is refused rather than
    /// trimmed.
    ///
    /// Case: a script builds the spec from a padded shell variable.
    #[test]
    fn surrounding_whitespace_is_refused_rather_than_trimmed() {
        for spec in [b" rgb:ff/ff/ff".as_slice(), b"rgb:ff/ff/ff ", b" #fff"] {
            assert_eq!(Rgb::from_color_spec(spec), None, "{spec:?}");
        }
    }
}
