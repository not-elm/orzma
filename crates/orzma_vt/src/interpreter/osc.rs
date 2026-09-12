//! The operating system commands this terminal implements.
//!
//! The window title (OSC 0 and OSC 2), the working directory (OSC 7),
//! and the indexed palette (OSC 4 and OSC 104) are implemented.
//!
//! TODO: implement the dynamic colors (OSC 10 / 11 / 12), hyperlinks,
//! and the clipboard.

use crate::device::color::Rgb;
use percent_encoding::percent_decode;
use std::path::PathBuf;

/// The directory an `OSC 7` reports, or `None` for every other
/// operating system command and for a URI this parser does not accept.
///
/// Only the `file` scheme is accepted, and only a URI that carries an
/// absolute path after its authority. The host — `localhost`, a real
/// hostname, or the empty host of `file:///…` — is ignored.
/// Percent-encoded octets in the path (`%20` for a space, the UTF-8
/// octets of a non-ASCII name) are decoded.
pub(crate) fn current_dir(params: &[&[u8]]) -> Option<PathBuf> {
    let [b"7", parts @ ..] = params else {
        return None;
    };
    let joined = parts.join(&b';');
    let rest = joined.strip_prefix(b"file://")?;
    let index = rest.iter().position(|byte| *byte == b'/')?;
    let decoded = percent_decode(&rest[index..]).decode_utf8_lossy();
    Some(PathBuf::from(decoded.into_owned()))
}

/// The sanitized window title an `OSC 0` or `OSC 2` sets, or `None` for
/// every other operating system command. An `OSC 0` or `OSC 2` that
/// carries no text at all also returns `None`, rather than emptying the
/// title.
///
/// The command arrives split on every `;`, and the pieces after the
/// number are rejoined, so a title keeps its semicolons.
pub(crate) fn window_title(params: &[&[u8]]) -> Option<String> {
    let [b"0" | b"2", text @ ..] = params else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    Some(sanitize(&String::from_utf8_lossy(&text.join(&b';'))))
}

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

/// How an operating system command was closed, which a reply to it
/// echoes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OscTerminator {
    /// BEL (`0x07`).
    Bel,
    /// The string terminator, whether it arrived as `ESC \`, as `0x9C`,
    /// or as any other byte that ends the command.
    St,
}

impl OscTerminator {
    /// The terminator the byte that ended an operating system command
    /// stands for.
    pub const fn from_byte(byte: u8) -> Self {
        if byte == 0x07 { Self::Bel } else { Self::St }
    }

    /// The text a reply closes with. The string terminator is always
    /// the seven-bit `ESC \`.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Bel => "\x07",
            Self::St => "\x1b\\",
        }
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

/// Maximum length, in `char`s, of a sanitized title.
const MAX_LEN: usize = 256;

/// Returns a display-safe copy of an operating system command's title.
///
/// Edge whitespace is trimmed after stripping, including whitespace a
/// stripped character exposed, and a truncation drops the whitespace it
/// cuts against before appending the ellipsis.
///
/// Filtering and truncation are not grapheme-aware. The stripped set
/// includes U+200D ZERO WIDTH JOINER, so an emoji sequence loses its
/// joins, and a truncation can fall between a base character and its
/// combining marks.
fn sanitize(raw: &str) -> String {
    let stripped: String = raw.chars().filter(|c| !is_disallowed(*c)).collect();
    let trimmed = stripped.trim();
    let mut boundary = trimmed.char_indices().skip(MAX_LEN - 1);
    let Some((cut, _)) = boundary.next() else {
        return trimmed.to_owned();
    };
    if boundary.next().is_none() {
        return trimmed.to_owned();
    }
    let mut truncated = trimmed[..cut].to_owned();
    truncated.truncate(truncated.trim_end().len());
    truncated.push('…');
    truncated
}

/// Whether a character must not reach a window title.
///
/// Three groups are refused: the control characters, together with
/// U+2028 and U+2029; the whole `Bidi_Control` property (U+061C,
/// U+200E..U+200F, U+202A..U+202E, and U+2066..U+2069); and the
/// characters that occupy no space (U+00AD, U+180E, U+200B..U+200D,
/// U+2060..U+2064, U+FEFF, U+FFF9..U+FFFB, and the U+E0000..U+E007F tag
/// block).
fn is_disallowed(c: char) -> bool {
    // NOTE: the arms are grouped to match the three groups the doc
    // above names, so the two can be checked against each other line by
    // line. Merging the adjacent ranges across groups would be shorter
    // and would break that correspondence.
    c.is_control()
        || matches!(c, '\u{2028}'..='\u{2029}')
        || matches!(
            c,
            '\u{061C}' | '\u{200E}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
        || matches!(
            c,
            '\u{00AD}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200D}'
                | '\u{2060}'..='\u{2064}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{E0000}'..='\u{E007F}'
        )
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

    /// Asserts that a `file://` URI naming an explicit host reports the
    /// path after it.
    ///
    /// Case: a shell reports its directory with `OSC 7
    /// file://localhost/tmp/project` after a `cd`.
    #[test]
    fn a_localhost_uri_reports_its_path() {
        assert_eq!(
            current_dir(&[b"7", b"file://localhost/tmp/project"]),
            Some(PathBuf::from("/tmp/project"))
        );
    }

    /// Asserts that a `file://` URI with an empty host reports the same
    /// path as one naming `localhost`.
    ///
    /// Case: a shell reports its directory with the terser
    /// `file:///tmp/project` form some prompts emit instead.
    #[test]
    fn an_empty_host_uri_reports_its_path() {
        assert_eq!(
            current_dir(&[b"7", b"file:///tmp/project"]),
            Some(PathBuf::from("/tmp/project"))
        );
    }

    /// Asserts that a URI outside the `file` scheme reports no directory.
    ///
    /// Case: a misbehaving program sends `OSC 7` with an `http://` URI
    /// instead of a local path.
    #[test]
    fn a_foreign_scheme_is_rejected() {
        assert!(current_dir(&[b"7", b"http://localhost/tmp/project"]).is_none());
    }

    /// Asserts that a `file://` URI with no path after its host reports
    /// no directory.
    ///
    /// Case: a shell emits `OSC 7 file://localhost` without ever naming a
    /// directory.
    #[test]
    fn a_uri_missing_a_path_is_rejected() {
        assert!(current_dir(&[b"7", b"file://localhost"]).is_none());
    }

    /// Asserts that percent-encoded octets in the path are decoded, so
    /// a directory with a space or a non-ASCII name comes back as the
    /// directory itself, while a stray `%` is kept verbatim.
    ///
    /// Case: a fish or zsh integration reports `cd ~/My Project/ドキュメント`
    /// with every reserved and non-ASCII byte escaped.
    #[test]
    fn percent_encoded_octets_in_the_path_are_decoded() {
        assert_eq!(
            current_dir(&[
                b"7",
                b"file:///Users/x/My%20Project/%E3%83%89%E3%82%AD%E3%83%A5"
            ]),
            Some(PathBuf::from("/Users/x/My Project/ドキュ"))
        );
        assert_eq!(
            current_dir(&[b"7", b"file:///tmp/100%25/x%2"]),
            Some(PathBuf::from("/tmp/100%/x%2"))
        );
    }

    /// Asserts that OSC 0 and OSC 2 set the same window title.
    ///
    /// Case: one shell prompt sets the title with `OSC 0` and another
    /// with `OSC 2`, and both must reach the same tab.
    #[test]
    fn osc_zero_and_osc_two_set_the_same_title() {
        assert_eq!(window_title(&[b"0", b"hi"]).as_deref(), Some("hi"));
        assert_eq!(window_title(&[b"2", b"hi"]).as_deref(), Some("hi"));
    }

    /// Asserts that an icon name request sets no window title.
    ///
    /// Case: oh-my-zsh sends `OSC 1` for the tab and `OSC 2` for the
    /// window.
    #[test]
    fn an_icon_name_sets_no_title() {
        assert!(window_title(&[b"1", b"hi"]).is_none());
    }

    /// Asserts that a title containing a semicolon survives whole.
    ///
    /// Case: a shell puts `user@host: ~/src; make` in the title, and the
    /// parser hands the pieces over already split.
    #[test]
    fn a_semicolon_in_the_title_is_rejoined() {
        assert_eq!(window_title(&[b"0", b"a", b"b"]).as_deref(), Some("a;b"));
    }

    /// Asserts that an empty parameter list sets no title.
    ///
    /// Case: a program emits a bare `ESC ] BEL` with nothing inside it.
    #[test]
    fn an_empty_parameter_list_sets_no_title() {
        assert!(window_title(&[]).is_none());
    }

    /// Asserts that a leading empty field sets no title, which is how a
    /// numberless command arrives once the parser has collapsed it.
    ///
    /// Case: a program emits `ESC ] ; foo BEL`, omitting the number.
    #[test]
    fn a_numberless_command_sets_no_title() {
        assert!(window_title(&[b"foo"]).is_none());
    }

    /// Asserts that a command carrying no text at all sets no title,
    /// rather than emptying it.
    ///
    /// Case: a program emits `ESC ] 0 BEL`, omitting the separator and
    /// the text after it.
    #[test]
    fn a_command_without_text_sets_no_title() {
        assert!(window_title(&[b"0"]).is_none());
    }

    /// Asserts that an empty title is carried through as an empty
    /// string rather than being treated as a reset.
    ///
    /// Case: a shell blanks its prompt title by emitting `OSC 0` with
    /// the separator but nothing after it.
    #[test]
    fn an_empty_title_stays_empty() {
        assert_eq!(window_title(&[b"0", b""]).as_deref(), Some(""));
    }

    /// Asserts that control characters are stripped from the title.
    ///
    /// Case: a hostile program embeds a DEL in the title to disturb the
    /// tab bar.
    #[test]
    fn control_characters_are_stripped() {
        assert_eq!(window_title(&[b"0", b"a\x7fb"]).as_deref(), Some("ab"));
    }

    /// Asserts that every Unicode bidi control is stripped, including
    /// the Arabic letter mark.
    ///
    /// Case: a hostile program uses a bidi override to make the title
    /// read as a different program's name.
    #[test]
    fn bidi_controls_are_stripped() {
        let raw = "a\u{061c}\u{200e}\u{202e}\u{2066}b".as_bytes();
        assert_eq!(window_title(&[b"0", raw]).as_deref(), Some("ab"));
    }

    /// Asserts that the Unicode line and paragraph separators are
    /// stripped wherever they sit, as the C0 line breaks already are.
    ///
    /// Case: a hostile program embeds a line separator in the title so
    /// the tab bar draws a second line under the one beside it.
    #[test]
    fn unicode_line_breaks_are_stripped() {
        let raw = "a\u{2028}\u{2029}b".as_bytes();
        assert_eq!(window_title(&[b"0", raw]).as_deref(), Some("ab"));
    }

    /// Asserts that the characters occupying no space outside
    /// U+200B..U+200D are stripped too, including the word joiner that
    /// replaced U+FEFF and the tag block.
    ///
    /// Case: a hostile program hides tag-encoded text inside a title
    /// that reads as another program's name.
    #[test]
    fn invisible_characters_are_stripped() {
        let raw = "a\u{00ad}\u{180e}\u{2060}\u{fff9}\u{e0041}b".as_bytes();
        assert_eq!(window_title(&[b"0", raw]).as_deref(), Some("ab"));
    }

    /// Asserts that whitespace exposed by stripping a zero-width
    /// character is trimmed, so a title cannot carry indentation.
    ///
    /// Case: a hostile program prefixes the title with a zero-width
    /// space and a real space to indent it in the tab bar.
    #[test]
    fn whitespace_exposed_by_stripping_is_trimmed() {
        let raw = "\u{200b} hi".as_bytes();
        assert_eq!(window_title(&[b"0", raw]).as_deref(), Some("hi"));
    }

    /// Asserts that an over-long title is truncated with an ellipsis.
    ///
    /// Case: a hostile program sets a title long enough to push every
    /// other tab off the bar.
    #[test]
    fn an_over_long_title_is_truncated() {
        let raw = "x".repeat(1000);
        let title = window_title(&[b"0", raw.as_bytes()]).expect("OSC 0 sets a title");
        assert_eq!(title.chars().count(), 256);
        assert!(title.ends_with('…'));
    }

    /// Asserts that a truncation drops the whitespace it cut against
    /// instead of leaving it in front of the ellipsis.
    ///
    /// Case: a hostile program pads a long title so the cut lands in a
    /// run of spaces and the tab reads as indented from its ellipsis.
    #[test]
    fn a_truncation_trims_before_the_ellipsis() {
        let raw = format!("{}{}{}", "x".repeat(250), " ".repeat(10), "y".repeat(100));
        let title = window_title(&[b"0", raw.as_bytes()]).expect("OSC 0 sets a title");
        assert_eq!(title, format!("{}…", "x".repeat(250)));
    }

    /// Asserts that a title of exactly the maximum length passes through
    /// untouched, rather than being truncated at the boundary.
    ///
    /// Case: a program sets a title exactly `MAX_LEN` characters long.
    #[test]
    fn a_title_at_the_max_length_is_not_truncated() {
        let raw = "x".repeat(MAX_LEN);
        let title = window_title(&[b"0", raw.as_bytes()]).expect("OSC 0 sets a title");
        assert_eq!(title.chars().count(), MAX_LEN);
        assert!(!title.ends_with('…'));
    }

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

    /// Asserts that a command closed by BEL is answered with BEL.
    ///
    /// Case: a shell script closes its query with BEL, as most do.
    #[test]
    fn a_bel_closed_command_is_answered_with_bel() {
        assert_eq!(OscTerminator::from_byte(0x07), OscTerminator::Bel);
    }

    /// Asserts that every other byte that closes a command is answered
    /// with the string terminator.
    ///
    /// Case: terminfo closes its commands with `ESC \`, an eight-bit
    /// program with a raw `0x9C`, and a cancelled command ends on CAN.
    #[test]
    fn every_other_closing_byte_is_answered_with_st() {
        for byte in [0x1b, 0x9c, 0x18] {
            assert_eq!(OscTerminator::from_byte(byte), OscTerminator::St);
        }
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
