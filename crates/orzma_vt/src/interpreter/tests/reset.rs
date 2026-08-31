//! Tests for the hard terminal reset.

use super::*;

/// Asserts that `ESC c` blanks every visible row.
///
/// Case: a program dies mid-redraw and leaves the screen unusable,
/// so the user runs `reset` to take the terminal back.
#[test]
fn the_seven_bit_reset_blanks_every_visible_row() {
    let device = interpret(b"ab\r\nc\x1bc");
    for line in 0..3 {
        let row = device.active_screen().viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| *cell == Cell::default()));
    }
}

/// Asserts that the repaint `ESC c` calls for reaches the chunk
/// liveness rather than being dropped by the handler.
///
/// Case: the user runs `reset` on a screen a previous command filled,
/// and the owner must open its coalesce window for the frame that
/// repaints it.
#[test]
fn the_seven_bit_reset_marks_its_own_chunk_damaged() {
    assert!(liveness_after(b"a", b"\x1bc"));
}
