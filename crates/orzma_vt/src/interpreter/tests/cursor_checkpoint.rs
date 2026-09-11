//! Tests for saving and restoring the cursor, through both the escape
//! pair and the private mode that shares its checkpoint.

use super::*;

/// Asserts that `ESC 7` and `ESC 8` bracket a detour, putting the
/// cursor back where the save found it.
///
/// Case: a program saves its cursor, moves away to write a line
/// elsewhere on the screen, restores, and continues where it left
/// off.
#[test]
fn the_seven_bit_save_and_restore_bracket_a_detour() {
    let device = interpret(b"ab\x1b7\r\x1bDxy\x1b8c");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'x'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[2].c,
        'c'
    );
    assert_eq!(device.active_screen().cursor_column(), GridColumn(3));
}

/// Asserts that `?1048h` and `?1048l` save and restore the cursor
/// exactly as `ESC 7` and `ESC 8` do, the restore keeping the chunk
/// live through the cursor move.
///
/// Case: a program parks the cursor with `?1048h`, draws elsewhere,
/// and brings it back with `?1048l`.
#[test]
fn mode_1048_saves_and_restores_the_cursor() {
    let mut session = Session::new();
    session.feed(b"\x1b[1;2H\x1b[?1048h\x1b[1;4H");
    let output = session.feed(b"\x1b[?1048l");
    assert_eq!(session.cursor_column(), 1);
    assert!(output.damaged);
}

/// Asserts that `?1048` acts on the active screen's own DECSC slot,
/// so a save on the alternate screen leaves the primary screen's
/// untouched.
///
/// Case: a shell saves its cursor, a full-screen program saves its
/// own with `?1048h` while on the alternate screen, and the shell
/// restores after the program exits.
#[test]
fn mode_1048_uses_the_active_screens_own_checkpoint() {
    let mut session = Session::new();
    session.feed(b"\x1b[1;2H\x1b7\x1b[?47h\x1b[1;4H\x1b[?1048h\x1b[?47l\x1b8");
    assert_eq!(session.cursor_column(), 1);
}

/// Asserts that `CSI s` and `CSI u` bracket a detour exactly as `ESC 7`
/// and `ESC 8` do, putting the cursor back where the save found it.
///
/// Case: a program written against ANSI.SYS saves its cursor, writes a
/// line elsewhere on the screen, restores, and continues where it left
/// off.
#[test]
fn the_sco_save_and_restore_bracket_a_detour() {
    let device = interpret(b"ab\x1b[s\r\x1bDxy\x1b[uc");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'x'
    );
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[2].c,
        'c'
    );
    assert_eq!(device.active_screen().cursor_column(), GridColumn(3));
}

/// Asserts that a `CSI s` carrying parameters is ignored rather than
/// saved over the slot `ESC 7` filled.
///
/// Case: a program saves its cursor with `ESC 7`, emits `CSI 1 ; 2 s`
/// meant as left and right margins for a terminal that has them, and
/// then restores with the SCO spelling.
#[test]
fn a_sco_save_carrying_parameters_is_ignored() {
    let device = interpret(b"\x1b[1;2H\x1b7\x1b[1;4H\x1b[1;2s\x1b[1;3H\x1b[u");
    assert_eq!(device.active_screen().cursor_column(), GridColumn(1));
}

/// Asserts that a `CSI u` carrying a parameter is ignored rather than
/// restoring the saved cursor.
///
/// Case: a program emits a parameterized `CSI u` this terminal does not
/// define, after saving its cursor with `CSI s` and moving away.
#[test]
fn a_sco_restore_carrying_parameters_is_ignored() {
    let device = interpret(b"\x1b[1;2H\x1b[s\x1b[1;4H\x1b[1u");
    assert_eq!(device.active_screen().cursor_column(), GridColumn(3));
}

/// Asserts that a kitty keyboard protocol request, which shares the `u`
/// final byte behind a private marker, restores nothing.
///
/// Case: a TUI pushes, pops, sets, and queries its keyboard enhancement
/// flags after saving the cursor with `CSI s` and moving away.
#[test]
fn a_kitty_keyboard_request_does_not_restore_the_cursor() {
    for request in [&b"\x1b[>1u"[..], b"\x1b[<u", b"\x1b[=1;1u", b"\x1b[?u"] {
        let mut session = Session::new();
        session.feed(b"\x1b[1;2H\x1b[s\x1b[1;4H");
        session.feed(request);
        assert_eq!(session.cursor_column(), 3, "{request:?} restores nothing");
    }
}
