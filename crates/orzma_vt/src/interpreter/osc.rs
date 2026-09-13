//! The operating system commands this terminal implements.
//!
//! The window title (OSC 0 and OSC 2), the working directory (OSC 7),
//! the indexed palette (OSC 4 and OSC 104), the dynamic foreground and
//! background (OSC 10, OSC 11, OSC 110, and OSC 111), and the clipboard
//! write (OSC 52) are implemented.
//!
//! TODO: implement the dynamic cursor color (OSC 12 and OSC 112) and
//! hyperlinks (OSC 8).

pub(crate) mod dynamic_color;
pub(crate) mod palette;

use crate::device::color::Rgb;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
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

/// The text an `OSC 52` writes to the system clipboard, or `None` for
/// every other operating system command and for one that writes
/// nothing. An empty string clears the clipboard.
///
/// `Pc` names the selections to write, and this terminal has one system
/// clipboard to write them to: it writes for `c`, for `s`, and for an
/// empty `Pc`, which stands for `s0` (xterm-ctlseqs.pdf p.40-41, "If
/// the parameter is empty, xterm uses s 0"). A `Pc` naming only `p`, `q`,
/// or cut buffers writes nothing, and one carrying a byte outside
/// `cpqs01234567` is refused whole rather than read for the bytes that
/// do belong to the set.
///
/// `Pd` is base64 (RFC 4648) and must carry canonical padding. A `Pd`
/// that is not base64, or that decodes to bytes which are not UTF-8,
/// leaves the clipboard untouched rather than clearing it. A `Pd` of
/// `?` queries the clipboard, which this terminal does not answer.
///
/// TODO: answer the `?` query, which needs a clipboard read to reach
/// the reply path.
pub(crate) fn clipboard_text(params: &[&[u8]]) -> Option<String> {
    let [b"52", selections, data] = params else {
        return None;
    };
    if !writes_clipboard(selections) || *data == b"?" {
        return None;
    }
    String::from_utf8(BASE64.decode(data).ok()?).ok()
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

/// The `rgb:` spelling a colour reply carries.
///
/// Each channel is written twice, which is the 16-bit value an `hh`
/// component scales to (xlib.pdf p.90), so an application that replays
/// a reply sets the colour back to the one the reply reported.
pub(crate) fn rgb_spec(color: Rgb) -> String {
    let Rgb { r, g, b } = color;
    format!("rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}")
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

/// The selection characters an `OSC 52` may name: the clipboard, the
/// primary and secondary selections, the configurable select target,
/// and cut buffers 0 through 7 (xterm-ctlseqs.pdf p.40).
const SELECTIONS: &[u8] = b"cpqs01234567";

/// Whether an `OSC 52` naming `selections` writes the one system
/// clipboard this terminal has.
fn writes_clipboard(selections: &[u8]) -> bool {
    if selections.is_empty() {
        return true;
    }
    selections.iter().all(|target| SELECTIONS.contains(target))
        && selections
            .iter()
            .any(|target| matches!(target, b'c' | b's'))
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

    /// Asserts that `s0`, xterm's empty-parameter default spelled out,
    /// writes the system clipboard even though it also names a cut
    /// buffer.
    ///
    /// Case: a program spells out xterm's default target instead of
    /// leaving the selection list empty.
    #[test]
    fn the_select_target_writes_the_clipboard() {
        assert_eq!(
            clipboard_text(&[b"52", b"s0", b"aGk="]),
            Some("hi".to_owned())
        );
    }

    /// Asserts that an omitted selection list stands for `s0` and writes
    /// the system clipboard.
    ///
    /// Case: a shell helper script leaves the selection field empty and
    /// relies on the terminal's default target.
    #[test]
    fn an_omitted_selection_list_writes_the_clipboard() {
        assert_eq!(
            clipboard_text(&[b"52", b"", b"aGk="]),
            Some("hi".to_owned())
        );
    }

    /// Asserts that a selection list naming only the primary selection
    /// writes nothing.
    ///
    /// Case: a user yanks into Neovim's `*` register on macOS, where the
    /// host has no primary selection to write.
    #[test]
    fn the_primary_selection_target_writes_nothing() {
        assert!(clipboard_text(&[b"52", b"p", b"aGk="]).is_none());
    }

    /// Asserts that selection lists naming only the secondary selection
    /// or a cut buffer write nothing.
    ///
    /// Case: a program written for an X11 host stores its yank in a cut
    /// buffer, which this terminal has no counterpart for.
    #[test]
    fn the_secondary_and_cut_buffer_targets_write_nothing() {
        assert!(clipboard_text(&[b"52", b"q", b"aGk="]).is_none());
        assert!(clipboard_text(&[b"52", b"0", b"aGk="]).is_none());
    }

    /// Asserts that a selection list writes the system clipboard when the
    /// clipboard appears anywhere in it, not only at its head.
    ///
    /// Case: a copy helper asks for the primary selection and the
    /// clipboard at once by sending both characters.
    #[test]
    fn a_selection_list_containing_the_clipboard_writes_it() {
        assert_eq!(
            clipboard_text(&[b"52", b"pc", b"aGk="]),
            Some("hi".to_owned())
        );
    }

    /// Asserts that a selection list carrying a character outside the
    /// recognized set is refused whole, rather than read for the
    /// characters that do belong to it.
    ///
    /// Case: a program with a formatting bug emits a stray character ahead
    /// of the clipboard target.
    #[test]
    fn a_selection_list_outside_the_recognized_set_writes_nothing() {
        assert!(clipboard_text(&[b"52", b"xc", b"aGk="]).is_none());
    }

    /// Asserts that a payload which is not base64 leaves the clipboard
    /// untouched rather than clearing it.
    ///
    /// Case: a user cats a binary file whose bytes happen to open an
    /// operating system command, while the clipboard holds something the
    /// user still wants.
    #[test]
    fn a_payload_that_is_not_base64_leaves_the_clipboard_untouched() {
        assert!(clipboard_text(&[b"52", b"c", b"not base64!"]).is_none());
    }

    /// Asserts that a payload missing its canonical padding writes
    /// nothing.
    ///
    /// Case: a hand-written shell helper strips the trailing `=` from the
    /// text it encodes.
    #[test]
    fn an_unpadded_payload_writes_nothing() {
        assert!(clipboard_text(&[b"52", b"c", b"aGk"]).is_none());
    }

    /// Asserts that an empty payload writes the empty string, which
    /// clears the clipboard.
    ///
    /// Case: a program drops the selection it published earlier once the
    /// user closes the buffer it came from.
    #[test]
    fn an_empty_payload_clears_the_clipboard() {
        assert_eq!(clipboard_text(&[b"52", b"c", b""]), Some(String::new()));
    }

    /// Asserts that a command carrying no payload field at all writes
    /// nothing.
    ///
    /// Case: a script builds the sequence by concatenation and the
    /// variable holding the payload separator is unset.
    #[test]
    fn a_command_without_a_payload_writes_nothing() {
        assert!(clipboard_text(&[b"52", b"c"]).is_none());
    }

    /// Asserts that a clipboard query is not treated as text to store.
    ///
    /// Case: a program checks whether it can read the clipboard back
    /// before deciding how to implement its paste command.
    #[test]
    fn a_clipboard_query_writes_nothing() {
        assert!(clipboard_text(&[b"52", b"c", b"?"]).is_none());
    }

    /// Asserts that a payload decoding to bytes which are not UTF-8
    /// writes nothing.
    ///
    /// Case: a program yanks a region of a file it opened as binary, and
    /// encodes the raw bytes.
    #[test]
    fn a_payload_that_is_not_utf8_writes_nothing() {
        assert!(clipboard_text(&[b"52", b"c", b"//4="]).is_none());
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
}
