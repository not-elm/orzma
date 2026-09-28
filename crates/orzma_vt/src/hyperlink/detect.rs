//! Plain-text URL detection: the scanner over text, and the lookup that
//! maps a match back to the viewport cells showing it.

use crate::screen::cell::Cell;
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

/// A viewport cell: a zero-based row counted from the top of the
/// viewport and a zero-based column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportCell {
    /// The row, counted from the top of the viewport.
    pub row: u16,
    /// The column.
    pub col: u16,
}

/// A URL shown in the viewport's plain text, with the cells that show its
/// first and last characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedUrl {
    /// The URL as displayed.
    pub uri: String,
    /// The cell showing the URL's first character.
    pub first: ViewportCell,
    /// The cell showing the URL's last character.
    pub last: ViewportCell,
}

impl DetectedUrl {
    /// The URL shown at the viewport cell (`row`, `col`) of `rows`, if any.
    ///
    /// `rows` holds the viewport's cells row by row. `wraps[r]` is how many
    /// leading cells of row `r` continue on row `r + 1`, and a missing
    /// entry reads as `None`. `continues_from_above` says whether row 0
    /// continues a line from above the viewport. The URL is found by
    /// [`UrlMatch::scan`] over the cell's logical line: its rows joined
    /// through their wraps, the cells past a row's wrap and continuation
    /// columns left out, and a cell carrying an OSC 8 hyperlink read as a
    /// blank.
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
        row: u16,
        col: u16,
    ) -> Option<Self> {
        let cell = rows.get(usize::from(row))?.get(usize::from(col))?;
        if cell.hyperlink_id.is_some() || !cell.chars().next().is_some_and(is_url_body) {
            return None;
        }
        LogicalLine::around(rows, wraps, continues_from_above, row)?
            .url_at(ViewportCell { row, col })
    }
}

/// One URL found in plain text, as byte ranges into that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlMatch {
    /// The URL, with trailing punctuation trimmed.
    pub url: Range<usize>,
    /// The span the scan covered before trimming. It starts where `url`
    /// starts and ends at or after `url`'s end.
    pub raw: Range<usize>,
}

impl UrlMatch {
    /// Every URL in `text`, leftmost first and never overlapping.
    ///
    /// A URL starts with `http://`, `https://`, `ftp://` or `mailto:`,
    /// matched ASCII case-insensitively, at the start of `text` or after a
    /// byte that is not an ASCII letter or digit. Its body runs over
    /// printable ASCII other than ``<>"`{}|\^``, keeping balanced `()` and
    /// `[]`, and ends before an unmatched `)` or `]`; trailing
    /// `.,:;!?'([` are then trimmed. A non-ASCII character right after the
    /// body ends the URL there when it is whitespace, a control, a width-2
    /// character, or one of `’”»›–—`, and voids the URL otherwise. A URL
    /// with nothing left after its scheme is not reported. Scanning
    /// resumes at each match's `raw` end, so a scheme inside a URL never
    /// starts a second match.
    pub fn scan(text: &str) -> Vec<Self> {
        let bytes = text.as_bytes();
        let mut found = Vec::new();
        let mut from = 0;
        while let Some((start, body)) = next_scheme(bytes, from) {
            let (end, voided) = body_end(text, body);
            let url_end = trimmed_end(bytes, body, end);
            if !voided && url_end > body {
                found.push(Self {
                    url: start..url_end,
                    raw: start..end,
                });
            }
            from = end;
        }
        found
    }
}

/// One soft-wrapped logical line of viewport rows as text, with the cell
/// each byte of the text came from.
struct LogicalLine {
    text: String,
    /// The cell each byte of `text` came from, one entry per byte.
    origins: Vec<ViewportCell>,
    /// Whether the line continues above the first row it holds.
    cut_above: bool,
    /// Whether the line continues below the last row it holds.
    cut_below: bool,
}

impl LogicalLine {
    /// The logical line holding viewport row `row`; `None` when `row` is
    /// outside `rows`.
    fn around(
        rows: &[Vec<Cell>],
        wraps: &[Option<u16>],
        continues_from_above: bool,
        row: u16,
    ) -> Option<Self> {
        let wrap = |index: usize| wraps.get(index).copied().flatten();
        let row = usize::from(row);
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
        let mut line = Self {
            text: String::new(),
            origins: Vec::new(),
            cut_above: first == 0 && continues_from_above,
            cut_below: wrap(last).is_some(),
        };
        for (index, cells) in rows.iter().enumerate().take(last + 1).skip(first) {
            let limit =
                wrap(index).map_or(cells.len(), |count| usize::from(count).min(cells.len()));
            for (col, cell) in cells.iter().enumerate().take(limit) {
                let at = ViewportCell {
                    row: index as u16,
                    col: col as u16,
                };
                line.push_cell(cell, at);
            }
        }
        Some(line)
    }

    /// The URL covering the cell `at`, unless an edge rule drops it.
    fn url_at(&self, at: ViewportCell) -> Option<DetectedUrl> {
        let target = self.origins.iter().position(|origin| *origin == at)?;
        let found = UrlMatch::scan(&self.text)
            .into_iter()
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
                .is_none_or(|stop| found.raw.start < stop);
        let may_end_below = self.cut_below && found.raw.end == self.text.len();
        !may_start_above && !may_end_below
    }

    /// Appends the characters `cell` shows, or a blank for an OSC 8 cell.
    fn push_cell(&mut self, cell: &Cell, at: ViewportCell) {
        if cell.hyperlink_id.is_some() {
            self.push(' ', at);
            return;
        }
        for c in cell.chars() {
            self.push(c, at);
        }
    }

    fn push(&mut self, c: char, at: ViewportCell) {
        self.text.push(c);
        self.origins.resize(self.text.len(), at);
    }
}

/// Whether `c` may appear in a URL body.
fn is_url_body(c: char) -> bool {
    c.is_ascii_graphic() && !EXCLUDED_ASCII.contains(&c)
}

/// The byte offset where the next scheme at or after `from` starts, and
/// the offset where its body starts.
fn next_scheme(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    (from..bytes.len()).find_map(|start| {
        let bounded = start
            .checked_sub(1)
            .and_then(|before| bytes.get(before))
            .is_none_or(|byte| !byte.is_ascii_alphanumeric());
        let scheme = SCHEMES.iter().find(|scheme| {
            bytes
                .get(start..start + scheme.len())
                .is_some_and(|window| window.eq_ignore_ascii_case(scheme))
        })?;
        bounded.then_some((start, start + scheme.len()))
    })
}

/// The byte offset where the URL body starting at `body` ends, and
/// whether the character found there voids the URL.
fn body_end(text: &str, body: usize) -> (usize, bool) {
    let Some(rest) = text.get(body..) else {
        return (body, false);
    };
    let mut open: Vec<char> = Vec::new();
    for (offset, c) in rest.char_indices() {
        let at = body + offset;
        match c {
            '(' | '[' => open.push(c),
            ')' | ']' => {
                let opener = if c == ')' { '(' } else { '[' };
                if open.last() != Some(&opener) {
                    return (at, false);
                }
                open.pop();
            }
            _ if is_url_body(c) => {}
            _ => return (at, voids_url(c)),
        }
    }
    (text.len(), false)
}

/// Whether `c`, right after a URL body, voids the URL rather than
/// ending it there.
fn voids_url(c: char) -> bool {
    !c.is_ascii()
        && !c.is_whitespace()
        && !c.is_control()
        && UnicodeWidthChar::width(c) != Some(2)
        && !CLOSING_PUNCTUATION.contains(&c)
}

/// `end` moved back over trailing punctuation, never before `body`.
fn trimmed_end(bytes: &[u8], body: usize, end: usize) -> usize {
    let mut end = end;
    while end > body
        && bytes
            .get(end - 1)
            .is_some_and(|byte| TRAILING_TRIM.contains(byte))
    {
        end -= 1;
    }
    end
}

/// The scheme prefixes a URL may start with.
const SCHEMES: [&[u8]; 4] = [b"http://", b"https://", b"ftp://", b"mailto:"];

/// The printable ASCII characters a URL body never holds.
const EXCLUDED_ASCII: [char; 9] = ['<', '>', '"', '`', '{', '}', '|', '\\', '^'];

/// The non-ASCII characters besides whitespace and width-2 characters
/// that end a URL instead of voiding it.
const CLOSING_PUNCTUATION: [char; 6] = [
    '\u{2019}', '\u{201D}', '\u{00BB}', '\u{203A}', '\u{2013}', '\u{2014}',
];

/// The characters trimmed from the end of a URL.
const TRAILING_TRIM: [u8; 9] = [b'.', b',', b':', b';', b'!', b'?', b'\'', b'(', b'['];

#[cfg(test)]
mod tests;
