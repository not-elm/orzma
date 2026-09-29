//! Tests for finding the URL a viewport cell shows.

use super::*;
use crate::hyperlink::HyperlinkId;
use crate::screen::cell::CellWidth;

/// The cells of a viewport row showing `text` from column zero, padded
/// with blanks to `cols`; a width-2 character takes its body and the
/// continuation column after it.
fn row(text: &str, cols: usize) -> Vec<Cell> {
    let mut cells = Vec::new();
    for c in text.chars() {
        if GlyphClass::of(c) == Some(GlyphClass::Wide) {
            let body = Cell {
                c,
                width: CellWidth::Wide,
                ..Cell::default()
            };
            let continuation = body.continuation();
            cells.push(body);
            cells.push(continuation);
        } else {
            cells.push(Cell {
                c,
                ..Cell::default()
            });
        }
    }
    cells.resize(cols, Cell::default());
    cells
}

fn cell(row: u16, col: u16) -> ViewportCell {
    ViewportCell { row, col }
}

fn uri_at(
    rows: &[Vec<Cell>],
    wraps: &[Option<u16>],
    above: bool,
    at: ViewportCell,
) -> Option<String> {
    DetectedUrl::at(rows, wraps, above, at.row, at.col).map(|url| url.uri)
}

/// Asserts that a URL wrapped over two rows is found whole from a cell on
/// either row, with its first and last cells.
///
/// Case: `gh pr create` prints a pull request URL that wraps in a narrow
/// split pane.
#[test]
fn a_url_wrapped_over_rows_is_found_from_any_of_its_cells() {
    let rows = [row("https://", 8), row("a.b/c", 8)];
    let wraps = [Some(8), None];
    let expected = DetectedUrl {
        uri: "https://a.b/c".to_string(),
        first: cell(0, 0),
        last: cell(1, 4),
    };
    assert_eq!(
        DetectedUrl::at(&rows, &wraps, false, 1, 2),
        Some(expected.clone())
    );
    assert_eq!(DetectedUrl::at(&rows, &wraps, false, 0, 3), Some(expected));
}

/// Asserts that the cells past a row's wrap are left out of the line.
///
/// Case: a wide character that did not fit left a filler in the last
/// column of the row the URL starts on.
#[test]
fn cells_past_a_rows_wrap_are_skipped() {
    let rows = [row("go:http", 8), row("s://a.b", 8)];
    let found =
        DetectedUrl::at(&rows, &[Some(7), None], false, 1, 2).expect("the URL spans both rows");
    assert_eq!(found.uri, "https://a.b");
    assert_eq!((found.first, found.last), (cell(0, 3), cell(1, 6)));
}

/// Asserts that a wide character before a URL keeps its cells aligned,
/// and a wide character after one ends it.
///
/// Case: a Japanese log prefix precedes a URL that a Japanese word
/// follows directly.
#[test]
fn wide_characters_keep_cells_aligned_and_end_a_url() {
    let rows = [row("字 https://a.b字", 16)];
    let found = DetectedUrl::at(&rows, &[None], false, 0, 5).expect("the URL after the prefix");
    assert_eq!(found.uri, "https://a.b");
    assert_eq!((found.first, found.last), (cell(0, 3), cell(0, 13)));
}

/// Asserts that an OSC 8 cell reads as a blank, splitting the text around
/// it, and itself shows no detected URL.
///
/// Case: `ls --hyperlink` prints a linked name right after a plain URL.
#[test]
fn an_osc8_cell_splits_the_text() {
    let mut rows = [row("https://a.b/cd", 16)];
    for linked in &mut rows[0][12..14] {
        linked.hyperlink_id = HyperlinkId::new(3);
    }
    let found = DetectedUrl::at(&rows, &[None], false, 0, 0).expect("the plain part");
    assert_eq!(found.uri, "https://a.b/");
    assert_eq!(found.last, cell(0, 11));
    assert_eq!(DetectedUrl::at(&rows, &[None], false, 0, 12), None);
}

/// Asserts that cells that show no character of their own find nothing
/// and never fail: a wide glyph and its right half, a filler past a
/// row's wrap, and cells outside the grid.
///
/// Case: the user holds Cmd and sweeps the pointer across Japanese text,
/// the right edge of a wrapped row, and the margin below the last row.
#[test]
fn cells_that_show_no_character_of_their_own_find_nothing() {
    let rows = [row("字 https://a.b", 16)];
    assert_eq!(uri_at(&rows, &[None], false, cell(0, 0)), None);
    assert_eq!(uri_at(&rows, &[None], false, cell(0, 1)), None);
    assert_eq!(uri_at(&rows, &[None], false, cell(0, 40)), None);
    assert_eq!(uri_at(&rows, &[None], false, cell(3, 0)), None);
    let wrapped = [row("go:http", 8), row("s://a.b", 8)];
    assert_eq!(uri_at(&wrapped, &[Some(7), None], false, cell(0, 7)), None);
}

/// Asserts that plain words and blanks show no URL.
///
/// Case: the user holds Cmd over shell output that holds a URL further
/// along the row.
#[test]
fn words_and_blanks_show_no_url() {
    let rows = [row("hello https://a.b", 20)];
    assert_eq!(uri_at(&rows, &[None], false, cell(0, 1)), None);
    assert_eq!(uri_at(&rows, &[None], false, cell(0, 5)), None);
    assert_eq!(
        uri_at(&rows, &[None], false, cell(0, 7)),
        Some("https://a.b".to_string())
    );
}

/// Asserts that a line continuing from above the viewport links only a
/// match that starts after its first character that cannot be in a URL.
///
/// Case: the user scrolls back so that a Wayback Machine URL is cut by
/// the top of the pane, while a URL after a space on the same line stays
/// whole.
#[test]
fn a_line_continuing_from_above_links_only_after_its_first_stop() {
    let cut = [row("0101/https://example.com/", 32)];
    assert_eq!(uri_at(&cut, &[None], true, cell(0, 10)), None);
    assert_eq!(
        uri_at(&cut, &[None], false, cell(0, 10)),
        Some("https://example.com/".to_string())
    );
    let spaced = [row("ab https://example.com", 32)];
    assert_eq!(
        uri_at(&spaced, &[None], true, cell(0, 5)),
        Some("https://example.com".to_string())
    );
    let bare = [row("https://x.com", 16)];
    assert_eq!(uri_at(&bare, &[None], true, cell(0, 0)), None);
    let second_line = [row("plain", 8), row("https://x.com", 16)];
    assert_eq!(
        uri_at(&second_line, &[None, None], true, cell(1, 0)),
        Some("https://x.com".to_string())
    );
}

/// Asserts that a line continuing below the viewport drops a match whose
/// scan reached the last visible character, trimmed punctuation included.
///
/// Case: the user scrolls back so that a URL is cut by the bottom of the
/// pane, once right after a `.` that may begin `.html`.
#[test]
fn a_line_continuing_below_drops_a_url_reaching_its_end() {
    let cut = [row("see", 8), row("http://a", 8)];
    assert_eq!(uri_at(&cut, &[None, Some(8)], false, cell(1, 0)), None);
    let dotted = [row("http://a.", 9)];
    assert_eq!(uri_at(&dotted, &[Some(9)], false, cell(0, 0)), None);
    let ended = [row("http://a b", 10)];
    assert_eq!(
        uri_at(&ended, &[Some(10)], false, cell(0, 0)),
        Some("http://a".to_string())
    );
}

/// Asserts that rows without a wrap entry read as line ends.
///
/// Case: the renderer's cells are pre-sized before the first frame has
/// sent a wrap list.
#[test]
fn missing_wrap_entries_read_as_line_ends() {
    let rows = [row("https://", 8), row("a.b", 8)];
    assert_eq!(uri_at(&rows, &[], false, cell(1, 0)), None);
    assert_eq!(uri_at(&rows, &[], false, cell(0, 0)), None);
}

/// Asserts that a viewport holding one enormous logical line resolves a
/// URL inside a row and one crossing a row boundary.
///
/// Case: a program prints minified JSON full of URLs that wraps over the
/// whole 200x60 pane.
#[test]
fn a_viewport_long_logical_line_resolves_urls_across_rows() {
    let pattern = "ab https://example.com/path ";
    let text: String = pattern.chars().cycle().take(200 * 60).collect();
    let chars: Vec<char> = text.chars().collect();
    let rows: Vec<Vec<Cell>> = chars
        .chunks(200)
        .map(|chunk| row(&chunk.iter().collect::<String>(), 200))
        .collect();
    let mut wraps = vec![Some(200); 60];
    wraps[59] = None;
    assert_eq!(
        uri_at(&rows, &wraps, false, cell(30, 100)),
        Some("https://example.com/path".to_string())
    );
    let crossing =
        DetectedUrl::at(&rows, &wraps, false, 0, 199).expect("a URL starts in the last column");
    assert_eq!(crossing.uri, "https://example.com/path");
    assert_eq!((crossing.first, crossing.last), (cell(0, 199), cell(1, 22)));
}
