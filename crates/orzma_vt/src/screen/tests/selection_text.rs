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
        screen.print(c, PrintOptions::default());
    }
    screen.start_selection(
        GridPoint {
            line: GridLine(0),
            column: GridColumn(0),
        },
        CellSide::Left,
        SelectionKind::Simple,
    );
    screen.extend_selection(
        GridPoint {
            line: GridLine(0),
            column: GridColumn(3),
        },
        CellSide::Right,
    );
    assert_eq!(screen.selection_text().as_deref(), Some("あい"));
}
