//! Tests for the reverse index and the region scrolling it triggers.

use super::*;

/// Asserts that a reverse index below the top margin moves the
/// cursor up one row without scrolling.
///
/// Case: a full-screen application walks its cursor back up the
/// screen a line at a time.
#[test]
fn a_reverse_index_below_the_top_moves_the_cursor_up() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    let damage = screen.reverse_index();
    assert_eq!(screen.state.line, ScreenLine(1));
    assert_eq!(damage, None);
}

/// Asserts that a reverse index at the top margin scrolls the
/// screen down instead of moving the cursor.
///
/// ECMA-48 § 6.1.7 leaves a movement past the first line undefined
/// and lists seven permitted behaviours; the agreed policy takes
/// (f), scrolling, rather than blocking the position or leaving
/// the cursor where it is.
///
/// Case: a pager scrolls backwards with its cursor already parked
/// on the first line of the screen.
#[test]
fn a_reverse_index_at_the_top_margin_scrolls_the_screen_down() {
    let mut screen = screen();
    screen.grid[ScreenLine(0)][0].c = 'a';
    let damage = screen.reverse_index();
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.grid[ScreenLine(1)][0].c, 'a');
    assert_eq!(damage, Some(DamageSpan::Full));
}

/// Asserts that a reverse index disarms the deferred wrap on both
/// the moving and the scrolling path.
///
/// The agreed policy follows xterm and VTE, whose reverse index
/// reaches its cursor-up helper on both paths and resets the
/// flag there. It is a deliberate divergence from kitty and
/// wezterm, which clear it only when the cursor moves, and
/// from alacritty, which clears it on neither — and
/// `Screen::line_feed` preserves the flag, so the split is
/// not accidental.
///
/// Case: a program fills the last column of a row and then emits a
/// reverse index instead of the newline the pending wrap was
/// waiting for.
#[test]
fn a_reverse_index_disarms_the_deferred_wrap_on_both_paths() {
    let mut moved = screen();
    moved.state.line = ScreenLine(1);
    moved.state.pending_wrap = true;
    moved.reverse_index();
    assert!(!moved.state.pending_wrap);

    let mut scrolled = screen();
    scrolled.state.pending_wrap = true;
    scrolled.reverse_index();
    assert!(!scrolled.state.pending_wrap);
}

/// Asserts that the row exposed at the top carries the pen's
/// background.
///
/// Case: an application paints a coloured panel and scrolls it
/// backwards, expecting the newly exposed row to match rather than
/// show the terminal default.
#[test]
fn the_exposed_row_carries_the_pen_background() {
    let mut screen = screen();
    screen.pen_mut().bg = Color::Indexed(4);
    screen.reverse_index();
    assert_eq!(screen.grid[ScreenLine(0)][0].bg, Color::Indexed(4));
}

/// Asserts that a reverse index leaves a scrolled-back viewport
/// showing what it was showing.
///
/// The agreed policy leaves the display offset alone rather than
/// adjusting it the way `Screen::line_feed` does. A forward scroll grows
/// history, so holding the view still requires moving the offset;
/// a reverse scroll leaves history untouched, so moving the offset
/// would push the viewport onto different history instead.
///
/// Case: the user has scrolled back to read earlier output while a
/// full-screen application keeps scrolling its own view backwards.
#[test]
fn a_reverse_index_leaves_a_scrolled_viewport_where_it_is() {
    let mut screen = screen();
    for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c')] {
        screen.grid[ScreenLine(line)][0].c = glyph;
    }
    screen.state.line = ScreenLine(2);
    screen.line_feed();
    screen.line_feed();
    screen.set_display_offset(DisplayOffset(1));
    let showing = screen.viewport_row(ViewportLine(0))[0].c;
    screen.state.line = ScreenLine(0);
    screen.reverse_index();
    assert_eq!(screen.display_offset(), DisplayOffset(1));
    assert_eq!(screen.viewport_row(ViewportLine(0))[0].c, showing);
}

/// Asserts that a reverse index with the cursor above a non-zero
/// top margin, already on the first row, moves and scrolls nothing.
///
/// The agreed policy follows DEC STD 070 and xterm: a cursor that
/// hits the screen edge outside the scrolling region stays put,
/// rather than scrolling the region it is not inside.
///
/// Case: an application sets a scroll region below a status line
/// and emits a reverse index while the cursor sits on that status
/// line.
#[test]
fn a_reverse_index_above_a_top_margin_at_row_zero_does_nothing() {
    let mut screen = screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(2),
    });
    screen.grid[ScreenLine(0)][0].c = 'a';
    let damage = screen.reverse_index();
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(damage, None);
}

/// Asserts that a cursor above a non-zero top margin still walks
/// up toward the first row.
///
/// The agreed policy bounds this movement by the screen edge
/// rather than by the margin: the region gates the scroll alone,
/// so a cursor outside it moves like an ordinary cursor-up
/// instead of being pinned at the margin.
///
/// Case: an application sets a scroll region below a two-line
/// header and emits a reverse index while the cursor sits on the
/// header's second line.
#[test]
fn a_reverse_index_above_a_top_margin_walks_toward_the_first_row() {
    let mut screen = screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(2),
        bottom: ScreenLine(2),
    });
    screen.state.line = ScreenLine(1);
    let damage = screen.reverse_index();
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(damage, None);
}

/// Asserts that a reverse index at a non-zero top margin scrolls
/// the region and leaves the rows above it alone.
///
/// Case: an application keeps a status line on the first row and
/// scrolls the pane below it backwards.
#[test]
fn a_reverse_index_at_a_top_margin_scrolls_only_the_region() {
    let mut screen = screen();
    screen.scroll_region.set_margins(Margins {
        top: ScreenLine(1),
        bottom: ScreenLine(2),
    });
    for (line, glyph) in [(0u16, 'a'), (1, 'b'), (2, 'c')] {
        screen.grid[ScreenLine(line)][0].c = glyph;
    }
    screen.state.line = ScreenLine(1);
    let blank = screen.state.pen.erase_cell().c;
    let damage = screen.reverse_index();
    assert_eq!(screen.grid[ScreenLine(0)][0].c, 'a');
    assert_eq!(screen.grid[ScreenLine(1)][0].c, blank);
    assert_eq!(screen.grid[ScreenLine(2)][0].c, 'b');
    assert_eq!(damage, Some(DamageSpan::Full));
}
