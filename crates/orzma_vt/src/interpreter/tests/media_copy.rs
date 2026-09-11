//! Tests that the media copy family is ignored, so a printer request
//! neither hides the output behind it nor answers.

use super::*;

/// Asserts that `CSI 5 i` leaves the terminal displaying what follows
/// rather than entering printer controller mode and diverting it to a
/// printer.
///
/// Case: a user `cat`s a binary file whose bytes happen to spell
/// `CSI 5 i` and goes on reading the text printed behind it.
#[test]
fn printer_controller_mode_is_never_entered() {
    let device = interpret(b"\x1b[5iab");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].c, 'a');
    assert_eq!(row[1].c, 'b');
}

/// Asserts that every media copy is ignored rather than answered: none
/// raises chunk liveness, a reply, or a signal.
///
/// Case: an application written for a terminal with a printer attached
/// requests a screen print, toggles printer controller and autoprint
/// modes, and asks for xterm's HTML and SVG screen dumps.
#[test]
fn every_media_copy_is_ignored() {
    for request in [
        &b"\x1b[i"[..],
        b"\x1b[0i",
        b"\x1b[4i",
        b"\x1b[5i",
        b"\x1b[10i",
        b"\x1b[11i",
        b"\x1b[?1i",
        b"\x1b[?4i",
        b"\x1b[?5i",
    ] {
        let output = interpret_fully(request).1;
        assert!(!output.damaged, "{request:?} raises no liveness");
        assert!(output.replies.is_empty(), "{request:?} sends no reply");
        assert!(output.signals.is_empty(), "{request:?} raises no signal");
    }
}
