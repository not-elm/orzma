//! The operating system commands this terminal implements.
//!
//! The window title (OSC 0 and OSC 2) and the working directory (OSC 7)
//! are implemented; the palette, hyperlinks, and the clipboard land
//! later.

use std::path::PathBuf;

/// The directory an `OSC 7` reports, or `None` for every other
/// operating system command and for a URI this parser does not accept.
///
/// Only the `file` scheme is accepted, and only a URI that carries an
/// absolute path after its authority. The host — `localhost`, a real
/// hostname, or the empty host of `file:///…` — is ignored, because
/// every one of them names the local machine as far as a
/// working-directory report is concerned.
pub(crate) fn current_dir(params: &[&[u8]]) -> Option<PathBuf> {
    let [b"7", parts @ ..] = params else {
        return None;
    };
    let joined = parts.join(&b';');
    let uri = String::from_utf8_lossy(&joined);
    let rest = uri.strip_prefix("file://")?;
    let index = rest.find('/')?;
    // TODO: percent-encoded octets in the path (e.g. `%20` for a space)
    // are not decoded; the caller receives the escape sequence verbatim.
    Some(PathBuf::from(&rest[index..]))
}

/// The sanitized window title an `OSC 0` or `OSC 2` sets, or `None` for
/// every other operating system command. An `OSC 0` or `OSC 2` that
/// carries no text at all also returns `None`, rather than emptying the
/// title.
///
/// The parser splits an operating system command on every `;`, so a
/// title carrying one arrives in pieces and the tail is rejoined before
/// anything else runs.
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
/// that indents itself in the tab bar. A truncation trims once more
/// before appending the ellipsis, because the cut can land inside the
/// padding the first trim was too early to see.
///
/// Filtering and truncation are deliberately not grapheme-aware. The
/// stripped set includes U+200D ZERO WIDTH JOINER, so an emoji sequence
/// loses its joins, and a truncation can fall between a base character
/// and its combining marks. Both are accepted: the alternative is a
/// segmentation dependency this crate does not carry.
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
/// Three groups are refused. The control characters are refused because
/// a title is rendered as text and must not steer the surface drawing
/// it, and U+2028 and U+2029 are refused beside them because they break
/// a line without being control characters. The whole `Bidi_Control`
/// property — U+061C, U+200E..U+200F, U+202A..U+202E, and
/// U+2066..U+2069 — is refused because a title that reorders itself can
/// impersonate another program. And the characters that occupy no space
/// — U+00AD, U+180E, U+200B..U+200D, U+2060..U+2064, U+FEFF,
/// U+FFF9..U+FFFB, and the U+E0000..U+E007F tag block — are refused
/// because text the reader cannot see can hide inside a title that
/// looks benign.
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
}
