//! Tests for the private modes that select a mouse tracking level and
//! its report encoding.

use super::*;

/// Asserts that each mouse tracking number selects its own level,
/// the levels replacing one another.
///
/// Case: an editor raises its tracking from clicks to any-event
/// motion when the user starts a drag selection.
#[test]
fn each_mouse_tracking_number_selects_its_level() {
    assert_eq!(
        interpret(b"\x1b[?1000h").modes().mouse_tracking,
        MouseTracking::Clicks
    );
    assert_eq!(
        interpret(b"\x1b[?1002h").modes().mouse_tracking,
        MouseTracking::Drag
    );
    assert_eq!(
        interpret(b"\x1b[?1000h\x1b[?1003h").modes().mouse_tracking,
        MouseTracking::Motion
    );
}

/// Asserts that resetting a tracking number that is not the active
/// level leaves that level alone.
///
/// Case: an application tears down every tracking mode it knows,
/// including ones it never set.
#[test]
fn resetting_an_inactive_tracking_number_keeps_the_active_level() {
    assert_eq!(
        interpret(b"\x1b[?1002h\x1b[?1000l").modes().mouse_tracking,
        MouseTracking::Drag
    );
    assert_eq!(
        interpret(b"\x1b[?1002h\x1b[?1002l").modes().mouse_tracking,
        MouseTracking::Off
    );
}

/// Asserts that `DECSET 1006` selects SGR reports and its reset
/// returns to the default framing.
///
/// Case: an application asks for SGR reports so it can address a
/// window wider than the legacy coordinate cap.
#[test]
fn the_sgr_mouse_number_selects_its_encoding() {
    assert_eq!(
        interpret(b"\x1b[?1006h").modes().mouse_encoding,
        MouseEncoding::Sgr
    );
    assert_eq!(
        interpret(b"\x1b[?1006h\x1b[?1006l").modes().mouse_encoding,
        MouseEncoding::X10
    );
}

/// Asserts that `DECSET 1005` is not answered, leaving the default
/// framing in force.
///
/// Case: an application asks for the UTF-8 coordinate extension as
/// it starts tracking the mouse.
#[test]
fn the_utf8_mouse_number_is_not_answered() {
    assert_eq!(
        interpret(b"\x1b[?1005h").modes().mouse_encoding,
        MouseEncoding::X10
    );
}
