//! Plain-text URL lookup: the URL a viewport cell shows, found across
//! soft wraps and mapped back to the cells showing it.

use crate::hyperlink::detected_url::url_match::{UrlMatch, is_url_body};
use crate::screen::cell::Cell;
use crate::screen::grid::coords::GridColumn;
use crate::screen::viewport::{ViewportLine, ViewportPoint};

mod url_match;

/// A URL shown in the viewport's plain text, with the cells that show its
/// first and last characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedUrl {
    /// The URL as displayed.
    pub uri: String,
    /// The cell showing the URL's first character.
    pub first: ViewportPoint,
    /// The cell showing the URL's last character.
    pub last: ViewportPoint,
}

impl DetectedUrl {
    /// The URL shown at the viewport cell `at` of `rows`, if any.
    ///
    /// `rows` holds the viewport's cells row by row. `wraps[r]` is how many
    /// leading cells of row `r` continue on row `r + 1`, and a missing
    /// entry reads as `None`. `continues_from_above` says whether row 0
    /// continues a line from above the viewport. The URL is looked up in
    /// the cell's logical line, which joins its rows through their wraps.
    /// That line skips continuation columns and the cells past a row's
    /// wrap, and reads a cell carrying an OSC 8 hyperlink as a blank.
    ///
    /// A URL starts with `http://`, `https://`, `ftp://` or `mailto:`,
    /// matched ASCII case-insensitively, at the start of the line or after
    /// a character that is not an ASCII letter or digit. Its body runs over
    /// printable ASCII other than ``<>"`{}|\^``, keeping balanced `()` and
    /// `[]`, and ends before an unmatched `)` or `]`; trailing `.,:;!?'([`
    /// are then trimmed. A non-ASCII character right after the body ends
    /// the URL there when it is whitespace, a control, a width-2 character,
    /// or one of `’”»›–—`, and voids the URL otherwise. A scheme with
    /// nothing left after it is not a URL, and a scheme inside a URL does
    /// not start another one.
    ///
    /// Returns `None` when the cell shows no URL, and when the URL may run
    /// past the viewport: on a line continuing from above, a URL that
    /// starts before the line's first visible character that cannot be in
    /// a URL body; on a line continuing below, a URL whose body, before
    /// trimming, runs to the line's last visible character.
    pub fn at(
        rows: &[Vec<Cell>],
        wraps: &[Option<u16>],
        continues_from_above: bool,
        at: ViewportPoint,
    ) -> Option<Self> {
        let cell = rows
            .get(usize::from(at.line.0))?
            .get(usize::from(at.column.0))?;
        if cell.hyperlink_id.is_some() || !cell.chars().next().is_some_and(is_url_body) {
            return None;
        }
        LogicalLine::around(rows, wraps, continues_from_above, at.line)?.url_at(at)
    }
}

/// One soft-wrapped logical line of viewport rows as text, with the cell
/// each byte of the text came from.
struct LogicalLine {
    text: String,
    /// The cell each byte of `text` came from, one entry per byte.
    origins: Vec<ViewportPoint>,
    /// Whether the line continues above the first row it holds.
    cut_above: bool,
    /// Whether the line continues below the last row it holds.
    cut_below: bool,
}

impl LogicalLine {
    /// The logical line holding `line`; `None` when `line` is outside
    /// `rows`.
    fn around(
        rows: &[Vec<Cell>],
        wraps: &[Option<u16>],
        continues_from_above: bool,
        line: ViewportLine,
    ) -> Option<Self> {
        let wrap = |index: usize| wraps.get(index).copied().flatten();
        let row = usize::from(line.0);
        if row >= rows.len() {
            return None;
        }
        let mut first = row;
        while first > 0 && wrap(first - 1).is_some() {
            first -= 1;
        }
        let mut last = row;
        while last + 1 < rows.len() && wrap(last).is_some() {
            last += 1;
        }
        let held = rows.get(first..=last).unwrap_or_default();
        let capacity = held.iter().map(Vec::len).sum();
        let mut line = Self {
            text: String::with_capacity(capacity),
            origins: Vec::with_capacity(capacity),
            cut_above: first == 0 && continues_from_above,
            cut_below: wrap(last).is_some(),
        };
        for (index, cells) in (first..).zip(held) {
            let limit =
                wrap(index).map_or(cells.len(), |count| usize::from(count).min(cells.len()));
            for (col, cell) in cells.iter().enumerate().take(limit) {
                let at = ViewportPoint {
                    line: ViewportLine(index as u16),
                    column: GridColumn(col as u16),
                };
                line.push_cell(cell, at);
            }
        }
        Some(line)
    }

    /// The URL covering the cell `at`, unless an edge rule drops it.
    fn url_at(&self, at: ViewportPoint) -> Option<DetectedUrl> {
        let target = self.origins.iter().position(|origin| *origin == at)?;
        let found = UrlMatch::scan(&self.text)
            .take_while(|found| found.url.start <= target)
            .find(|found| found.url.contains(&target))?;
        if !self.is_wholly_visible(&found) {
            return None;
        }
        Some(DetectedUrl {
            uri: self.text.get(found.url.clone())?.to_owned(),
            first: *self.origins.get(found.url.start)?,
            last: *self.origins.get(found.url.end.checked_sub(1)?)?,
        })
    }

    /// Whether `found` cannot run past either edge of the viewport.
    fn is_wholly_visible(&self, found: &UrlMatch) -> bool {
        let may_start_above = self.cut_above
            && self
                .text
                .find(|c: char| !is_url_body(c))
                .is_none_or(|stop| found.url.start < stop);
        let may_end_below = self.cut_below && found.scan_end == self.text.len();
        !may_start_above && !may_end_below
    }

    /// Appends the characters `cell` shows, or a blank for an OSC 8 cell.
    fn push_cell(&mut self, cell: &Cell, at: ViewportPoint) {
        if cell.hyperlink_id.is_some() {
            self.push(' ', at);
            return;
        }
        for c in cell.chars() {
            self.push(c, at);
        }
    }

    fn push(&mut self, c: char, at: ViewportPoint) {
        self.text.push(c);
        self.origins.resize(self.text.len(), at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyperlink::HyperlinkId;
    use crate::screen::cell::{CellWidth, GlyphClass};

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

    fn point(line: u16, column: u16) -> ViewportPoint {
        ViewportPoint {
            line: ViewportLine(line),
            column: GridColumn(column),
        }
    }

    fn uri_at(
        rows: &[Vec<Cell>],
        wraps: &[Option<u16>],
        above: bool,
        at: ViewportPoint,
    ) -> Option<String> {
        DetectedUrl::at(rows, wraps, above, at).map(|url| url.uri)
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
            first: point(0, 0),
            last: point(1, 4),
        };
        assert_eq!(
            DetectedUrl::at(&rows, &wraps, false, point(1, 2)),
            Some(expected.clone())
        );
        assert_eq!(
            DetectedUrl::at(&rows, &wraps, false, point(0, 3)),
            Some(expected)
        );
    }

    /// Asserts that the cells past a row's wrap are left out of the line.
    ///
    /// Case: a wide character that did not fit left a filler in the last
    /// column of the row the URL starts on.
    #[test]
    fn cells_past_a_rows_wrap_are_skipped() {
        let rows = [row("go:http", 8), row("s://a.b", 8)];
        let found = DetectedUrl::at(&rows, &[Some(7), None], false, point(1, 2))
            .expect("the URL spans both rows");
        assert_eq!(found.uri, "https://a.b");
        assert_eq!((found.first, found.last), (point(0, 3), point(1, 6)));
    }

    /// Asserts that a wide character before a URL keeps its cells aligned,
    /// and a wide character after one ends it.
    ///
    /// Case: a Japanese log prefix precedes a URL that a Japanese word
    /// follows directly.
    #[test]
    fn wide_characters_keep_cells_aligned_and_end_a_url() {
        let rows = [row("字 https://a.b字", 16)];
        let found =
            DetectedUrl::at(&rows, &[None], false, point(0, 5)).expect("the URL after the prefix");
        assert_eq!(found.uri, "https://a.b");
        assert_eq!((found.first, found.last), (point(0, 3), point(0, 13)));
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
        let found = DetectedUrl::at(&rows, &[None], false, point(0, 0)).expect("the plain part");
        assert_eq!(found.uri, "https://a.b/");
        assert_eq!(found.last, point(0, 11));
        assert_eq!(DetectedUrl::at(&rows, &[None], false, point(0, 12)), None);
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
        assert_eq!(uri_at(&rows, &[None], false, point(0, 0)), None);
        assert_eq!(uri_at(&rows, &[None], false, point(0, 1)), None);
        assert_eq!(uri_at(&rows, &[None], false, point(0, 40)), None);
        assert_eq!(uri_at(&rows, &[None], false, point(3, 0)), None);
        let wrapped = [row("go:http", 8), row("s://a.b", 8)];
        assert_eq!(uri_at(&wrapped, &[Some(7), None], false, point(0, 7)), None);
    }

    /// Asserts that plain words and blanks show no URL.
    ///
    /// Case: the user holds Cmd over shell output that holds a URL further
    /// along the row.
    #[test]
    fn words_and_blanks_show_no_url() {
        let rows = [row("hello https://a.b", 20)];
        assert_eq!(uri_at(&rows, &[None], false, point(0, 1)), None);
        assert_eq!(uri_at(&rows, &[None], false, point(0, 5)), None);
        assert_eq!(
            uri_at(&rows, &[None], false, point(0, 7)),
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
        assert_eq!(uri_at(&cut, &[None], true, point(0, 10)), None);
        assert_eq!(
            uri_at(&cut, &[None], false, point(0, 10)),
            Some("https://example.com/".to_string())
        );
        let spaced = [row("ab https://example.com", 32)];
        assert_eq!(
            uri_at(&spaced, &[None], true, point(0, 5)),
            Some("https://example.com".to_string())
        );
        let bare = [row("https://x.com", 16)];
        assert_eq!(uri_at(&bare, &[None], true, point(0, 0)), None);
        let second_line = [row("plain", 8), row("https://x.com", 16)];
        assert_eq!(
            uri_at(&second_line, &[None, None], true, point(1, 0)),
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
        assert_eq!(uri_at(&cut, &[None, Some(8)], false, point(1, 0)), None);
        let dotted = [row("http://a.", 9)];
        assert_eq!(uri_at(&dotted, &[Some(9)], false, point(0, 0)), None);
        let ended = [row("http://a b", 10)];
        assert_eq!(
            uri_at(&ended, &[Some(10)], false, point(0, 0)),
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
        assert_eq!(uri_at(&rows, &[], false, point(1, 0)), None);
        assert_eq!(uri_at(&rows, &[], false, point(0, 0)), None);
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
            uri_at(&rows, &wraps, false, point(30, 100)),
            Some("https://example.com/path".to_string())
        );
        let crossing = DetectedUrl::at(&rows, &wraps, false, point(0, 199))
            .expect("a URL starts in the last column");
        assert_eq!(crossing.uri, "https://example.com/path");
        assert_eq!(
            (crossing.first, crossing.last),
            (point(0, 199), point(1, 22))
        );
    }
}
