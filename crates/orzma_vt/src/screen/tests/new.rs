//! Tests for constructing a screen.

use super::*;

/// Asserts that a fresh screen starts at the origin, pinned to the
/// live tail, with an empty history.
///
/// Case: a terminal spawns and the shell prints its first prompt.
#[test]
fn a_fresh_screen_starts_at_the_origin() {
    let screen = screen();
    assert_eq!(
        (screen.state.line, screen.state.column),
        (ScreenLine(0), GridColumn(0))
    );
    assert_eq!(screen.display_offset(), DisplayOffset(0));
    assert_eq!(screen.grid.history_len(), 0);
}
