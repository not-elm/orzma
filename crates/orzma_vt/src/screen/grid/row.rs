//! One row of elements, ordered left to right.

use crate::screen::cell::Cell;
use crate::screen::grid::coords::GridColumn;
use crate::screen::grid::run::Run;
use std::ops::{Deref, DerefMut, Index, IndexMut};

/// A single row of `T`, left to right.
///
/// Storage rows are `Row<Cell>` and emitted rows are [`Row<Run>`], so
/// the two differ only in what one element spans: a cell is one
/// column, a run is as many as its text.
///
/// The element type is deliberately not defaulted — a bare `Row` in a
/// wire struct silently meaning `Row<Cell>` is exactly the mistake
/// that would compile and then fail far from its cause.
///
/// [`Row<Run>`]: crate::screen::grid::run::Run
#[derive(Debug, Clone, PartialEq)]
pub struct Row<T>(Vec<T>);

impl<T: Clone> Row<T> {
    /// Builds a row of `len` copies of `fill`.
    pub fn filled(len: u16, fill: T) -> Self {
        Self(vec![fill; usize::from(len)])
    }
}

impl Row<Cell> {
    /// Coalesces the row's cells into the attribute runs a frame
    /// carries.
    ///
    /// Adjacent cells sharing foreground, background, and style become
    /// one [`Run`], and the runs together span every column of the row.
    pub fn to_runs(&self) -> Row<Run> {
        let mut runs: Vec<Run> = Vec::with_capacity(self.0.len());
        for cell in self.0.iter() {
            match runs.last_mut() {
                Some(run) if run.fg == cell.fg && run.bg == cell.bg && run.style == cell.style => {
                    run.cols += 1;
                    run.text.push(cell.c);
                }
                _ => runs.push(Run {
                    cols: 1,
                    fg: cell.fg,
                    bg: cell.bg,
                    style: cell.style,
                    text: cell.c.to_string(),
                    hyperlink_id: None,
                }),
            }
        }
        Row(runs)
    }
}

impl<T> From<Vec<T>> for Row<T> {
    fn from(elements: Vec<T>) -> Self {
        Self(elements)
    }
}

impl<T> Deref for Row<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Row<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Indexes the element at a 0-based position.
///
/// The position is a column only for `Row<Cell>`; one
/// [`Run`](crate::screen::grid::run::Run) spans as many columns as its
/// text is wide.
impl<T> Index<u16> for Row<T> {
    type Output = T;

    fn index(&self, position: u16) -> &T {
        &self.0[usize::from(position)]
    }
}

impl<T> IndexMut<u16> for Row<T> {
    fn index_mut(&mut self, position: u16) -> &mut T {
        &mut self.0[usize::from(position)]
    }
}

/// Indexes the cell at a grid column.
///
/// Meaningful only for `Row<Cell>`, where one element is one column; a
/// [`Run`](crate::screen::grid::run::Run) spans as many columns as its
/// text is wide.
impl Index<GridColumn> for Row<Cell> {
    type Output = Cell;

    fn index(&self, column: GridColumn) -> &Cell {
        &self.0[usize::from(column.0)]
    }
}

impl IndexMut<GridColumn> for Row<Cell> {
    fn index_mut(&mut self, column: GridColumn) -> &mut Cell {
        &mut self.0[usize::from(column.0)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::color::Color;
    use crate::screen::grid::run::Style;

    fn cell(c: char, fg: Color, bg: Color, style: Style) -> Cell {
        Cell { c, fg, bg, style }
    }

    fn plain(c: char) -> Cell {
        Cell {
            c,
            ..Cell::default()
        }
    }

    /// Asserts that adjacent cells with identical attributes become one
    /// run carrying their glyphs in order.
    ///
    /// Case: a shell prints an unstyled word at the start of an
    /// otherwise blank line.
    #[test]
    fn cells_sharing_their_attributes_coalesce_into_one_run() {
        let row = Row::from(vec![plain('a'), plain('b'), plain('c')]);
        let runs = row.to_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "abc");
        assert_eq!(runs[0].cols, 3);
        assert_eq!(runs[0].fg, Color::DefaultForeground);
        assert_eq!(runs[0].bg, Color::DefaultBackground);
        assert_eq!(runs[0].style, Style::empty());
        assert!(runs[0].hyperlink_id.is_none());
    }

    /// Asserts that a difference in any one of foreground, background,
    /// or style starts a new run.
    ///
    /// Case: a prompt renders a colored user name, a differently
    /// highlighted path, and a bold marker on the same line.
    #[test]
    fn any_attribute_difference_starts_a_new_run() {
        let base = plain('a');
        let differing = [
            cell('b', Color::Indexed(1), base.bg, base.style),
            cell('b', base.fg, Color::Indexed(2), base.style),
            cell('b', base.fg, base.bg, Style::BOLD),
        ];
        for second in differing {
            let runs = Row::from(vec![base, second]).to_runs();
            assert_eq!(runs.len(), 2, "expected a split before {second:?}");
            assert_eq!(runs[0].text, "a");
            assert_eq!(runs[1].text, "b");
        }
    }

    /// Asserts that the runs span the row's full width, with the
    /// unwritten tail emitted rather than trimmed.
    ///
    /// Case: a short command is echoed onto a wide, freshly cleared
    /// line.
    #[test]
    fn the_runs_cover_every_column_including_a_blank_tail() {
        let mut row = Row::filled(8, Cell::default());
        row[0].c = 'h';
        row[1].c = 'i';
        let runs = row.to_runs();
        assert_eq!(runs.iter().map(|run| u32::from(run.cols)).sum::<u32>(), 8);
        assert_eq!(runs[0].text, "hi      ");
    }
}
