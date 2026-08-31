//! Tests for how the private-mode parameter list is processed, apart
//! from what any one mode does.

use super::*;

/// Asserts that the flag-shaped private modes reach their fields
/// on set and go back on reset.
///
/// Case: a full-screen application turns on the modes it needs at
/// startup and turns them off again on the way out.
#[test]
fn the_flag_private_modes_reach_their_fields() {
    let device = interpret(b"\x1b[?1;1004;1007;2004h");
    let modes = device.modes();
    assert!(modes.app_cursor);
    assert!(modes.focus_in_out);
    assert!(modes.alternate_scroll);
    assert!(modes.bracketed_paste);

    let device = interpret(b"\x1b[?1;1004;1007;2004h\x1b[?1;1004;1007;2004l");
    let modes = device.modes();
    assert!(!modes.app_cursor);
    assert!(!modes.focus_in_out);
    assert!(!modes.alternate_scroll);
    assert!(!modes.bracketed_paste);
}

/// Asserts that an unknown private mode is ignored rather than
/// disturbing the modes around it.
///
/// Case: an application probes for a feature this terminal does not
/// implement while other modes are already in force.
#[test]
fn an_unknown_private_mode_is_ignored() {
    let device = interpret(b"\x1b[?2004h\x1b[?9999h");
    assert!(device.modes().bracketed_paste);
}

/// Asserts that a private mode this terminal does not implement does
/// not hide one it does.
///
/// Case: an application turns on application cursor keys and origin
/// mode in a single `CSI ? 1 ; 6 h`.
#[test]
fn an_unimplemented_private_mode_does_not_hide_origin_mode() {
    let device = interpret(b"\x1b[2;3r\x1b[?1;6h\x1b[1;1Hx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'x'
    );
}
