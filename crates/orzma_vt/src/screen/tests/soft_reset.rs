//! Tests for the screen-scoped half of the soft terminal reset.

use super::*;

/// Asserts that a soft reset returns the scrolling margins to the whole
/// page and the cursor origin to the upper-left corner, without seating
/// the cursor at the resulting home.
///
/// Case: a full-screen program leaves a scrolling region and origin
/// mode behind, and the shell resets the terminal at a prompt part-way
/// down the screen.
#[test]
fn a_soft_reset_returns_the_margins_and_the_origin_to_the_page() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.seat_cursor(ScreenLine(2), GridColumn(3));

    screen.soft_reset();

    assert_eq!(screen.scroll_region.top_margin(), ScreenLine(0));
    assert_eq!(screen.scroll_region.bottom_margin(), ScreenLine(3));
    assert_eq!(
        screen.scroll_region.origin_mode(),
        OriginMode::UpperLeftCorner
    );
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that a soft reset returns the SGR pen to its default.
///
/// Case: a program dies with a coloured pen set and the shell resets
/// the terminal before printing its next prompt.
#[test]
fn a_soft_reset_returns_the_pen_to_its_default() {
    let mut screen = dirty_screen();

    screen.soft_reset();

    assert_eq!(*screen.pen_mut(), Pen::default());
}

/// Asserts that a soft reset restores the power-up character set
/// mapping, including the locking shift and any pending single shift.
///
/// Case: a program designates the line-drawing set, locks it into GL to
/// draw a box, and exits without shifting back.
#[test]
fn a_soft_reset_restores_the_default_character_set_mapping() {
    let mut screen = dirty_screen();
    screen.single_shift(SingleShift::G2);

    screen.soft_reset();

    assert_eq!(screen.character_set_mapping, CharacterSetMapping::default());
}

/// Asserts that a soft reset returns the saved cursor to the home
/// position with a default pen.
///
/// Case: a program saves its cursor mid-screen with a coloured pen, and
/// the shell resets the terminal before anything restores it.
#[test]
fn a_soft_reset_returns_the_saved_cursor_to_its_default() {
    let mut screen = dirty_screen();
    screen.save_checkpoint();

    screen.soft_reset();

    assert_eq!(screen.checkpoint, Checkpoint::default());
}

/// Asserts that a soft reset leaves the cursor position and an armed
/// deferred wrap as they are.
///
/// Case: the shell fills a line to the right edge and the reset arrives
/// before the character that completes the wrap.
#[test]
fn a_soft_reset_leaves_the_cursor_and_its_deferred_wrap_alone() {
    let mut screen = dirty_screen();

    screen.soft_reset();

    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(3));
    assert!(screen.state.pending_wrap);
}

/// Asserts that a soft reset leaves the cells on screen as they are.
///
/// Case: the shell resets the terminal with command output the user
/// still wants to read.
#[test]
fn a_soft_reset_leaves_the_cells_alone() {
    let mut screen = screen();
    seed(&mut screen, &['a', 'b', 'c']);

    screen.soft_reset();

    assert_eq!(glyphs(&screen, 3), vec!['a', 'b', 'c']);
}

/// Asserts that a soft reset leaves the tabulation stops as they are.
///
/// Case: a program clears every tab stop to lay out a table and the
/// shell resets the terminal before the next tab.
#[test]
fn a_soft_reset_leaves_the_tab_stops_alone() {
    let mut screen = screen();
    screen.edit_tab_stop(CharacterTabEdit::ClearAllColumns);
    let mut cleared = TabStops::default();
    cleared.clear_all();

    screen.soft_reset();

    assert_eq!(screen.tabs, cleared);
}
