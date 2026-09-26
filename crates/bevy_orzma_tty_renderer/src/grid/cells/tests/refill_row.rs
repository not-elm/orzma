//! Tests for materializing one row's cells from its attribute runs.

use super::*;
use orzma_vt::prelude::{Color, MAX_COMBINING, Style};

fn refilled(runs: &[Run], cols: u16, table: &HashMap<HyperlinkId, HyperlinkUri>) -> Vec<Cell> {
    let mut out = Vec::new();
    refill_row(&mut out, runs, cols, table);
    out
}

/// Asserts that a run's hyperlink id resolves against the retained
/// table when cells are built, and that an id absent from the
/// table leaves the cell unlinked.
///
/// Case: a shell prints an OSC 8 link whose id → URI entry arrived
/// in an earlier frame's table.
#[test]
fn refill_row_resolves_hyperlink_ids_against_the_table() {
    let runs = vec![
        run_with_link("a", Some(id(7))),
        run_with_link("b", Some(id(9))),
    ];
    let table = link_table(7, "https://example");
    let slots = refilled(&runs, 2, &table);
    assert_eq!(slots[0].hyperlink_id, Some(id(7)));
    assert_eq!(text_of(&slots[1]), "b");
    assert_eq!(slots[1].hyperlink_id, None);
}

/// Asserts that a row materializes one cell per column, with a wide
/// glyph taking a wide cell and the continuation that follows it.
///
/// Case: a CJK character is printed at the start of a four-column
/// row.
#[test]
fn refill_row_indexes_cells_by_column() {
    let slots = refilled(&[run_with_widths("あz", &[2, 1])], 4, &HashMap::new());
    assert_eq!(slots.len(), 4);
    assert_eq!(text_of(&slots[0]), "あ");
    assert_eq!(slots[0].width, CellWidth::Wide);
    assert_eq!(slots[1], slots[0].continuation());
    assert_eq!(text_of(&slots[2]), "z");
    assert_eq!(slots[3], Cell::default());
}

/// Asserts that a wide glyph landing on the grid's last column is
/// stored as one narrow cell rather than a wide cell without its
/// continuation.
///
/// Case: a CJK character is the only character a single-column-wide
/// pane can hold.
#[test]
fn refill_row_stops_a_wide_glyph_at_the_last_column() {
    let slots = refilled(&[run_with_widths("あ", &[2])], 1, &HashMap::new());
    assert_eq!(slots.len(), 1);
    assert_eq!(text_of(&slots[0]), "あ");
    assert_eq!(slots[0].width, CellWidth::Narrow);
}

/// Asserts that a zero-width `char` joins the marks of the cell
/// before it instead of taking a column of its own.
///
/// Case: a program prints an accented latin word, so the accent
/// arrives right after the letter it modifies.
#[test]
fn refill_row_joins_a_zero_width_char_to_the_previous_cell() {
    let slots = refilled(
        &[run_with_widths("a\u{0301}b", &[1, 0, 1])],
        3,
        &HashMap::new(),
    );
    assert_eq!(text_of(&slots[0]), "a\u{0301}");
    assert_eq!(text_of(&slots[1]), "b");
    assert_eq!(slots[2], Cell::default());
}

/// Asserts that a run without widths yields one one-column cell per
/// `char`, whatever the characters are.
///
/// Case: a frame carries an ASCII row on the empty-width path.
#[test]
fn refill_row_treats_an_empty_width_list_as_one_column_each() {
    let slots = refilled(&[run_with_widths("ab", &[])], 2, &HashMap::new());
    assert_eq!(text_of(&slots[0]), "a");
    assert_eq!(text_of(&slots[1]), "b");
}

/// Asserts that a run's `char`s are never re-measured: a wide glyph
/// declared at width one takes one narrow cell.
///
/// Case: a frame declares a CJK glyph at width one on a row the VT
/// already laid out.
#[test]
fn refill_row_does_not_remeasure_the_text() {
    let slots = refilled(&[run_with_widths("あb", &[1, 1])], 2, &HashMap::new());
    assert_eq!(text_of(&slots[0]), "あ");
    assert_eq!(slots[0].width, CellWidth::Narrow);
    assert_eq!(text_of(&slots[1]), "b");
}

/// Asserts that refilling a cell that held combining marks leaves
/// none of those marks behind.
///
/// Case: an editor replaces an accented letter with a plain one in
/// the same column.
#[test]
fn refill_row_drops_the_marks_of_the_cell_it_reuses() {
    let mut out = Vec::new();
    let table = HashMap::new();
    refill_row(
        &mut out,
        &[run_with_widths("a\u{0301}", &[1, 0])],
        1,
        &table,
    );
    refill_row(&mut out, &[run_with_widths("b", &[1])], 1, &table);
    assert_eq!(text_of(&out[0]), "b");
}

/// Asserts that refilling a cell replaces its hyperlink along with
/// its glyph, so an unlinked run leaves no link on the cell it
/// reuses.
///
/// Case: a pager scrolls a linked filename out of a column that the
/// next frame fills with plain text.
#[test]
fn refill_row_drops_the_hyperlink_of_the_cell_it_reuses() {
    let mut out = Vec::new();
    let table = link_table(7, "https://example");
    refill_row(&mut out, &[run_with_link("a", Some(id(7)))], 1, &table);
    assert_eq!(out[0].hyperlink_id, Some(id(7)));

    refill_row(&mut out, &[run_with_link("a", None)], 1, &table);

    assert_eq!(out[0].hyperlink_id, None);
}

/// Asserts that a wide glyph and the narrow glyphs that replace it,
/// and the reverse, leave wide cells and their continuations where
/// the new row puts them.
///
/// Case: a file listing scrolls so that a row holding a CJK name now
/// holds an ASCII name, and the next scroll brings the CJK name back.
#[test]
fn refill_row_swaps_wide_and_narrow_cells() {
    let mut out = Vec::new();
    let table = HashMap::new();
    refill_row(&mut out, &[run_with_widths("あ", &[2])], 2, &table);
    assert_eq!(out[1].width, CellWidth::Spacer);

    refill_row(&mut out, &[run_with_widths("ab", &[1, 1])], 2, &table);
    assert_eq!(text_of(&out[0]), "a");
    assert_eq!(text_of(&out[1]), "b");

    refill_row(&mut out, &[run_with_widths("あ", &[2])], 2, &table);
    assert_eq!(text_of(&out[0]), "あ");
    assert_eq!(out[1].width, CellWidth::Spacer);
}

/// Asserts that refilling a row with runs that cover fewer columns
/// than before resets the uncovered tail to default cells.
///
/// Case: a producer that does not pad its rows with blanks repaints
/// a full row with a shorter line.
#[test]
fn refill_row_blanks_the_tail_a_shorter_row_leaves() {
    let mut out = Vec::new();
    let table = HashMap::new();
    refill_row(&mut out, &[run_with_widths("abc", &[1, 1, 1])], 3, &table);
    refill_row(&mut out, &[run_with_widths("x", &[1])], 3, &table);
    assert_eq!(text_of(&out[0]), "x");
    assert_eq!(out[1], Cell::default());
    assert_eq!(out[2], Cell::default());
}

/// Asserts that a zero-column refill empties the row, whatever it
/// held and whatever the runs carry.
///
/// Case: a malformed frame declares a size of zero columns for a
/// row that still holds the text an earlier frame painted.
#[test]
fn refill_row_empties_a_zero_column_row() {
    let mut out = Vec::new();
    let table = HashMap::new();
    refill_row(&mut out, &[run_with_widths("ab", &[1, 1])], 2, &table);
    refill_row(&mut out, &[run_with_widths("ab", &[1, 1])], 0, &table);
    assert!(out.is_empty());
}

/// Asserts that a cell keeps at most `MAX_COMBINING` of the marks
/// that follow its glyph and drops the rest.
///
/// Case: a frame stacks a hundred accents on one letter.
#[test]
fn refill_row_keeps_at_most_max_combining_marks() {
    let text = format!("a{}", "\u{0301}".repeat(100));
    let mut widths = vec![0u8; 101];
    widths[0] = 1;
    let slots = refilled(&[run_with_widths(&text, &widths)], 1, &HashMap::new());
    assert_eq!(slots[0].c, 'a');
    assert_eq!(slots[0].marks(), ['\u{0301}'; MAX_COMBINING]);
}

/// Asserts that the continuation of a wide glyph carries the glyph's
/// colors, style and hyperlink.
///
/// Case: a CJK filename is printed in bold red inside an OSC 8 link,
/// and the user hovers the right half of its first character.
#[test]
fn refill_row_carries_a_wide_glyphs_pen_and_link_into_its_continuation() {
    let run = Run {
        fg: Color::Indexed(1),
        style: Style::BOLD,
        hyperlink_id: Some(id(7)),
        ..run_with_widths("あ", &[2])
    };
    let slots = refilled(&[run], 2, &link_table(7, "https://example"));
    assert_eq!(slots[1], slots[0].continuation());
    assert_eq!(
        (slots[1].fg, slots[1].style, slots[1].hyperlink_id),
        (Color::Indexed(1), Style::BOLD, Some(id(7)))
    );
}

/// Asserts that a mark arriving right after the glyph that fills the
/// last column still joins that glyph, on every refill.
///
/// Case: an accented letter sits in the last column of a pane that
/// redraws the same row twice.
#[test]
fn refill_row_joins_a_mark_after_the_last_column_on_every_refill() {
    let mut out = Vec::new();
    let table = HashMap::new();
    let runs = [run_with_widths("a\u{0301}", &[1, 0])];
    refill_row(&mut out, &runs, 1, &table);
    refill_row(&mut out, &runs, 1, &table);
    assert_eq!(out.len(), 1);
    assert_eq!(text_of(&out[0]), "a\u{0301}");
}

/// Asserts that a mark following a wide glyph joins the glyph's cell
/// rather than its continuation, on every refill.
///
/// Case: a CJK character carrying a combining mark is redrawn in
/// place.
#[test]
fn refill_row_joins_a_mark_to_the_wide_cell_on_every_refill() {
    let mut out = Vec::new();
    let table = HashMap::new();
    let runs = [run_with_widths("あ\u{0301}", &[2, 0])];
    refill_row(&mut out, &runs, 2, &table);
    refill_row(&mut out, &runs, 2, &table);
    assert_eq!(text_of(&out[0]), "あ\u{0301}");
    assert_eq!(out[1], out[0].continuation());
}

/// Asserts that a mark following a glyph truncated past the last
/// column joins no cell.
///
/// Case: a frame carries a row wider than the pane, and the first
/// glyph that does not fit is accented.
#[test]
fn refill_row_drops_a_mark_whose_glyph_was_truncated() {
    let slots = refilled(
        &[run_with_widths("ab\u{0301}", &[1, 1, 0])],
        1,
        &HashMap::new(),
    );
    assert_eq!(slots.len(), 1);
    assert_eq!(text_of(&slots[0]), "a");
}
