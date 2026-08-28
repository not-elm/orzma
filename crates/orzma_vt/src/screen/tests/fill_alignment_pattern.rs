//! Tests for the screen alignment pattern.

use super::*;

/// Asserts that the alignment pattern reaches every visible cell and
/// reports the whole screen as damaged.
///
/// Case: a service technician sends `ESC # 8` to a terminal showing a
/// half-drawn prompt, to get a uniform field to judge the display
/// against.
#[test]
fn an_alignment_pattern_fills_every_visible_cell() {
    let mut screen = screen();
    screen.print('x');
    assert_eq!(screen.fill_alignment_pattern(), DamageSpan::Full);
    for line in 0..3 {
        let row = screen.viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| cell.c == 'E'));
    }
}
