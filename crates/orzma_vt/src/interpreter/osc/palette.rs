//! The indexed palette an `OSC 4` sets, queries, and an `OSC 104`
//! resets.

use crate::device::color::Rgb;
use crate::interpreter::osc::OscTerminator;

/// One request an `OSC 4` or `OSC 104` makes of the indexed palette,
/// decoded before the device is touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaletteRequest {
    /// Sets slot `index` to `color` (`OSC 4 ; c ; spec`).
    Set { index: u8, color: Rgb },
    /// Reports the colour slot `index` holds (`OSC 4 ; c ; ?`).
    Query { index: u8 },
    /// Returns slot `index` to its default (`OSC 104 ; c`).
    Reset { index: u8 },
    /// Returns every slot to its default (`OSC 104` with no number).
    ResetAll,
}

impl PaletteRequest {
    /// The palette requests an `OSC 4` or `OSC 104` carries, in the
    /// order they appear; empty for every other operating system
    /// command.
    ///
    /// `OSC 4` takes colour number and spec pairs, and "Any number of
    /// c/spec pairs may be given" (xterm-ctlseqs.pdf p.38); a `?` in
    /// place of the spec asks for the slot's colour instead of setting
    /// it. `OSC 104` takes colour numbers to reset, and "If no
    /// parameters are given, the entire table will be reset"
    /// (xterm-ctlseqs.pdf p.42).
    ///
    /// # Invariants
    ///
    /// A pair or number that cannot be read is dropped on its own and
    /// the rest still decode. A colour number of 256 or more is dropped
    /// rather than truncated to a byte. An unpaired trailing number is
    /// ignored.
    pub fn parse(params: &[&[u8]]) -> Vec<Self> {
        match params {
            [b"4", pairs @ ..] => pairs
                .chunks_exact(2)
                .filter_map(|pair| Self::from_pair(pair[0], pair[1]))
                .collect(),
            [b"104"] | [b"104", b""] => vec![Self::ResetAll],
            [b"104", numbers @ ..] => numbers
                .iter()
                .filter_map(|number| palette_index(number))
                .map(|index| Self::Reset { index })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The request one `OSC 4` colour number and spec pair makes; `None`
    /// when either half cannot be read.
    fn from_pair(number: &[u8], spec: &[u8]) -> Option<Self> {
        let index = palette_index(number)?;
        if spec == b"?" {
            return Some(Self::Query { index });
        }
        Rgb::from_color_spec(spec).map(|color| Self::Set { index, color })
    }
}

/// The reply an `OSC 4 ; c ; ?` owes: the same command with slot
/// `index`'s colour spelled as `rgb:`, closed the way the query was.
///
/// Each channel is written twice, which is the 16-bit value an `hh`
/// component scales to (xlib.pdf p.90), so an application that replays
/// the reply sets the slot back to the same colour.
pub(crate) fn palette_reply(index: u8, color: Rgb, terminator: OscTerminator) -> Vec<u8> {
    let Rgb { r, g, b } = color;
    let end = terminator.as_str();
    format!("\x1b]4;{index};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}{end}").into_bytes()
}

/// The palette slot a decimal colour number names; `None` for an empty
/// number, a byte other than a decimal digit, a sign included, and a
/// value past slot 255.
fn palette_index(number: &[u8]) -> Option<u8> {
    // NOTE: the charset is checked here rather than left to `parse`,
    // which also accepts a leading `+`, so `OSC 4 ; +1 ; ?` would read as
    // a query of slot 1.
    if number.is_empty() || !number.iter().all(u8::is_ascii_digit) {
        return None;
    }
    str::from_utf8(number).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Asserts that an `OSC 4` pair carrying a colour spec decodes to a set
    /// of that slot.
    ///
    /// Case: terminfo's `initc` recolors slot 1 with
    /// `OSC 4 ; 1 ; rgb:12/34/56`.
    #[test]
    fn an_osc_4_pair_decodes_to_a_set() {
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"1", b"rgb:12/34/56"]),
            vec![PaletteRequest::Set {
                index: 1,
                color: rgb(0x12, 0x34, 0x56)
            }]
        );
    }

    /// Asserts that a `?` in place of the spec decodes to a query of that
    /// slot.
    ///
    /// Case: a program asks for slot 1 before recoloring it, so that it can
    /// restore the slot on exit.
    #[test]
    fn a_question_mark_decodes_to_a_query() {
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"1", b"?"]),
            vec![PaletteRequest::Query { index: 1 }]
        );
    }

    /// Asserts that every query in one command decodes to its own request.
    ///
    /// Case: a program saves two slots by asking for both in a single
    /// command.
    #[test]
    fn each_query_in_one_command_decodes_separately() {
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"0", b"?", b"1", b"?"]),
            vec![
                PaletteRequest::Query { index: 0 },
                PaletteRequest::Query { index: 1 }
            ]
        );
    }

    /// Asserts that several pairs in one command decode in the order they
    /// appear, mixing sets and queries.
    ///
    /// Case: a theme script recolors two slots and asks for a third in one
    /// command.
    #[test]
    fn several_pairs_decode_in_order() {
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"1", b"rgb:ff/00/00", b"2", b"?", b"3", b"#00f"]),
            vec![
                PaletteRequest::Set {
                    index: 1,
                    color: rgb(0xff, 0x00, 0x00)
                },
                PaletteRequest::Query { index: 2 },
                PaletteRequest::Set {
                    index: 3,
                    color: rgb(0x00, 0x00, 0xf0)
                },
            ]
        );
    }

    /// Asserts that a pair whose spec cannot be read is dropped on its own
    /// while the pairs after it still decode.
    ///
    /// Case: a script written for xterm recolors slot 1 by the name `red`,
    /// which this terminal does not know, before recoloring slot 2.
    #[test]
    fn an_unreadable_spec_drops_only_its_own_pair() {
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"1", b"red", b"2", b"rgb:00/00/ff"]),
            vec![PaletteRequest::Set {
                index: 2,
                color: rgb(0x00, 0x00, 0xff)
            }]
        );
    }

    /// Asserts that the first and last slots of the 256-colour table both
    /// decode.
    ///
    /// Case: a theme script recolors the terminal's black and its last
    /// grayscale step in one pass.
    #[test]
    fn the_table_bounds_decode() {
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"0", b"?", b"255", b"?"]),
            vec![
                PaletteRequest::Query { index: 0 },
                PaletteRequest::Query { index: 255 }
            ]
        );
    }

    /// Asserts that a colour number past slot 255 is dropped rather
    /// than truncated to a byte, while slot 255 itself decodes and the
    /// pairs after a dropped one still decode.
    ///
    /// Case: a script written for xterm sets its special colors through
    /// `OSC 4 ; 256 ; …` to `OSC 4 ; 260 ; …`.
    #[test]
    fn a_color_number_past_the_table_is_dropped_rather_than_truncated() {
        assert!(
            PaletteRequest::parse(&[b"4", b"256", b"rgb:ff/ff/ff", b"260", b"?", b"300", b"?"])
                .is_empty()
        );
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"255", b"?"]),
            vec![PaletteRequest::Query { index: 255 }]
        );
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"256", b"?", b"2", b"?"]),
            vec![PaletteRequest::Query { index: 2 }]
        );
    }

    /// Asserts that a colour number too long for any integer is dropped
    /// rather than wrapped.
    ///
    /// Case: a corrupted stream delivers an `OSC 4` whose colour number
    /// runs to twenty digits.
    #[test]
    fn a_huge_color_number_is_dropped_rather_than_wrapped() {
        assert!(PaletteRequest::parse(&[b"4", b"18446744073709551617", b"?"]).is_empty());
    }

    /// Asserts that a colour number that is not a plain decimal is
    /// dropped.
    ///
    /// Case: a buggy script formats the colour number with a sign, a
    /// space, or a hex prefix, or leaves it out.
    #[test]
    fn a_malformed_color_number_is_dropped() {
        assert!(
            PaletteRequest::parse(&[
                b"4", b"", b"?", b"x", b"?", b"+1", b"?", b"-1", b"?", b" 1", b"?", b"0x1", b"?"
            ])
            .is_empty()
        );
    }

    /// Asserts that a colour number with leading zeros reads as its
    /// decimal value.
    ///
    /// Case: a script pads colour numbers to three digits and sends
    /// `007` for slot 7.
    #[test]
    fn leading_zeros_read_as_decimal() {
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"007", b"?"]),
            vec![PaletteRequest::Query { index: 7 }]
        );
    }

    /// Asserts that an unpaired trailing colour number is ignored while
    /// the pairs before it still decode.
    ///
    /// Case: the parser's parameter cap cuts a long `OSC 4` off between a
    /// colour number and its spec.
    #[test]
    fn an_unpaired_trailing_number_is_ignored() {
        assert_eq!(
            PaletteRequest::parse(&[b"4", b"1", b"?", b"2"]),
            vec![PaletteRequest::Query { index: 1 }]
        );
    }

    /// Asserts that an `OSC 4` with no pairs decodes to nothing.
    ///
    /// Case: a program emits a bare `OSC 4`.
    #[test]
    fn an_osc_4_without_pairs_decodes_to_nothing() {
        assert!(PaletteRequest::parse(&[b"4"]).is_empty());
    }

    /// Asserts that an `OSC 104` without a colour number, or with only
    /// an empty one, decodes to a reset of every slot.
    ///
    /// Case: `tput init` on an ncurses 6.6 entry sends `oc`, a bare
    /// `OSC 104`, and another program sends it with a stray trailing
    /// `;`.
    #[test]
    fn a_bare_osc_104_decodes_to_a_reset_of_every_slot() {
        assert_eq!(
            PaletteRequest::parse(&[b"104"]),
            vec![PaletteRequest::ResetAll]
        );
        assert_eq!(
            PaletteRequest::parse(&[b"104", b""]),
            vec![PaletteRequest::ResetAll]
        );
    }

    /// Asserts that an `OSC 104` carrying a colour number decodes to a
    /// reset of that slot.
    ///
    /// Case: a program restores the one slot it recolored before it
    /// exits.
    #[test]
    fn an_osc_104_number_decodes_to_a_reset() {
        assert_eq!(
            PaletteRequest::parse(&[b"104", b"1"]),
            vec![PaletteRequest::Reset { index: 1 }]
        );
    }

    /// Asserts that an `OSC 104` with several colour numbers decodes to a
    /// reset of each slot, in order.
    ///
    /// Case: a program restores the two slots it recolored before it
    /// exits.
    #[test]
    fn several_osc_104_numbers_reset_each_slot() {
        assert_eq!(
            PaletteRequest::parse(&[b"104", b"1", b"3"]),
            vec![
                PaletteRequest::Reset { index: 1 },
                PaletteRequest::Reset { index: 3 }
            ]
        );
    }

    /// Asserts that an `OSC 104` drops each unreadable colour number on
    /// its own, an empty one included, rather than resetting every
    /// slot.
    ///
    /// Case: a script restores slots from a list that holds an empty
    /// entry, a name, and an out-of-range number between the real ones.
    #[test]
    fn an_osc_104_drops_unreadable_numbers_individually() {
        assert_eq!(
            PaletteRequest::parse(&[b"104", b"", b"3", b"x", b"256", b"5"]),
            vec![
                PaletteRequest::Reset { index: 3 },
                PaletteRequest::Reset { index: 5 }
            ]
        );
    }

    /// Asserts that every other operating system command decodes to no
    /// palette request.
    ///
    /// Case: a shell sets its title while a script sets xterm's special
    /// and dynamic colors, which this terminal ignores or handles
    /// elsewhere.
    #[test]
    fn other_commands_decode_to_no_palette_request() {
        assert!(PaletteRequest::parse(&[b"0", b"title"]).is_empty());
        assert!(PaletteRequest::parse(&[b"5", b"0", b"?"]).is_empty());
        assert!(PaletteRequest::parse(&[b"10", b"?"]).is_empty());
        assert!(PaletteRequest::parse(&[b"105"]).is_empty());
        assert!(PaletteRequest::parse(&[]).is_empty());
    }

    /// Asserts that a reply writes each channel twice in lowercase hex
    /// and closes with BEL when asked to.
    ///
    /// Case: a program asks for the stock xterm red in slot 1 with a
    /// BEL-closed query.
    #[test]
    fn a_reply_doubles_each_channel_in_lowercase_hex() {
        assert_eq!(
            palette_reply(1, rgb(0xcd, 0x00, 0x00), OscTerminator::Bel),
            b"\x1b]4;1;rgb:cdcd/0000/0000\x07"
        );
    }

    /// Asserts that a reply closed with the string terminator uses its
    /// seven-bit form.
    ///
    /// Case: a program asks for slot 255 with an ST-closed query.
    #[test]
    fn an_st_reply_closes_with_the_seven_bit_string_terminator() {
        assert_eq!(
            palette_reply(255, rgb(0xab, 0x12, 0xef), OscTerminator::St),
            b"\x1b]4;255;rgb:abab/1212/efef\x1b\\"
        );
    }

    /// Asserts that a reply's spec reads back to the colour it reports,
    /// so replaying the reply restores the slot exactly.
    ///
    /// Case: a program saves a slot with a query and later restores it
    /// by sending the reply back as a set.
    #[test]
    fn a_reply_reads_back_to_the_color_it_reports() {
        for channel in [0x00, 0x01, 0x7f, 0x80, 0xcd, 0xfe, 0xff] {
            let color = rgb(channel, channel, channel);
            let reply = palette_reply(1, color, OscTerminator::Bel);
            let spec = reply
                .strip_prefix(b"\x1b]4;1;".as_slice())
                .and_then(|rest| rest.strip_suffix(b"\x07".as_slice()))
                .expect("the reply is framed as OSC 4 ; 1 ; spec BEL");
            assert_eq!(Rgb::from_color_spec(spec), Some(color));
        }
    }
}
