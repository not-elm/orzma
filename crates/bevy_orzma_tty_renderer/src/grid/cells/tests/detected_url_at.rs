//! Tests for resolving a visible cell to the URL its plain text shows.

use super::*;

/// Narrow cells showing `text`, padded with blanks to `cols`.
fn text_row(text: &str, cols: usize) -> Vec<Cell> {
    let mut row: Vec<Cell> = text
        .chars()
        .map(|c| Cell {
            c,
            ..Cell::default()
        })
        .collect();
    row.resize(cols, Cell::default());
    row
}

/// Asserts that a URL wrapped over two rows resolves from its second row
/// through the retained wrap list.
///
/// Case: `gh pr create` prints a pull request URL that wraps in a narrow
/// split pane, and the user points at its second half.
#[test]
fn a_wrapped_url_resolves_through_the_retained_wraps() {
    let cells = TerminalCells {
        cells: vec![text_row("https://", 8), text_row("a.b/c", 8)],
        wraps: vec![Some(8), None],
        ..TerminalCells::default()
    };
    let url = cells
        .detected_url_at(ViewportCell { row: 1, col: 2 })
        .expect("the second row shows the URL's tail");
    assert_eq!(url.uri, "https://a.b/c");
}

/// Asserts that a URL starting a top row that continues from above the
/// viewport does not resolve, while the same text on a fresh line does.
///
/// Case: the user scrolls back so that a long wrapped URL is cut by the
/// top of the pane.
#[test]
fn a_url_cut_by_the_top_of_the_viewport_does_not_resolve() {
    let cut = TerminalCells {
        cells: vec![text_row("0101/https://example.com/", 32)],
        wraps: vec![None],
        continues_from_above: true,
        ..TerminalCells::default()
    };
    assert_eq!(cut.detected_url_at(ViewportCell { row: 0, col: 10 }), None);
    let whole = TerminalCells {
        continues_from_above: false,
        ..cut
    };
    assert_eq!(
        whole
            .detected_url_at(ViewportCell { row: 0, col: 10 })
            .map(|url| url.uri),
        Some("https://example.com/".to_string())
    );
}
