//! Tests for the screen alignment pattern.

use super::*;

/// Asserts that `ESC # 8` fills every visible row with the alignment
/// pattern.
///
/// Case: a service technician sends the alignment pattern to judge
/// the geometry of a display showing a half-drawn prompt.
#[test]
fn the_alignment_pattern_fills_every_visible_row() {
    let device = interpret(b"ab\r\nc\x1b#8");
    for line in 0..3 {
        let row = device.active_screen().viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| cell.c == 'E'));
    }
}

/// Asserts that the repaint `ESC # 8` calls for reaches the chunk
/// liveness.
///
/// Case: a service technician sends the alignment pattern to a
/// screen an earlier command already printed on.
#[test]
fn the_alignment_pattern_marks_its_own_chunk_damaged() {
    assert!(liveness_after(b"a", b"\x1b#8"));
}
