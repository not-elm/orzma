//! The operating system commands this terminal implements.
//!
//! Only the window title (OSC 0 and OSC 2) is implemented; the palette,
//! the working directory, hyperlinks, and the clipboard land later.

use std::iter::once;

/// The sanitized window title an `OSC 0` or `OSC 2` sets, or `None` for
/// every other operating system command.
///
/// The title is sanitized before it leaves this function because
/// `OSC 0` and `OSC 2` content is fully attacker-controlled, and
/// [`crate::VtSignal::Title`] carries it across the crate boundary.
pub(crate) fn window_title(params: &[&[u8]]) -> Option<String> {
    let [b"0" | b"2", text @ ..] = params else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    Some(sanitize(&String::from_utf8_lossy(&text.join(&b';'))))
}

/// Maximum length, in `char`s, of a sanitized title.
const MAX_LEN: usize = 256;

/// Returns a display-safe copy of an operating system command's title.
///
/// Stripping runs before trimming: removing a zero-width character can
/// expose fresh edge whitespace, and trimming first would leave a title
/// that indents itself in the tab bar.
///
/// Filtering and truncation are deliberately not grapheme-aware. The
/// stripped set includes U+200D ZERO WIDTH JOINER, so an emoji sequence
/// loses its joins, and a truncation can fall between a base character
/// and its combining marks. Both are accepted: the alternative is a
/// segmentation dependency this crate does not carry.
fn sanitize(raw: &str) -> String {
    let stripped: String = raw.chars().filter(|c| !is_disallowed(*c)).collect();
    let trimmed = stripped.trim();
    if trimmed.chars().count() > MAX_LEN {
        trimmed.chars().take(MAX_LEN - 1).chain(once('…')).collect()
    } else {
        trimmed.to_owned()
    }
}

/// Whether a character must not reach a window title.
///
/// Three groups are refused. The control characters, because a title is
/// rendered as text and must not steer the surface drawing it. The
/// whole `Bidi_Control` property — U+061C, U+200E..U+200F,
/// U+202A..U+202E, and U+2066..U+2069 — because a title that reorders
/// itself can impersonate another program. And the zero-width
/// characters U+200B..U+200D and U+FEFF, because text that occupies no
/// space can hide inside a title that looks benign.
fn is_disallowed(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// window, and this terminal carries only one title.
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
    /// Case: a shell clears the title with `OSC 0` and no text.
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
}
