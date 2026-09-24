//! Tests for the text the active selection copies out.

use super::*;

/// Asserts that copying a fullwidth glyph yields the glyph alone, with no
/// blank for its continuation column.
///
/// Case: the user selects `あい` on a four-column row and copies it.
#[test]
fn copying_wide_glyphs_emits_no_blank_for_continuations() {
    let mut screen = screen();
    for c in ['あ', 'い'] {
        screen
            .print(classified(c), PrintOptions::default())
            .expect("a printable glyph");
    }
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 3), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("あい"));
}

/// Asserts that copying a partly selected fullwidth glyph copies it
/// whole.
///
/// Case: the user drags from the right half of one Japanese character
/// to the left half of the next and copies.
#[test]
fn copying_partly_selected_wide_glyphs_copies_them_whole() {
    let mut screen = screen();
    for c in ['あ', 'い'] {
        screen
            .print(classified(c), PrintOptions::default())
            .expect("a printable glyph");
    }
    screen.start_selection(point(0, 1), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 2), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("あい"));
}

/// Asserts that copying an accented letter keeps its combining mark.
///
/// Case: the user copies a word containing `e` followed by a combining
/// acute accent.
#[test]
fn copying_an_accented_letter_keeps_its_mark() {
    let mut screen = screen();
    for c in ['e', '\u{0301}', 'x'] {
        screen
            .print(classified(c), PrintOptions::default())
            .expect("a printable glyph");
    }
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 1), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("e\u{0301}x"));
}

/// Asserts that copying a fullwidth glyph carrying a mark keeps the
/// mark and adds no blank for the continuation column.
///
/// Case: the user copies a Japanese character followed by a combining
/// voiced sound mark.
#[test]
fn copying_a_wide_glyph_with_a_mark_keeps_the_mark() {
    let mut screen = screen();
    for c in ['か', '\u{3099}'] {
        screen
            .print(classified(c), PrintOptions::default())
            .expect("a printable glyph");
    }
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 1), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("か\u{3099}"));
}

/// Asserts that trailing blanks are still trimmed after marks are
/// emitted.
///
/// Case: the user selects a short accented word and the blank rest of
/// its row.
#[test]
fn trailing_blanks_are_trimmed_after_marks() {
    let mut screen = screen();
    for c in ['e', '\u{0301}'] {
        screen
            .print(classified(c), PrintOptions::default())
            .expect("a printable glyph");
    }
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(0, 3), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("e\u{0301}"));
}

/// Asserts that a selection across a soft wrap copies the line without a
/// newline at the wrap.
///
/// Case: the user copies a long command that autowrap carried onto a
/// second row.
#[test]
fn copying_across_a_soft_wrap_joins_the_rows() {
    let mut screen = screen();
    print_text(&mut screen, "abcdef");
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(1, 1), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("abcdef"));
}

/// Asserts that blanks at the end of a wrapped row are kept, since the
/// line continues after them.
///
/// Case: the user copies `ab  cd`, where the wrap fell between the two
/// spaces and the next word.
#[test]
fn copying_across_a_soft_wrap_keeps_the_blanks_before_it() {
    let mut screen = screen();
    print_text(&mut screen, "ab  cd");
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(1, 1), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("ab  cd"));
}

/// Asserts that a wrapped row's cells past its recorded length are left
/// out of the copy.
///
/// Case: the user copies a line whose first row a resize padded out past
/// the text that continues.
#[test]
fn copying_leaves_out_the_cells_past_the_recorded_wrap() {
    let mut screen = screen();
    seed_row(&mut screen, ScreenLine(0), &['a', 'b', 'x', 'x']);
    seed_row(&mut screen, ScreenLine(1), &['c', 'd']);
    screen.grid.set_wrap_at(GridLine(0), 2);
    screen.start_selection(point(0, 0), CellSide::Left, SelectionKind::Simple);
    screen.extend_selection(point(1, 1), CellSide::Right);
    assert_eq!(screen.selection_text().as_deref(), Some("abcd"));
}
