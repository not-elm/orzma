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
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(screen.fill_alignment_pattern(), DamageSpan::Full);
    for line in 0..3 {
        let row = screen.viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| cell.c == 'E'));
    }
}

/// Asserts that the alignment pattern returns the scrolling margins to
/// the extremes of the page.
///
/// Case: a full-screen application has reserved a status line with
/// `CSI 2 ; 3 r` when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_widens_the_margins_to_the_page() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.fill_alignment_pattern();
    assert_eq!(screen.scroll_region.top_margin(), ScreenLine(0));
    assert_eq!(screen.scroll_region.bottom_margin(), ScreenLine(3));
}

/// Asserts that the alignment pattern seats the cursor at the home
/// position.
///
/// Case: the cursor sits in the middle of the screen when `ESC # 8`
/// arrives.
#[test]
fn an_alignment_pattern_seats_the_cursor_at_home() {
    let mut screen = screen();
    screen.move_cursor_to(Some(2), Some(3));
    screen.fill_alignment_pattern();
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that the alignment pattern disarms an armed deferred wrap.
///
/// Case: the cursor has just printed into the last column, leaving the
/// wrap pending, when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_disarms_the_deferred_wrap() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    assert!(screen.state.pending_wrap);
    screen.fill_alignment_pattern();
    assert!(!screen.state.pending_wrap);
}

/// Asserts that the alignment pattern returns the cursor origin to the
/// upper-left corner.
///
/// Case: an application has turned on origin mode inside a reserved
/// pane when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_returns_the_origin_to_the_corner() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen
        .scroll_region
        .set_origin_mode(OriginMode::WithinMargins);
    screen.fill_alignment_pattern();
    assert_eq!(
        screen.scroll_region.origin_mode(),
        OriginMode::UpperLeftCorner
    );
}

/// Asserts that the alignment pattern leaves the scrollback history
/// untouched.
///
/// Case: an earlier command's output has scrolled off the top of the
/// screen when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_leaves_the_history_untouched() {
    let mut screen = screen();
    screen.print('a', InsertReplaceMode::Replace, AutoWrap::Enabled);
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    screen.fill_alignment_pattern();
    screen.viewport.offset = DisplayOffset(1);
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, 'a');
}

/// Asserts that the alignment pattern is drawn with default attributes
/// rather than the current pen.
///
/// Case: an application has selected a red background for a banner and
/// has not restored the rendition when the alignment pattern arrives.
#[test]
fn an_alignment_pattern_ignores_the_pen() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(1);
    screen.print('x', InsertReplaceMode::Replace, AutoWrap::Enabled);
    screen.fill_alignment_pattern();
    let expected = Cell {
        c: 'E',
        ..Cell::default()
    };
    for line in 0..3 {
        let row = screen.viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| *cell == expected));
    }
}
