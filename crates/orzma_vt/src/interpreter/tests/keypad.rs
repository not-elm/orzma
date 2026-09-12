//! Tests for the keypad mode that decides which sequences the numeric
//! keypad sends.

use super::*;

/// Asserts that `ESC =` puts the keypad in application mode.
///
/// Case: a full-screen editor starts up and takes the numeric keypad
/// over so that its own bindings receive those keys.
#[test]
fn keypad_application_mode_selects_application_sequences() {
    let device = interpret(b"\x1b=");
    assert_eq!(device.modes().keypad_mode, KeypadMode::Application);
}

/// Asserts that `ESC >` puts the keypad back in numeric mode.
///
/// Case: a full-screen editor exits and hands the keypad back to the
/// shell.
#[test]
fn keypad_numeric_mode_selects_ascii_numerals() {
    let device = interpret(b"\x1b=\x1b>");
    assert_eq!(device.modes().keypad_mode, KeypadMode::Numeric);
}

/// Asserts that `CSI ? 66 h` selects application mode, the same
/// state `ESC =` selects.
///
/// Case: an application that manages its modes through the request
/// and report pair takes the numeric keypad over as it starts up.
#[test]
fn the_numeric_keypad_mode_set_matches_keypad_application_mode() {
    let device = interpret(b"\x1b[?66h");
    assert_eq!(device.modes().keypad_mode, KeypadMode::Application);
}

/// Asserts that `CSI ? 66 l` selects numeric mode, the same state
/// `ESC >` selects.
///
/// Case: the same application hands the numeric keypad back as it
/// shuts down.
#[test]
fn the_numeric_keypad_mode_reset_matches_keypad_numeric_mode() {
    let device = interpret(b"\x1b=\x1b[?66l");
    assert_eq!(device.modes().keypad_mode, KeypadMode::Numeric);
}
