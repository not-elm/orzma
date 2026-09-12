//! Tests for the soft terminal reset.

use super::*;
use crate::device::modes::{AutoWrap, KeypadMode};

/// Asserts that a soft reset shows a cursor an application hid.
///
/// Case: a full-screen program dies with the caret hidden and the user
/// runs `tput init` to get the terminal back.
#[test]
fn a_soft_reset_shows_a_hidden_cursor() {
    let device = interpret(b"\x1b[?25l\x1b[!p");
    assert!(device.cursor().visible);
}

/// Asserts that a soft reset returns the terminal to replace mode.
///
/// Case: a program turns insert mode on to shift a row right and exits
/// before turning it back off.
#[test]
fn a_soft_reset_returns_the_terminal_to_replace_mode() {
    let device = interpret(b"\x1b[4h\x1b[!p");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}

/// Asserts that a soft reset returns autowrap to enabled rather than to
/// the `No autowrap` of vt510.pdf p.277 Table 5-9.
///
/// Case: a status-bar program turns autowrap off to draw a full-width
/// label, and the shell runs `tput init` behind it.
#[test]
fn a_soft_reset_re_enables_autowrap() {
    let device = interpret(b"\x1b[?7l\x1b[!p");
    assert_eq!(device.modes().auto_wrap, AutoWrap::Enabled);
}

/// Asserts that a soft reset returns the arrow keys to their normal
/// encoding.
///
/// Case: a full-screen editor puts the cursor keys in application mode
/// and the shell resets the terminal after it exits.
#[test]
fn a_soft_reset_returns_the_cursor_keys_to_normal() {
    let device = interpret(b"\x1b[?1h\x1b[!p");
    assert!(!device.modes().app_cursor);
}

/// Asserts that a soft reset returns the keypad to numeric.
///
/// Case: a program puts the keypad in application mode with `ESC =` and
/// the shell resets the terminal after it exits.
#[test]
fn a_soft_reset_returns_the_keypad_to_numeric() {
    let device = interpret(b"\x1b=\x1b[!p");
    assert_eq!(device.modes().keypad_mode, KeypadMode::Numeric);
}

/// Asserts that a soft reset which shows a hidden cursor raises the
/// chunk liveness.
///
/// Case: the terminal sits idle with the caret hidden when `tput init`
/// arrives in a read of its own.
#[test]
fn a_soft_reset_that_shows_the_cursor_raises_the_liveness() {
    assert!(liveness_after(b"\x1b[?25l", b"\x1b[!p"));
}

/// Asserts that a soft reset which only returns the pen stages no
/// damage.
///
/// Case: a program leaves a coloured pen set and `tput init` arrives
/// with nothing on screen that needs repainting.
#[test]
fn a_soft_reset_that_only_returns_the_pen_stages_no_damage() {
    assert!(!liveness_after(b"\x1b[1;31m", b"\x1b[!p"));
}

/// Asserts that a soft reset returns a recoloured indexed palette slot
/// to its built-in default.
///
/// Case: a colour-scheme script recolours the palette with `OSC 4` and
/// the shell runs `tput init` afterwards.
#[test]
fn a_soft_reset_returns_the_indexed_palette_to_its_default() {
    let recoloured = replies_of(b"\x1b]4;1;rgb:0102/0304/0506\x1b\\\x1b]4;1;?\x1b\\");
    let after_reset = replies_of(b"\x1b]4;1;rgb:0102/0304/0506\x1b\\\x1b[!p\x1b]4;1;?\x1b\\");
    let untouched = replies_of(b"\x1b]4;1;?\x1b\\");
    assert_ne!(recoloured, untouched);
    assert_eq!(after_reset, untouched);
}

/// Asserts that a soft reset which returns a recoloured palette stages
/// the repaint that owes.
///
/// Case: a colour-scheme script recolours the palette and `tput init`
/// arrives in a read of its own, with the old colours still on screen.
#[test]
fn a_soft_reset_that_returns_the_palette_stages_a_repaint() {
    assert!(liveness_after(
        b"\x1b]4;1;rgb:0102/0304/0506\x1b\\",
        b"\x1b[!p"
    ));
}
