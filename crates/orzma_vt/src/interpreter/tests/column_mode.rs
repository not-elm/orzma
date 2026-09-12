//! Tests that the column mode is ignored, so an initialization string
//! neither erases the page nor returns the scrolling margins.

use super::*;

/// Asserts that neither column-mode direction erases the page or moves
/// the cursor.
///
/// Case: a script runs `tput init`, whose `is2` string carries
/// `CSI ? 3 l`, while the shell's earlier output is still on screen.
#[test]
fn neither_column_mode_direction_erases_the_page() {
    for chunk in [&b"a\r\nb\x1b[?3hc"[..], b"a\r\nb\x1b[?3lc"] {
        let device = interpret(chunk);
        assert_eq!(glyph_at(&device, 0, 0), 'a', "{chunk:?} keeps the page");
        assert_eq!(glyph_at(&device, 1, 0), 'b', "{chunk:?} keeps the page");
        assert_eq!(glyph_at(&device, 1, 1), 'c', "{chunk:?} keeps the cursor");
    }
}

/// Asserts that neither column-mode direction returns the scrolling
/// margins to the full page, leaving a later linefeed scrolling against
/// the region the application set.
///
/// Case: a full-screen application reserves the last row for a status
/// line, and a `CSI ? 3 l` reaches it from an initialization string a
/// child process emitted.
#[test]
fn neither_column_mode_direction_returns_the_margins() {
    for chunk in [&b"\x1b[1;2r\x1b[?3ha\n\nb"[..], b"\x1b[1;2r\x1b[?3la\n\nb"] {
        let device = interpret(chunk);
        assert_eq!(glyph_at(&device, 1, 1), 'b', "{chunk:?} keeps the region");
        assert_eq!(glyph_at(&device, 2, 1), ' ', "{chunk:?} keeps the region");
    }
}

/// Asserts that a column mode reaching the alternate screen erases
/// neither that screen nor the one behind it, and leaves the alternate
/// screen shown.
///
/// Case: a user runs `tput init` from a full-screen editor's shell
/// escape while the editor's page is on the alternate screen.
#[test]
fn a_column_mode_leaves_the_alternate_screen_alone() {
    let mut session = Session::new();
    session.feed(b"a\x1b[?1049hb\x1b[?3lc");
    assert_eq!(session.active_screen(), ScreenKind::Alternate);
    assert_eq!(session.char_at(0, 0), 'b');
    assert_eq!(session.char_at(0, 1), 'c');

    session.feed(b"\x1b[?1049l");
    assert_eq!(session.char_at(0, 0), 'a');
}

/// Asserts that neither column-mode direction raises chunk liveness, a
/// reply, or a signal.
///
/// Case: a legacy VT application selects 132 columns as it starts and
/// returns to 80 columns as it exits.
#[test]
fn neither_column_mode_direction_raises_anything() {
    for request in [&b"\x1b[?3h"[..], b"\x1b[?3l"] {
        let output = interpret_fully(request).1;
        assert!(!output.damaged, "{request:?} raises no liveness");
        assert!(output.replies.is_empty(), "{request:?} sends no reply");
        assert!(output.signals.is_empty(), "{request:?} raises no signal");
    }
}
