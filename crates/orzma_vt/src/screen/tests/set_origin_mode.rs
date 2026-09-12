//! Tests for the `DECOM` cursor origin.

use super::*;

/// Asserts that setting the origin within the margins seats the
/// cursor at the top margin.
///
/// Case: an application reserves a header row, then turns on
/// origin mode so its own coordinates start below it.
#[test]
fn setting_the_origin_seats_the_cursor_at_the_top_margin() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.state.line = ScreenLine(3);
    screen.set_origin_mode(OriginMode::WithinMargins);
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that resetting the origin homes the cursor at the
/// upper-left corner rather than homing on set alone.
///
/// Case: a full-screen application drops origin mode on its way
/// out and prints without addressing the cursor first.
#[test]
fn resetting_the_origin_seats_the_cursor_at_the_corner() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(4));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.state.line = ScreenLine(3);
    screen.set_origin_mode(OriginMode::UpperLeftCorner);
    assert_eq!(screen.state.line, ScreenLine(0));
}

/// Asserts that the mode reaches the region the cursor motion
/// reads.
///
/// Case: an application turns on origin mode before addressing
/// the cursor.
#[test]
fn the_mode_reaches_the_scroll_region() {
    let mut screen = tall_screen();
    screen.set_origin_mode(OriginMode::WithinMargins);
    assert_eq!(
        screen.scroll_region.origin_mode(),
        OriginMode::WithinMargins
    );
}
