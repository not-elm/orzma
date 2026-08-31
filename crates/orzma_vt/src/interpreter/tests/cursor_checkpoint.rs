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
