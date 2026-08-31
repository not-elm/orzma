//! Tests for whole-screen state replacement.

use super::*;

/// Asserts that a reset blanks every visible cell and reports
/// the whole screen as damaged.
///
/// Case: a full-screen application exits and the shell sends
/// `RIS` to take the terminal back to a known state.
#[test]
fn a_reset_empties_every_visible_cell() {
    let mut screen = screen();
    for line in 0..3 {
        for column in 0..4 {
            screen.grid[ScreenLine(line)][column].c = 'x';
        }
    }
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    for line in 0..3 {
        let row = screen.viewport_row(ViewportLine(line));
        assert!(row.iter().all(|cell| *cell == Cell::default()));
    }
}

/// Asserts that a reset fills the grid with default cells rather
/// than carrying the pen background into them the way an erase
/// does.
///
/// Case: an application paints a red-backgrounded banner and the
/// shell resets the terminal without the application restoring
/// SGR first.
#[test]
fn a_reset_leaves_no_trace_of_the_pen_background_in_the_cells() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(1);
    screen.print('x');
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    assert_eq!(screen.viewport_row(ViewportLine(0))[0], Cell::default());
}

/// Asserts that a reset seats the cursor at the upper-left
/// corner of the screen.
///
/// Case: a shell sends `RIS` while its cursor sits mid-screen
/// after a half-drawn prompt.
#[test]
fn a_reset_homes_the_cursor() {
    let mut screen = screen();
    screen.grid[ScreenLine(0)][0].c = 'x';
    screen.move_cursor_to(Some(2), Some(3));
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that a reset discards the scrollback history and
/// reseats the viewport on the live tail.
///
/// Case: the user has an earlier command's output scrolled back
/// into view when the next one sends `RIS`.
#[test]
fn a_reset_drops_the_scrollback_history() {
    let mut screen = screen();
    screen.grid[ScreenLine(0)][0].c = 'a';
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.grid[ScreenLine(0)][0].c = 'b';
    screen.viewport.offset = DisplayOffset(1);
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    assert_eq!(screen.grid.history_len(), 0);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
}

/// Asserts that a reset returns the SGR pen to normal rendition.
///
/// Case: the shell resets a terminal an application left with a
/// red background selected, then prints its own prompt.
#[test]
fn a_reset_returns_the_pen_to_normal_rendition() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(1);
    screen.print('x');
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    assert_eq!(*screen.pen_mut(), Pen::default());
}

/// Asserts that a reset blanks the rows outside the scrolling
/// region as well as the ones inside it.
///
/// Case: an application reserves a status line outside its
/// scrolling region and is then reset.
#[test]
fn a_reset_empties_rows_outside_the_scrolling_region_as_well() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    for line in 0..4 {
        screen.grid[ScreenLine(line)][0].c = 'x';
    }
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    assert_eq!(screen.viewport_row(ViewportLine(0))[0], Cell::default());
    assert_eq!(screen.viewport_row(ViewportLine(3))[0], Cell::default());
}

/// Asserts that a reset restores the full-page margins and the
/// absolute origin, and homes to the screen's corner rather than
/// to the margin the old origin mode defined.
///
/// Case: a full-screen editor with a reserved status line and
/// origin mode on is reset by the shell that outlives it.
#[test]
fn a_reset_restores_the_margins_and_homes_to_the_screen_corner() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(2), Some(3));
    screen.set_origin_mode(OriginMode::WithinMargins);
    screen.grid[ScreenLine(0)][0].c = 'x';
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    assert_eq!(screen.scroll_region.top_margin(), ScreenLine(0));
    assert_eq!(screen.scroll_region.bottom_margin(), ScreenLine(3));
    assert_eq!(
        screen.scroll_region.origin_mode(),
        OriginMode::UpperLeftCorner
    );
    assert_eq!(screen.state.line, ScreenLine(0));
}

/// Asserts that a reset returns every G code and the GL
/// invocation to their defaults.
///
/// Case: a curses application locks the DEC line-drawing set
/// into GL and dies without restoring ASCII.
#[test]
fn a_reset_returns_the_character_sets_to_their_defaults() {
    let mut screen = screen();
    screen.designate_character_set(GCode::G1, CharacterSet::DecSpecialGraphics);
    screen.invoke_character_set(GCode::G1);
    screen.grid[ScreenLine(0)][0].c = 'x';
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    assert_eq!(screen.character_set_mapping, CharacterSetMapping::default());
}

/// Asserts that a reset reinstalls the default eight-column
/// tabulation stride.
///
/// Case: an application clears every stop with `TBC 3`, installs
/// one of its own, and is reset before it restores the defaults.
#[test]
fn a_reset_reinstalls_the_default_tabulation_stride() {
    let mut screen = wide_screen();
    screen.edit_tab_stop(CharacterTabEdit::ClearAllColumns);
    screen.state.column = GridColumn(3);
    screen.set_horizontal_tab_stop();
    screen.grid[ScreenLine(0)][0].c = 'x';
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(8));
}

/// Asserts that a reset of an already-blank screen with no
/// history reports no damage, and homes the cursor anyway.
///
/// Case: the user sends `RIS` twice in a row at a fresh prompt.
#[test]
fn a_reset_of_an_already_blank_screen_reports_no_damage() {
    let mut screen = screen();
    screen.move_cursor_to(Some(2), Some(3));
    assert_eq!(screen.reset(), None);
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(0));
}

/// Asserts that one dirty cell is enough to report the whole
/// screen as damaged.
///
/// Case: a background job prints a single character into the
/// corner of an otherwise untouched screen before the reset.
#[test]
fn a_single_dirty_cell_still_reports_the_whole_screen() {
    let mut screen = screen();
    screen.grid[ScreenLine(2)][3].c = 'x';
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
}

/// Asserts that history alone makes a reset report damage, with
/// every visible cell already blank.
///
/// Case: a long build scrolls its output away and leaves a blank
/// screen, and the user resets to reclaim the scrollback.
#[test]
fn a_blank_screen_with_history_still_reports_the_whole_screen() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    assert_eq!(screen.grid.history_len(), 1);
    assert_eq!(screen.reset(), Some(DamageSpan::Full));
}
