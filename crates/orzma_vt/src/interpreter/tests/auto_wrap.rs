//! Tests for `DECAWM`, the autowrap mode `CSI ? 7 h` and `CSI ? 7 l`
//! select between.

use super::*;

/// Asserts that, absent an earlier `DECSC`, a reset autowrap keeps a
/// run longer than the row on the row it started on, replacing the
/// last column.
///
/// Case: a status-bar program sends `CSI ? 7 l` and then writes a label
/// wider than the terminal.
#[test]
fn a_reset_autowrap_keeps_a_long_run_on_one_row() {
    let device = interpret(b"\x1b[?7labcdef");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'b', 'c', 'f']);
    assert_eq!(glyph_at(&device, 1, 0), ' ');
}

/// Asserts that a deferred wrap `DECSC` saved before a reset autowrap
/// survives the round trip verbatim and wraps once autowrap returns.
///
/// Case: an application fills a row, saves the cursor, turns autowrap
/// off and back on, restores the cursor, and prints one more
/// character.
#[test]
fn a_decsc_saved_deferred_wrap_survives_a_reset_autowrap_round_trip() {
    let device = interpret(b"abcd\x1b7\x1b[?7l\x1b[?7h\x1b8e");
    assert_eq!(glyph_at(&device, 1, 0), 'e');
}

/// Asserts that a set autowrap after a reset one does not cash in a
/// deferred wrap armed before the reset.
///
/// Case: an application fills a row, turns autowrap off and on again,
/// and prints one more character.
#[test]
fn a_set_autowrap_does_not_cash_in_a_wrap_armed_before_the_reset() {
    let device = interpret(b"abcd\x1b[?7l\x1b[?7he");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'b', 'c', 'e']);
    assert_eq!(glyph_at(&device, 1, 0), ' ');
}

/// Asserts that a redundant set autowrap leaves an armed deferred wrap
/// alone, so the next character still wraps.
///
/// Case: an application fills a row and re-sends `CSI ? 7 h` while
/// autowrap is already on.
#[test]
fn a_redundant_set_autowrap_leaves_an_armed_wrap_alone() {
    let device = interpret(b"abcd\x1b[?7he");
    assert_eq!(glyph_at(&device, 1, 0), 'e');
}

/// Asserts that the mode is device-wide: a reset on the primary screen
/// is still in force on the alternate screen.
///
/// Case: a full-screen application turns autowrap off, then enters the
/// alternate screen and draws to the right border.
#[test]
fn the_mode_carries_across_the_alternate_screen() {
    let device = interpret(b"\x1b[?7l\x1b[?1049habcdef");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'b', 'c', 'f']);
    assert_eq!(glyph_at(&device, 1, 0), ' ');
}

/// Asserts that `RIS` returns autowrap to its power-up value rather
/// than leaving a reset one in force.
///
/// Case: a program turns autowrap off and exits without restoring it,
/// and the shell issues a hard reset.
#[test]
fn a_hard_reset_returns_autowrap_to_the_power_up_value() {
    let device = interpret(b"\x1b[?7l\x1bcabcde");
    assert_eq!(glyph_at(&device, 1, 0), 'e');
}

/// Asserts that an erase to the end of the line still erases after a
/// `DECRC` restored a deferred wrap while autowrap is reset.
///
/// Case: an application fills a row, saves the cursor, turns autowrap
/// off, restores the cursor, and clears to the end of the line.
#[test]
fn an_erase_to_end_runs_after_a_restore_while_autowrap_is_reset() {
    let device = interpret(b"abcd\x1b7\x1b[?7l\x1b8\x1b[K");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'b', 'c', ' ']);
}

/// Asserts that erasing characters still erases after a `DECRC`
/// restored a deferred wrap while autowrap is reset.
///
/// Case: the same application saves and restores the cursor around a
/// reset of autowrap, then clears the character under the cursor with
/// `ECH` instead of clearing to the end of the line.
#[test]
fn an_erase_of_characters_runs_after_a_restore_while_autowrap_is_reset() {
    let device = interpret(b"abcd\x1b7\x1b[?7l\x1b8\x1b[X");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'b', 'c', ' ']);
}

/// Asserts that an erase to the end of the line still erases on the
/// primary screen after a bare alternate-screen round trip during which
/// autowrap was reset, and that the round trip disarmed the primary
/// screen's own deferred wrap: once autowrap returns, the next print
/// replaces the last column rather than wrapping.
///
/// Case: an application fills a row, enters the alternate screen with
/// `CSI ? 47 h`, turns autowrap off there, returns, clears to the end
/// of the line, turns autowrap back on, and prints one more character.
#[test]
fn an_erase_to_end_runs_after_a_bare_alternate_screen_round_trip() {
    let device = interpret(b"abcd\x1b[?47h\x1b[?7l\x1b[?47l\x1b[K\x1b[?7he");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'b', 'c', 'e']);
    assert_eq!(glyph_at(&device, 1, 0), ' ');
}

/// Asserts that an erase to the end of the line still erases on the
/// primary screen after a `DECSET 1049` round trip, which restores the
/// saved cursor and with it the saved deferred wrap.
///
/// Case: the same application makes the round trip through mode 1049
/// rather than mode 47, so the return restores the saved cursor.
#[test]
fn an_erase_to_end_runs_after_a_mode_1049_round_trip() {
    let device = interpret(b"abcd\x1b[?1049h\x1b[?7l\x1b[?1049l\x1b[K");
    assert_eq!(first_row_glyphs(&device), vec!['a', 'b', 'c', ' ']);
}

/// Asserts that a tabulation back from an armed deferred wrap, a save,
/// a reset and a restore leave no wrap for the next print to cash in.
///
/// Case: an application fills a row, tabs backwards, saves the cursor,
/// turns autowrap off, restores, and prints.
#[test]
fn a_backward_tab_before_a_save_leaves_no_wrap_to_cash_in() {
    let device = interpret(b"abcd\x1b[Z\x1b7\x1b[?7l\x1b8x");
    assert_eq!(first_row_glyphs(&device), vec!['x', 'b', 'c', 'd']);
    assert_eq!(glyph_at(&device, 1, 0), ' ');
}

/// Asserts that a reset autowrap stops the bottom row of the scrolling
/// region from scrolling when the last column is written.
///
/// Case: a program turns autowrap off and draws a full-width status
/// line on the bottom row.
#[test]
fn a_reset_autowrap_does_not_scroll_from_the_bottom_row() {
    let device = interpret(b"zz\x1b[?7l\x1b[3;1Habcdef");
    assert_eq!(first_row_glyphs(&device), vec!['z', 'z', ' ', ' ']);
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(2))[3].c,
        'f'
    );
}
