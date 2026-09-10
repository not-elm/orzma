//! Tests for erasure across the display.

use super::*;

/// Asserts that erase-below clears from the cursor cell to the end
/// of the screen, leaving earlier content in place.
///
/// Case: a full-screen application redraws everything under the
/// cursor with `ED 0` while the rows above stay intact.
#[test]
fn erase_display_below_clears_from_the_cursor_down() {
    let mut screen = screen();
    screen.print('a', InsertReplaceMode::Replace);
    screen.line_feed();
    screen.carriage_return();
    for c in ['b', 'c'] {
        screen.print(c, InsertReplaceMode::Replace);
    }
    screen.state.column = GridColumn(1);
    let damage = screen.erase_in_display(EraseScreenMode::Below);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(1)][0].c, 'b');
    assert_eq!(screen.grid[ScreenLine(1)][1].c, ' ');
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(1), ViewportLine(2)))
    );
}

/// Asserts that erase-above clears everything through the cursor
/// cell inclusively, leaving the rest of the cursor row intact.
///
/// Case: a full-screen application discards everything already
/// drawn above and left of the cursor with `ED 1`.
#[test]
fn erase_display_above_clears_through_the_cursor() {
    let mut screen = screen();
    screen.print('a', InsertReplaceMode::Replace);
    screen.line_feed();
    screen.carriage_return();
    for c in ['b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace);
    }
    screen.state.column = GridColumn(1);
    let damage = screen.erase_in_display(EraseScreenMode::Above);
    assert_eq!(screen.grid[ScreenLine(0)][0].c, ' ');
    assert_eq!(screen.grid[ScreenLine(1)][0].c, ' ');
    assert_eq!(screen.grid[ScreenLine(1)][1].c, ' ');
    assert_eq!(screen.grid[ScreenLine(1)][2].c, 'd');
    assert_eq!(
        damage,
        Some(DamageSpan::rows(ViewportLine(0), ViewportLine(1)))
    );
}

/// Asserts that erase-all clears the visible screen in place while
/// scrollback history survives.
///
/// The agreed policy is the classic xterm behavior: `ED 2` erases
/// in place and does not push the cleared rows into history (a
/// deliberate divergence from alacritty, which scrolls them out
/// first).
///
/// Case: the user runs `clear` in a session that already
/// accumulated scrollback, then scrolls back to check older
/// output.
#[test]
fn erase_display_all_clears_the_screen_but_not_history() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.carriage_return();
    for c in ['a', 'b'] {
        screen.print(c, InsertReplaceMode::Replace);
    }
    let damage = screen.erase_in_display(EraseScreenMode::All);
    assert_eq!(screen.grid[ScreenLine(2)][0].c, ' ');
    assert_eq!(screen.grid[ScreenLine(2)][1].c, ' ');
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(damage, Some(DamageSpan::Full));
}

/// Asserts that a span reaching past the last visible row is clamped
/// rather than reported out of range.
///
/// Case: the user scrolls back and the application erases from the
/// cursor to the bottom of the screen.
#[test]
fn a_span_running_past_the_viewport_is_clamped_to_its_last_row() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.set_display_offset(DisplayOffset(1));
    screen.state.line = ScreenLine(0);
    assert_eq!(
        screen.erase_in_display(EraseScreenMode::Below),
        Some(DamageSpan::rows(ViewportLine(1), ViewportLine(2)))
    );
}
