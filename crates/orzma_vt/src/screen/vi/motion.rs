//! Where each vi motion moves the vi cursor, read off the grid.

use crate::device::color::Color;
use crate::screen::cell::{Cell, CellWidth};
use crate::screen::grid::Grid;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint};
use crate::screen::grid::run::Style;
use crate::screen::vi::{SemanticEscapeChars, ViMotion};
use crate::screen::viewport::DisplayOffset;

/// The grid a vi motion reads, with the viewport and the word separators
/// the motion resolves against.
pub(crate) struct MotionGrid<'a> {
    grid: &'a Grid,
    offset: DisplayOffset,
    escape_chars: &'a SemanticEscapeChars,
}

impl<'a> MotionGrid<'a> {
    /// Builds the reader for `grid` shown at `offset`.
    pub fn new(
        grid: &'a Grid,
        offset: DisplayOffset,
        escape_chars: &'a SemanticEscapeChars,
    ) -> Self {
        Self {
            grid,
            offset,
            escape_chars,
        }
    }

    /// The point `motion` moves `point` to.
    ///
    /// The result stays inside the grid; a wide glyph counts as one cell.
    pub fn apply(&self, point: GridPoint, motion: ViMotion) -> GridPoint {
        let point = GridPoint {
            line: self.clamp_line(point.line.0),
            column: GridColumn(point.column.0.min(self.last_column())),
        };
        let rows = i32::from(self.grid.size().rows);
        match motion {
            ViMotion::Up => self.up(point),
            ViMotion::Down => self.down(point),
            ViMotion::Left => self.left(point),
            ViMotion::Right => self.right(point),
            ViMotion::First => self.first(point),
            ViMotion::Last => self.last(point),
            ViMotion::FirstOccupied => self.first_occupied(point),
            ViMotion::High => self.viewport_row(0),
            ViMotion::Middle => self.viewport_row(rows / 2 - 1),
            ViMotion::Low => self.viewport_row(rows - 1),
            ViMotion::SemanticLeft => self.semantic(point, Direction::Left, Direction::Left),
            ViMotion::SemanticRight => self.semantic(point, Direction::Right, Direction::Left),
            ViMotion::SemanticLeftEnd => self.semantic(point, Direction::Left, Direction::Right),
            ViMotion::SemanticRightEnd => self.semantic(point, Direction::Right, Direction::Right),
            ViMotion::WordLeft => self.word(point, Direction::Left, Direction::Left),
            ViMotion::WordRight => self.word(point, Direction::Right, Direction::Left),
            ViMotion::WordLeftEnd => self.word(point, Direction::Left, Direction::Right),
            ViMotion::WordRightEnd => self.word(point, Direction::Right, Direction::Right),
            ViMotion::Bracket => self.bracket_search(point).unwrap_or(point),
            ViMotion::ParagraphUp => self.paragraph_up(point),
            ViMotion::ParagraphDown => self.paragraph_down(point),
        }
    }

    /// The point a page motion of `lines` rows moves `point` to: the first
    /// non-blank cell of the row `lines` above it (below it when
    /// negative), or that row's first column when the row is blank. The
    /// row stays inside the grid.
    pub fn scroll_target(&self, point: GridPoint, lines: i32) -> GridPoint {
        let line = self.clamp_line(point.line.0.saturating_sub(lines));
        let column = self
            .first_occupied_in_line(line)
            .map_or(GridColumn(0), |occupied| occupied.column);
        GridPoint { line, column }
    }

    /// The oldest row the grid holds: the first history row, or the top
    /// row of the screen when there is no history.
    pub fn topmost(&self) -> GridLine {
        GridLine(-i32::try_from(self.grid.history_len()).unwrap_or(i32::MAX))
    }

    /// The bottom row of the screen.
    pub fn bottommost(&self) -> GridLine {
        GridLine(i32::from(self.grid.size().rows) - 1)
    }

    fn up(&self, mut point: GridPoint) -> GridPoint {
        if point.line.0 > self.topmost().0 {
            point.line.0 -= 1;
        }
        point
    }

    fn down(&self, mut point: GridPoint) -> GridPoint {
        if point.line.0 < self.bottommost().0 {
            point.line.0 += 1;
        }
        point
    }

    fn left(&self, point: GridPoint) -> GridPoint {
        let mut point = self.expand_wide(point, Direction::Left);
        let wrap_point = GridPoint {
            line: GridLine(point.line.0 - 1),
            column: GridColumn(self.last_column()),
        };
        if point.column.0 == 0 && point.line.0 > self.topmost().0 && self.is_wrap(wrap_point) {
            point = wrap_point;
        } else {
            point.column.0 = point.column.0.saturating_sub(1);
        }
        point
    }

    fn right(&self, point: GridPoint) -> GridPoint {
        let mut point = self.expand_wide(point, Direction::Right);
        if self.is_wrap(point) {
            point = GridPoint {
                line: self.clamp_line(point.line.0 + 1),
                column: GridColumn(0),
            };
        } else {
            point.column.0 = point.column.0.saturating_add(1).min(self.last_column());
        }
        point
    }

    fn first(&self, point: GridPoint) -> GridPoint {
        let mut point = self.expand_wide(point, Direction::Left);
        while point.column.0 == 0
            && point.line.0 > self.topmost().0
            && self.is_wrap(GridPoint {
                line: GridLine(point.line.0 - 1),
                column: GridColumn(self.last_column()),
            })
        {
            point.line.0 -= 1;
        }
        point.column = GridColumn(0);
        point
    }

    fn last(&self, point: GridPoint) -> GridPoint {
        let mut point = self.expand_wide(point, Direction::Right);
        match self.last_occupied_in_line(point.line) {
            Some(occupied) if point.column.0 < occupied.column.0 => occupied,
            _ if self.is_wrap(point) => {
                while self.is_wrap(point) && point.line.0 < self.bottommost().0 {
                    point.line.0 += 1;
                }
                self.last_occupied_in_line(point.line).unwrap_or(point)
            }
            _ => GridPoint {
                line: point.line,
                column: GridColumn(self.last_column()),
            },
        }
    }

    fn first_occupied(&self, point: GridPoint) -> GridPoint {
        let last_column = GridColumn(self.last_column());
        let point = self.expand_wide(point, Direction::Left);
        let occupied = self
            .first_occupied_in_line(point.line)
            .unwrap_or(GridPoint {
                line: point.line,
                column: last_column,
            });
        if point != occupied {
            return occupied;
        }
        let mut earlier = None;
        for line in (self.topmost().0..point.line.0).rev() {
            let line = GridLine(line);
            if !self.is_wrap(GridPoint {
                line,
                column: last_column,
            }) {
                break;
            }
            earlier = self.first_occupied_in_line(line).or(earlier);
        }
        if let Some(found) = earlier {
            return found;
        }
        let mut line = point.line;
        loop {
            if let Some(found) = self.first_occupied_in_line(line) {
                return found;
            }
            let last_cell = GridPoint {
                line,
                column: last_column,
            };
            if !self.is_wrap(last_cell) || line.0 >= self.bottommost().0 {
                return last_cell;
            }
            line.0 += 1;
        }
    }

    fn viewport_row(&self, rows_below_top: i32) -> GridPoint {
        let offset = i32::try_from(self.offset.0).unwrap_or(i32::MAX);
        let rows = i32::from(self.grid.size().rows);
        let top = -offset;
        let bottom = rows - 1 - offset;
        let line = GridLine((top + rows_below_top).clamp(top, bottom));
        let column = self
            .first_occupied_in_line(line)
            .map_or(GridColumn(0), |occupied| occupied.column);
        GridPoint { line, column }
    }

    fn semantic(&self, mut point: GridPoint, direction: Direction, side: Direction) -> GridPoint {
        if direction != side && !self.is_boundary(point, direction) {
            point = self.expand_semantic(point, direction);
        }
        point = self.expand_wide(point, direction);
        let mut next = self.advance(point, direction);
        while !self.is_boundary(point, direction) && self.is_space(next) {
            point = next;
            next = self.advance(point, direction);
        }
        if !self.is_boundary(point, direction) {
            point = self.advance(point, direction);
            if direction == Direction::Left {
                point = self.expand_wide(point, direction);
            }
        }
        if direction == side && !self.is_boundary(point, direction) {
            point = self.expand_semantic(point, direction);
        }
        point
    }

    fn expand_semantic(&self, point: GridPoint, direction: Direction) -> GridPoint {
        if self.is_separator(point) {
            return point;
        }
        match direction {
            Direction::Left => self.semantic_search_left(point),
            Direction::Right => self.semantic_search_right(point),
        }
    }

    fn word(&self, mut point: GridPoint, direction: Direction, side: Direction) -> GridPoint {
        point = self.expand_wide(point, direction);
        if direction == side {
            let mut next = self.advance(point, direction);
            while !self.is_boundary(point, direction) && self.is_space(next) {
                point = next;
                next = self.advance(point, direction);
            }
            while !self.is_boundary(point, direction) && !self.is_space(next) {
                point = next;
                next = self.advance(point, direction);
            }
        } else {
            while !self.is_boundary(point, direction) && !self.is_space(point) {
                point = self.advance(point, direction);
            }
            while !self.is_boundary(point, direction) && self.is_space(point) {
                point = self.advance(point, direction);
            }
        }
        point
    }

    fn semantic_search_left(&self, point: GridPoint) -> GridPoint {
        match self.inline_search_left(point) {
            Ok(found) => {
                let mut candidate = self.next_point(found);
                while let Some(at) = candidate {
                    if !self.cell(at).is_some_and(is_spacer) {
                        return at;
                    }
                    candidate = self.next_point(at);
                }
                found
            }
            Err(stopped) => stopped,
        }
    }

    fn semantic_search_right(&self, point: GridPoint) -> GridPoint {
        match self.inline_search_right(point) {
            Ok(found) => self.prev_point(found).unwrap_or(found),
            Err(stopped) => stopped,
        }
    }

    fn inline_search_left(&self, mut point: GridPoint) -> Result<GridPoint, GridPoint> {
        point.line = GridLine(point.line.0.max(self.topmost().0));
        let last_column = self.last_column();
        let mut cursor = point;
        while let Some(at) = self.prev_point(cursor) {
            cursor = at;
            if at.column.0 == last_column && !self.is_wrap(at) {
                break;
            }
            point = at;
            if self.is_separator(at) {
                return Ok(point);
            }
        }
        Err(point)
    }

    fn inline_search_right(&self, mut point: GridPoint) -> Result<GridPoint, GridPoint> {
        point.line = GridLine(point.line.0.max(self.topmost().0));
        let last_column = self.last_column();
        if point.column.0 == last_column && !self.is_wrap(point) {
            return Err(point);
        }
        let mut cursor = point;
        while let Some(at) = self.next_point(cursor) {
            cursor = at;
            point = at;
            if self.is_separator(at) {
                return Ok(point);
            }
            if at.column.0 == last_column && !self.is_wrap(at) {
                break;
            }
        }
        Err(point)
    }

    fn bracket_search(&self, point: GridPoint) -> Option<GridPoint> {
        let start = self.cell(point)?.c;
        let (forward, end) = BRACKET_PAIRS.iter().find_map(|&(open, close)| {
            if open == start {
                Some((true, close))
            } else if close == start {
                Some((false, open))
            } else {
                None
            }
        })?;
        let mut skip_pairs: u32 = 0;
        let mut cursor = point;
        loop {
            cursor = if forward {
                self.next_point(cursor)?
            } else {
                self.prev_point(cursor)?
            };
            let c = self.cell(cursor)?.c;
            if c == end && skip_pairs == 0 {
                return Some(cursor);
            }
            if c == start {
                skip_pairs = skip_pairs.saturating_add(1);
            } else if c == end {
                skip_pairs = skip_pairs.saturating_sub(1);
            }
        }
    }

    fn paragraph_up(&self, point: GridPoint) -> GridPoint {
        let topmost = self.topmost().0;
        let line = (topmost..=point.line.0)
            .rev()
            .skip_while(|&line| self.is_clear(GridLine(line)))
            .find(|&line| self.is_clear(GridLine(line)))
            .unwrap_or(topmost);
        GridPoint {
            line: GridLine(line),
            column: GridColumn(0),
        }
    }

    fn paragraph_down(&self, point: GridPoint) -> GridPoint {
        let bottommost = self.bottommost().0;
        let line = (point.line.0..bottommost)
            .skip_while(|&line| self.is_clear(GridLine(line)))
            .find(|&line| self.is_clear(GridLine(line)))
            .unwrap_or(bottommost);
        GridPoint {
            line: GridLine(line),
            column: GridColumn(0),
        }
    }

    fn expand_wide(&self, mut point: GridPoint, direction: Direction) -> GridPoint {
        let Some(width) = self.cell(point).map(|cell| cell.width) else {
            return point;
        };
        match (direction, width) {
            (Direction::Right, CellWidth::LeadingSpacer) if point.line.0 < self.bottommost().0 => {
                point = GridPoint {
                    line: GridLine(point.line.0 + 1),
                    column: GridColumn(1.min(self.last_column())),
                };
            }
            (Direction::Right, CellWidth::Wide) => {
                point.column.0 = point.column.0.saturating_add(1).min(self.last_column());
            }
            (Direction::Left, CellWidth::Wide | CellWidth::Spacer) => {
                if width == CellWidth::Spacer {
                    point.column.0 = point.column.0.saturating_sub(1);
                }
                let previous = self.advance(point, Direction::Left);
                if self
                    .cell(previous)
                    .is_some_and(|cell| cell.width == CellWidth::LeadingSpacer)
                {
                    point = previous;
                }
            }
            _ => {}
        }
        point
    }

    fn advance(&self, point: GridPoint, direction: Direction) -> GridPoint {
        match direction {
            Direction::Left => self.prev_point(point).unwrap_or(point),
            Direction::Right => self.next_point(point).unwrap_or(point),
        }
    }

    fn next_point(&self, point: GridPoint) -> Option<GridPoint> {
        if point.column.0 < self.last_column() {
            Some(GridPoint {
                line: point.line,
                column: GridColumn(point.column.0 + 1),
            })
        } else if point.line.0 < self.bottommost().0 {
            Some(GridPoint {
                line: GridLine(point.line.0 + 1),
                column: GridColumn(0),
            })
        } else {
            None
        }
    }

    fn prev_point(&self, point: GridPoint) -> Option<GridPoint> {
        if point.column.0 > 0 {
            Some(GridPoint {
                line: point.line,
                column: GridColumn(point.column.0 - 1),
            })
        } else if point.line.0 > self.topmost().0 {
            Some(GridPoint {
                line: GridLine(point.line.0 - 1),
                column: GridColumn(self.last_column()),
            })
        } else {
            None
        }
    }

    fn is_space(&self, point: GridPoint) -> bool {
        self.cell(point)
            .is_some_and(|cell| !is_spacer(cell) && (cell.c == ' ' || cell.c == '\t'))
    }

    /// Whether the glyph at `point` ends a semantic word: a blank, a tab, or
    /// one of the configured separators.
    fn is_separator(&self, point: GridPoint) -> bool {
        self.cell(point).is_some_and(|cell| {
            !is_spacer(cell) && (matches!(cell.c, ' ' | '\t') || self.escape_chars.contains(cell.c))
        })
    }

    fn is_wrap(&self, point: GridPoint) -> bool {
        point.column.0 == self.last_column() && self.grid.wrap_at(point.line).is_some()
    }

    fn is_boundary(&self, point: GridPoint, direction: Direction) -> bool {
        match direction {
            Direction::Left => point.line.0 <= self.topmost().0 && point.column.0 == 0,
            Direction::Right => {
                point.line.0 >= self.bottommost().0 && point.column.0 >= self.last_column()
            }
        }
    }

    /// Whether `line` separates paragraphs: it does not wrap and every cell
    /// is empty; a line outside the ring counts as clear.
    fn is_clear(&self, line: GridLine) -> bool {
        self.grid.wrap_at(line).is_none()
            && self
                .grid
                .row_at(line)
                .is_none_or(|row| row.iter().all(is_empty_cell))
    }

    fn first_occupied_in_line(&self, line: GridLine) -> Option<GridPoint> {
        (0..self.grid.size().cols)
            .map(|column| GridPoint {
                line,
                column: GridColumn(column),
            })
            .find(|&point| !self.is_space(point))
    }

    fn last_occupied_in_line(&self, line: GridLine) -> Option<GridPoint> {
        (0..self.grid.size().cols)
            .map(|column| GridPoint {
                line,
                column: GridColumn(column),
            })
            .rfind(|&point| !self.is_space(point))
    }

    fn cell(&self, point: GridPoint) -> Option<&Cell> {
        self.grid
            .row_at(point.line)?
            .get(usize::from(point.column.0))
    }

    fn clamp_line(&self, line: i32) -> GridLine {
        GridLine(line.clamp(self.topmost().0, self.bottommost().0))
    }

    fn last_column(&self) -> u16 {
        self.grid.size().cols.saturating_sub(1)
    }
}

/// The pairs `Bracket` matches, opening bracket first.
const BRACKET_PAIRS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

/// A direction along the grid in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Toward the top-left cell.
    Left,
    /// Toward the bottom-right cell.
    Right,
}

/// Whether `cell` is a continuation column or a wrap filler rather than a
/// glyph of its own.
fn is_spacer(cell: &Cell) -> bool {
    matches!(cell.width, CellWidth::Spacer | CellWidth::LeadingSpacer)
}

/// Whether `cell` shows nothing a paragraph motion counts as text: a
/// narrow blank or tab in the default colors, without reverse video,
/// underline, strike-through, or combining marks.
fn is_empty_cell(cell: &Cell) -> bool {
    matches!(cell.c, ' ' | '\t')
        && cell.width == CellWidth::Narrow
        && cell.fg == Color::DefaultForeground
        && cell.bg == Color::DefaultBackground
        && !cell
            .style
            .intersects(Style::REVERSE | Style::UNDERLINE | Style::STRIKE)
        && cell.extra.is_none()
}

#[cfg(test)]
mod tests;
