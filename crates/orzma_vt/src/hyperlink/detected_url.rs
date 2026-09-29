//! Plain-text URL lookup: the URL a viewport cell shows, found across
//! soft wraps and mapped back to the cells showing it.

use crate::hyperlink::url_match::{UrlMatch, is_url_body};
use crate::screen::cell::Cell;
use crate::screen::grid::coords::GridColumn;
use crate::screen::viewport::{ViewportLine, ViewportPoint};

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
    /// continues a line from above the viewport. A URL is recognized as
    /// [`UrlMatch::scan`] recognizes one in the cell's logical line, which
    /// joins its rows through their wraps. That line skips continuation
    /// columns and the cells past a row's wrap, and reads a cell carrying
    /// an OSC 8 hyperlink as a blank.
    ///
    /// Returns `None` when the cell shows no URL, and when the URL may run
    /// past the viewport: on a line continuing from above, a match that
    /// starts before the line's first visible character that cannot be
    /// in a URL body; on a line continuing below, a match whose scan
    /// reached the line's last visible character.
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
mod tests;
