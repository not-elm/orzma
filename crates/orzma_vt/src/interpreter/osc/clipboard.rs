//! The system clipboard an `OSC 52` sets or clears.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

/// What one `OSC 52` does to the system clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClipboardRequest {
    /// Makes the text the clipboard's content.
    Set(String),
    /// Clears the clipboard.
    Clear,
}

impl ClipboardRequest {
    /// The request an `OSC 52` makes of the system clipboard, or `None`
    /// for every other operating system command and for one that leaves
    /// the clipboard untouched.
    ///
    /// `Pc` names the selections to act on, and this terminal has one
    /// system clipboard: it acts for `c`, for `s`, and for an empty `Pc`,
    /// which stands for `s0` (xterm-ctlseqs.pdf p.40-41, "If the parameter
    /// is empty, xterm uses s 0"). A `Pc` naming only `p`, `q`, or cut
    /// buffers leaves the clipboard untouched whatever `Pd` holds, and one
    /// carrying a byte outside `cpqs01234567` is refused whole rather than
    /// read for the bytes that do belong to the set.
    ///
    /// `Pd` is everything after `Pc`, `;` included. Base64 (RFC 4648) with
    /// canonical padding sets the clipboard to the text it decodes to, and
    /// an empty `Pd` sets it to the empty string. A missing `Pd` — which
    /// includes an `OSC 52` carrying no parameter at all — or one that is
    /// not base64 clears the clipboard: "If the second parameter
    /// is neither a base64 string nor ?, then the selection is cleared"
    /// (xterm-ctlseqs.pdf p.40). A `Pd` that decodes to bytes which are not
    /// UTF-8 clears it as well, rather than writing replacement characters.
    /// A `Pd` of `?` queries the clipboard, which this terminal does not
    /// answer.
    ///
    /// TODO: answer the `?` query, which needs a clipboard read to reach
    /// the reply path.
    pub fn parse(params: &[&[u8]]) -> Option<Self> {
        let [b"52", rest @ ..] = params else {
            return None;
        };
        let selections = rest.first().copied().unwrap_or_default();
        if !targets_clipboard(selections) {
            return None;
        }
        let [_, data] = rest else {
            return Some(Self::Clear);
        };
        if *data == b"?" {
            return None;
        }
        let text = BASE64
            .decode(data)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        Some(text.map_or(Self::Clear, Self::Set))
    }
}

/// The selection characters an `OSC 52` may name: the clipboard, the
/// primary and secondary selections, the configurable select target,
/// and cut buffers 0 through 7 (xterm-ctlseqs.pdf p.40).
const SELECTIONS: &[u8] = b"cpqs01234567";

/// Whether an `OSC 52` naming `selections` acts on the one system
/// clipboard this terminal has.
fn targets_clipboard(selections: &[u8]) -> bool {
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

    /// Asserts that `s0`, xterm's empty-parameter default spelled out,
    /// sets the system clipboard even though it also names a cut buffer.
    ///
    /// Case: a program spells out xterm's default target instead of
    /// leaving the selection list empty.
    #[test]
    fn the_select_target_sets_the_clipboard() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"s0", b"aGk="]),
            Some(ClipboardRequest::Set("hi".to_owned()))
        );
    }

    /// Asserts that an omitted selection list stands for `s0` and sets
    /// the system clipboard.
    ///
    /// Case: a shell helper script leaves the selection field empty and
    /// relies on the terminal's default target.
    #[test]
    fn an_omitted_selection_list_sets_the_clipboard() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"", b"aGk="]),
            Some(ClipboardRequest::Set("hi".to_owned()))
        );
    }

    /// Asserts that a selection list naming only the primary selection
    /// leaves the clipboard untouched.
    ///
    /// Case: a user yanks into Neovim's `*` register on macOS, where the
    /// host has no primary selection to write.
    #[test]
    fn the_primary_selection_target_leaves_the_clipboard_untouched() {
        assert_eq!(ClipboardRequest::parse(&[b"52", b"p", b"aGk="]), None);
    }

    /// Asserts that selection lists naming only the secondary selection
    /// or a cut buffer leave the clipboard untouched.
    ///
    /// Case: a program written for an X11 host stores its yank in a cut
    /// buffer, which this terminal has no counterpart for.
    #[test]
    fn the_secondary_and_cut_buffer_targets_leave_the_clipboard_untouched() {
        assert_eq!(ClipboardRequest::parse(&[b"52", b"q", b"aGk="]), None);
        assert_eq!(ClipboardRequest::parse(&[b"52", b"0", b"aGk="]), None);
    }

    /// Asserts that a selection list naming no clipboard target leaves
    /// the clipboard untouched even when its payload is not base64,
    /// rather than clearing it.
    ///
    /// Case: a program clears its primary selection with a malformed
    /// payload on macOS, where the host has no primary selection.
    #[test]
    fn a_bad_payload_for_another_selection_leaves_the_clipboard_untouched() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"p", b"not base64!"]),
            None
        );
    }

    /// Asserts that a selection list sets the system clipboard when the
    /// clipboard appears anywhere in it, not only at its head.
    ///
    /// Case: a copy helper asks for the primary selection and the
    /// clipboard at once by sending both characters.
    #[test]
    fn a_selection_list_containing_the_clipboard_sets_it() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"pc", b"aGk="]),
            Some(ClipboardRequest::Set("hi".to_owned()))
        );
    }

    /// Asserts that a selection list carrying a character outside the
    /// recognized set is refused whole, rather than read for the
    /// characters that do belong to it.
    ///
    /// Case: a program with a formatting bug emits a stray character ahead
    /// of the clipboard target.
    #[test]
    fn a_selection_list_outside_the_recognized_set_leaves_the_clipboard_untouched() {
        assert_eq!(ClipboardRequest::parse(&[b"52", b"xc", b"aGk="]), None);
    }

    /// Asserts that a payload which is not base64 clears the clipboard.
    ///
    /// Case: a user cats a binary file whose bytes happen to open an
    /// operating system command for the clipboard.
    #[test]
    fn a_payload_that_is_not_base64_clears_the_clipboard() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"c", b"not base64!"]),
            Some(ClipboardRequest::Clear)
        );
    }

    /// Asserts that a payload missing its canonical padding is not base64
    /// and clears the clipboard.
    ///
    /// Case: a hand-written shell helper strips the trailing `=` from the
    /// text it encodes.
    #[test]
    fn an_unpadded_payload_clears_the_clipboard() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"c", b"aGk"]),
            Some(ClipboardRequest::Clear)
        );
    }

    /// Asserts that an empty payload sets the clipboard to the empty
    /// string rather than clearing it.
    ///
    /// Case: a program publishes an empty yank, such as a zero-length
    /// visual selection.
    #[test]
    fn an_empty_payload_sets_the_empty_string() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"c", b""]),
            Some(ClipboardRequest::Set(String::new()))
        );
    }

    /// Asserts that a command carrying no payload field at all clears
    /// the clipboard.
    ///
    /// Case: a script builds the sequence by concatenation and the
    /// variable holding the payload separator is unset.
    #[test]
    fn a_command_without_a_payload_clears_the_clipboard() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"c"]),
            Some(ClipboardRequest::Clear)
        );
    }

    /// Asserts that an `OSC 52` carrying no parameter at all reads as an
    /// empty `Pc` with no `Pd`, and clears the clipboard.
    ///
    /// Case: a script sends the bare command number to drop whatever it
    /// copied earlier.
    #[test]
    fn a_command_without_any_parameter_clears_the_clipboard() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52"]),
            Some(ClipboardRequest::Clear)
        );
    }

    /// Asserts that a clipboard query leaves the clipboard untouched.
    ///
    /// Case: a program checks whether it can read the clipboard back
    /// before deciding how to implement its paste command.
    #[test]
    fn a_clipboard_query_leaves_the_clipboard_untouched() {
        assert_eq!(ClipboardRequest::parse(&[b"52", b"c", b"?"]), None);
    }

    /// Asserts that a payload decoding to bytes which are not UTF-8
    /// clears the clipboard.
    ///
    /// Case: a program yanks a region of a file it opened as binary, and
    /// encodes the raw bytes.
    #[test]
    fn a_payload_that_is_not_utf8_clears_the_clipboard() {
        assert_eq!(
            ClipboardRequest::parse(&[b"52", b"c", b"//4="]),
            Some(ClipboardRequest::Clear)
        );
    }
}
