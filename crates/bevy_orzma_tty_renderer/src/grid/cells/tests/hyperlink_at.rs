//! Tests for resolving the hyperlink at a visible cell.

use super::*;

/// Asserts that a lookup outside the populated grid returns `None`.
///
/// Case: the pointer hovers over the padding beyond the last row
/// or column while the grid is smaller than the window.
#[test]
fn hyperlink_at_returns_none_when_out_of_bounds() {
    let cells = TerminalCells {
        cells: vec![vec![], vec![]],
        ..Default::default()
    };
    assert!(cells.hyperlink_at(99, 0).is_none());
    assert!(cells.hyperlink_at(0, 99).is_none());
}

/// Asserts that a column no run covered resolves to no hyperlink.
///
/// Case: the pointer hovers a column of the row that no attribute
/// run painted this frame.
#[test]
fn hyperlink_at_returns_none_for_a_blank_cell() {
    let cells = TerminalCells {
        cells: vec![vec![Cell::default(); 4]],
        ..Default::default()
    };
    assert!(cells.hyperlink_at(0, 0).is_none());
}

/// Asserts that a linked cell resolves to its hyperlink id and URI.
///
/// Case: the user hovers an OSC 8 link a shell printed, and the
/// input layer asks which link sits under the pointer.
#[test]
fn hyperlink_at_returns_id_and_uri_for_linked_cell() {
    let cells = TerminalCells {
        cells: vec![vec![linked_cell('x', Some(7))]],
        hyperlinks: link_table(7, "https://example"),
        ..Default::default()
    };
    let (resolved, uri) = cells.hyperlink_at(0, 0).expect("hyperlink present");
    assert_eq!(resolved, id(7));
    assert_eq!(uri.as_str(), "https://example");
}

/// Asserts that an unlinked cell resolves to `None`.
///
/// Case: the user hovers plain shell output that carries no
/// hyperlink.
#[test]
fn hyperlink_at_returns_none_for_unlinked_cell() {
    let cells = TerminalCells {
        cells: vec![vec![linked_cell('x', None)]],
        ..Default::default()
    };
    assert!(cells.hyperlink_at(0, 0).is_none());
}

/// Asserts that both columns of a wide glyph resolve to the
/// same hyperlink while the cell after it stays unlinked.
///
/// Case: a CJK character inside an OSC 8 link spans two columns,
/// and the user may hover either half.
#[test]
fn hyperlink_at_resolves_both_halves_of_wide_char() {
    let wide_linked = Cell {
        width: CellWidth::Wide,
        ..linked_cell('あ', Some(7))
    };
    let continuation = wide_linked.continuation();
    let cells = TerminalCells {
        cells: vec![vec![wide_linked, continuation, linked_cell('b', None)]],
        hyperlinks: link_table(7, "https://example"),
        ..Default::default()
    };
    let (resolved, uri) = cells.hyperlink_at(0, 0).expect("left half should resolve");
    assert_eq!(resolved, id(7));
    assert_eq!(uri.as_str(), "https://example");
    let (resolved, uri) = cells.hyperlink_at(0, 1).expect("right half should resolve");
    assert_eq!(resolved, id(7));
    assert_eq!(uri.as_str(), "https://example");
    assert!(cells.hyperlink_at(0, 2).is_none());
}
