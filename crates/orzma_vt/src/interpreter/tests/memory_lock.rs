//! Tests that HP memory lock is ignored, so no row is held out of
//! scrolling.

use super::*;

/// Asserts that `ESC l` leaves the rows above the cursor free to scroll
/// rather than locking them in place.
///
/// Case: a stray `ESC l` reaches the terminal while a shell is filling
/// the screen, and the output goes on scrolling up past the top row.
#[test]
fn memory_lock_holds_no_row_out_of_scrolling() {
    let device = interpret(b"a\r\nb\x1bl\n\n");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[0].c,
        'b'
    );
}

/// Asserts that neither `ESC l` nor `ESC m` raises chunk liveness, a
/// reply, or a signal.
///
/// Case: a program written for an HP terminal locks the top of the
/// screen as it starts and unlocks it again as it exits.
#[test]
fn neither_memory_lock_nor_unlock_raises_anything() {
    for request in [&b"\x1bl"[..], b"\x1bm"] {
        let output = interpret_fully(request).1;
        assert!(!output.damaged, "{request:?} raises no liveness");
        assert!(output.replies.is_empty(), "{request:?} sends no reply");
        assert!(output.signals.is_empty(), "{request:?} raises no signal");
    }
}
