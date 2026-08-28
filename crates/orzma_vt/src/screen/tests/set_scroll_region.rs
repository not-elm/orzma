//! Tests for the `DECSTBM` scrolling region.

use super::*;

/// Asserts that a resolved region reaches the scroll span the
/// line feed and reverse index scroll against.
///
/// Case: an application reserves a status line on the last row
/// of a four-row screen.
#[test]
fn a_resolved_region_reaches_the_scroll_span() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(3));
    assert_eq!(
        screen.scroll_region.scroll_span(),
        ScreenLine(0)..=ScreenLine(2)
    );
}

/// Asserts that applying a region seats the cursor at the
/// origin-aware home rather than VT510's "column 1, line 1 of
/// the page".
///
/// Case: an application sets a region while its cursor sits
/// somewhere in the middle of the screen.
#[test]
fn applying_a_region_seats_the_cursor_at_home() {
    let mut screen = tall_screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    screen.set_scroll_region(Some(1), Some(3));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that home follows the origin mode rather than the
/// page.
///
/// Case: an application turns on origin mode and then moves its
/// pane down the screen with a second region.
#[test]
fn home_follows_the_origin_mode() {
    let mut screen = tall_screen();
    screen
        .scroll_region
        .set_origin_mode(OriginMode::WithinMargins);
    screen.set_scroll_region(Some(2), Some(4));
    assert_eq!(screen.state.line, ScreenLine(1));
}

/// Asserts that a refused request leaves both the margins and
/// the cursor untouched — a whole-sequence no-op rather than a
/// partial application.
///
/// Case: an application inverts its two parameters and sends
/// `CSI 5 ; 3 r`.
#[test]
fn a_refused_request_changes_nothing() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(3));
    screen.state.line = ScreenLine(2);
    screen.set_scroll_region(Some(5), Some(3));
    assert_eq!(
        screen.scroll_region.scroll_span(),
        ScreenLine(0)..=ScreenLine(2)
    );
    assert_eq!(screen.state.line, ScreenLine(2));
}
