//! Tests that a sequence this terminal does not implement is skipped
//! rather than fatal, and that the parser returns to ground behind it.

use super::*;

/// Asserts that a control function this terminal does not implement
/// is ignored rather than fatal.
///
/// The agreed policy follows what VT terminals do with sequences
/// they do not implement. It is also the point of the dispatcher:
/// before it existed every CSI sequence reached a `todo!()`.
///
/// Case: a program inserts blanks with `ICH` on a terminal that has
/// no character-editing functions yet.
#[test]
fn an_unimplemented_sequence_is_ignored() {
    let device = interpret(b"\x1b[2@a");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
}

/// Asserts that a device control string is ignored rather than
/// fatal, and that the parser returns to ground behind it.
///
/// Case: an application opens a Sixel image on a terminal that has
/// no DCS handlers.
#[test]
fn a_device_control_string_is_ignored_rather_than_fatal() {
    let device = interpret(b"\x1bP0q\x1b\\a");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'a'
    );
}
