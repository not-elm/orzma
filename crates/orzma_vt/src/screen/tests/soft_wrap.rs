//! Tests for the soft-wrap record autowrap leaves on the rows it
//! continues.

use super::*;

fn wrap_of(screen: &Screen, line: i32) -> Option<u16> {
    screen.grid.wrap_at(GridLine(line))
}

/// Asserts that text running past the last column records a wrap
/// covering the whole row it left.
///
/// Case: a shell echoes a command longer than the window is wide.
#[test]
fn autowrap_records_the_wrap_on_the_row_it_left() {
    let mut screen = screen();
    print_text(&mut screen, "abcdef");
    assert_eq!(wrap_of(&screen, 0), Some(4));
    assert_eq!(wrap_of(&screen, 1), None);
}

/// Asserts that a wrap on the bottom row, which scrolls, records the wrap
/// on the row that moved up.
///
/// Case: output wraps while the prompt sits on the last row of the
/// screen.
#[test]
fn autowrap_at_the_bottom_records_the_wrap_on_the_scrolled_row() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    print_text(&mut screen, "abcdef");
    assert_eq!(wrap_of(&screen, 1), Some(4));
    assert_eq!(row_text(&screen, 1), "abcd");
}

/// Asserts that a wide glyph pushed off the last column records a wrap
/// that stops short of the filler.
///
/// Case: Japanese text reaches the right edge with one column left.
#[test]
fn a_wide_glyph_wrap_stops_short_of_the_filler() {
    let mut screen = screen();
    print_text(&mut screen, "abcあ");
    assert_eq!(wrap_of(&screen, 0), Some(3));
}

/// Asserts that a glyph filling the last column arms the deferred wrap
/// without recording a wrap.
///
/// Case: a prompt ends exactly at the right edge and nothing has been
/// typed yet.
#[test]
fn an_unresolved_deferred_wrap_records_nothing() {
    let mut screen = screen();
    print_text(&mut screen, "abcd");
    assert!(screen.state.pending_wrap);
    assert_eq!(wrap_of(&screen, 0), None);
}

/// Asserts that with autowrap reset a glyph past the edge records
/// nothing.
///
/// Case: a program turns `DECAWM` off and prints a line longer than the
/// window.
#[test]
fn a_print_with_autowrap_reset_records_nothing() {
    let mut screen = screen();
    let options = PrintOptions {
        auto_wrap: AutoWrap::Disabled,
        ..PrintOptions::default()
    };
    for c in "abcdef".chars() {
        screen
            .print(classified(c), options)
            .expect("a printable glyph");
    }
    assert_eq!(wrap_of(&screen, 0), None);
}

/// Asserts that a glyph landing in the last column ends the row's line
/// until the deferred wrap resolves again.
///
/// Case: readline redraws the first row of a wrapped command up to the
/// edge, then prints the next character.
#[test]
fn a_glyph_in_the_last_column_ends_the_line_until_the_wrap_resolves() {
    let mut screen = screen();
    print_text(&mut screen, "abcdef");
    screen.move_cursor_to(Some(1), Some(4));
    print_text(&mut screen, "x");
    assert_eq!(wrap_of(&screen, 0), None);
    print_text(&mut screen, "y");
    assert_eq!(wrap_of(&screen, 0), Some(4));
}

/// Asserts that inserting characters inside a wrap that stops short of
/// the last column moves the wrap right with the text.
///
/// Case: the user inserts a character into a Japanese command line that
/// wrapped at a wide glyph.
#[test]
fn inserting_inside_a_short_wrap_moves_it_with_the_text() {
    let mut screen = screen();
    print_text(&mut screen, "abcあ");
    screen.move_cursor_to(Some(1), Some(2));
    let _ = screen.insert_characters(1);
    assert_eq!(wrap_of(&screen, 0), Some(4));
}

/// Asserts that deleting characters inside a wrap that stops short of
/// the last column moves the wrap left with the text.
///
/// Case: the user deletes a character from a Japanese command line that
/// wrapped at a wide glyph.
#[test]
fn deleting_inside_a_short_wrap_moves_it_with_the_text() {
    let mut screen = screen();
    print_text(&mut screen, "abcあ");
    screen.move_cursor_to(Some(1), Some(2));
    let _ = screen.delete_characters(1);
    assert_eq!(wrap_of(&screen, 0), Some(2));
}

/// Asserts that a glyph printed past a wrap that stops short of the last
/// column extends the wrap to cover it.
///
/// Case: a line editor redraws a command whose first row a deletion
/// shortened, printing past where the shortened text ended.
#[test]
fn a_glyph_printed_past_a_short_wrap_extends_it() {
    let mut screen = screen();
    print_text(&mut screen, "abcあ");
    screen.move_cursor_to(Some(1), Some(2));
    let _ = screen.delete_characters(1);
    screen.move_cursor_to(Some(1), Some(3));
    print_text(&mut screen, "x");
    assert_eq!(wrap_of(&screen, 0), Some(3));
}

/// Asserts that a wrap whose line feed cannot leave the row records
/// nothing.
///
/// Case: the cursor sits below the bottom margin on the last row when
/// autowrap resolves.
#[test]
fn a_wrap_that_stays_on_its_row_records_nothing() {
    let mut screen = tall_screen();
    screen.set_scroll_region(Some(1), Some(2));
    screen.move_cursor_to(Some(4), Some(4));
    print_text(&mut screen, "ab");
    assert_eq!(wrap_of(&screen, 3), None);
    assert_eq!(wrap_of(&screen, 2), None);
}

/// Asserts that a deferred wrap resolved on a row that ends in a filler
/// records a wrap that stops short of the filler.
///
/// Case: a program restores a cursor it saved at the right edge onto a row
/// where Japanese text has since wrapped, then prints.
#[test]
fn a_deferred_wrap_over_a_filler_stops_short_of_it() {
    let mut screen = screen();
    print_text(&mut screen, "abcd");
    screen.save_checkpoint();
    screen.move_cursor_to(Some(1), Some(1));
    print_text(&mut screen, "xyz\u{3042}");
    assert_eq!(wrap_of(&screen, 0), Some(3));
    screen.restore_checkpoint();
    print_text(&mut screen, "q");
    assert_eq!(wrap_of(&screen, 0), Some(3));
}
