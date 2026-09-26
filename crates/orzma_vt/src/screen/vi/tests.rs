//! Unit tests for the vi mode of [`Screen`].

use super::*;
use crate::screen::PrintOptions;
use crate::screen::cell::ClassifiedGlyph;
use crate::screen::character_sets::GraphicChar;
use crate::screen::grid::GridSize;
use crate::screen::selection::{CellSide, SelectionKind};
use crate::screen::viewport::DisplayOffset;

fn screen(cols: u16, rows: u16, max_history: usize) -> Screen {
    Screen::new(GridSize { cols, rows }, max_history)
}

fn point(line: i32, column: u16) -> GridPoint {
    GridPoint {
        line: GridLine(line),
        column: GridColumn(column),
    }
}

/// Prints `text` through the screen's own print path; a `'\n'` is a
/// carriage return followed by a line feed.
fn print_text(screen: &mut Screen, text: &str) {
    for c in text.chars() {
        if c == '\n' {
            screen.carriage_return();
            screen.line_feed();
        } else {
            let glyph =
                ClassifiedGlyph::classify(GraphicChar(c)).expect("a character with a width");
            screen
                .print(glyph, PrintOptions::default())
                .expect("a printable glyph");
        }
    }
}

fn vi_point(screen: &Screen) -> Option<GridPoint> {
    screen.vi_cursor().map(|cursor| cursor.point)
}

/// Asserts that entering vi mode seats the vi cursor on the write
/// cursor.
///
/// Case: the shell has printed a prompt and the user enters vi mode.
#[test]
fn entering_seats_the_vi_cursor_on_the_write_cursor() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "ab");
    assert!(screen.enter_vi_mode());
    assert_eq!(vi_point(&screen), Some(point(0, 2)));
}

/// Asserts that entering vi mode while the viewport is scrolled back
/// past the write cursor seats the vi cursor on the viewport's top-left
/// cell.
///
/// Case: the user has scrolled back through output with the wheel and
/// then enters vi mode.
#[test]
fn entering_while_scrolled_past_the_write_cursor_seats_it_top_left() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4\n5");
    let _ = screen.scroll(Scroll::Delta(2));
    assert!(screen.enter_vi_mode());
    assert_eq!(vi_point(&screen), Some(point(-2, 0)));
}

/// Asserts that entering vi mode twice reports no change the second
/// time and leaves the vi cursor where it was.
///
/// Case: the vi-mode shortcut is pressed while vi mode is already on.
#[test]
fn entering_twice_reports_no_change() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "ab");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 0));
    assert!(!screen.enter_vi_mode());
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}

/// Asserts that entering vi mode drops an existing selection.
///
/// Case: a mouse selection is still highlighted when the user enters vi
/// mode.
#[test]
fn entering_drops_the_selection() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "hello");
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 3), CellSide::Right);
    assert!(screen.enter_vi_mode());
    assert_eq!(screen.selection_range(), None);
}

/// Asserts that leaving vi mode drops the vi cursor and the selection and
/// returns a scrolled-back viewport to the live tail, owing a full
/// repaint.
///
/// Case: the user leaves vi mode after browsing scrollback with a
/// selection highlighted.
#[test]
fn exiting_returns_to_the_live_tail_and_drops_the_selection() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "1\n2\n3\n4\n5");
    assert!(screen.enter_vi_mode());
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Lines);
    let _ = screen.scroll(Scroll::Delta(2));
    assert_eq!(screen.exit_vi_mode(), ViewChange::Repainted);
    assert_eq!(screen.display_offset(), DisplayOffset(0));
    assert_eq!(screen.vi_cursor(), None);
    assert_eq!(screen.selection_range(), None);
}

/// Asserts that leaving vi mode outside vi mode changes nothing.
///
/// Case: a stale exit request arrives for a terminal that already left
/// vi mode.
#[test]
fn exiting_outside_vi_mode_changes_nothing() {
    let mut screen = screen(10, 3, 10);
    assert_eq!(screen.exit_vi_mode(), ViewChange::Unchanged);
}

/// Asserts that leaving vi mode at the live tail changes what a frame
/// carries without repainting any row.
///
/// Case: the user enters vi mode on the prompt and leaves it without
/// scrolling back.
#[test]
fn exiting_at_the_live_tail_repaints_nothing() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "ab");
    assert!(screen.enter_vi_mode());
    assert_eq!(screen.exit_vi_mode(), ViewChange::Carried);
}

/// Asserts that a change is classified by whether the viewport moved
/// first and by whether carried state changed second.
///
/// Case: a vi operation reports a viewport motion, a carried change, both,
/// or neither.
#[test]
fn a_view_change_is_classified_by_the_viewport_first() {
    assert_eq!(ViewChange::classify(false, None), ViewChange::Unchanged);
    assert_eq!(ViewChange::classify(true, None), ViewChange::Carried);
    assert_eq!(
        ViewChange::classify(false, Some(DamageSpan::Full)),
        ViewChange::Repainted
    );
    assert_eq!(
        ViewChange::classify(true, Some(DamageSpan::Full)),
        ViewChange::Repainted
    );
    assert_eq!(ViewChange::Repainted.damage(), Some(DamageSpan::Full));
    assert_eq!(ViewChange::Carried.damage(), None);
    assert!(ViewChange::Carried.is_changed());
    assert!(!ViewChange::Unchanged.is_changed());
}

/// Asserts that a vi cursor stored on the continuation column of a wide
/// glyph is reported on the glyph's body.
///
/// Case: output overwrites the cell under the vi cursor with a wide
/// glyph whose second column lands under it.
#[test]
fn a_vi_cursor_on_a_continuation_column_reports_the_glyph_body() {
    let mut screen = screen(10, 3, 10);
    print_text(&mut screen, "あ");
    assert!(screen.enter_vi_mode());
    screen.vi.set(point(0, 1));
    assert_eq!(vi_point(&screen), Some(point(0, 0)));
}
