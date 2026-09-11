//! Tests that a sequence this terminal does not implement is skipped
//! rather than fatal, and that the parser returns to ground behind it.

use super::*;

/// Asserts that a control function this terminal does not implement
/// is ignored rather than fatal: it raises no chunk liveness and the
/// byte behind it prints where it would have anyway.
///
/// Case: a program emits a `CSI` sequence this terminal does not
/// implement and goes on printing behind it.
#[test]
fn an_unimplemented_sequence_is_ignored() {
    // NOTE: `_` (05/15) is a final byte ECMA-48 leaves unallocated, so no
    // later implementation can claim it. Repointing this probe at a
    // sequence orzma might one day implement retires the case silently:
    // the test keeps passing while it stops reaching `_ => {}`, which is
    // what happened when `ICH` was implemented under its predecessor.
    assert!(!damage_of(b"\x1b[5_"));
    let device = interpret(b"\x1b[5_a");
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
