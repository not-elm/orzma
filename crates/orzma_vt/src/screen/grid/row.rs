//! One row of elements, ordered left to right.

use crate::screen::cell::Cell;
use crate::screen::grid::coords::GridColumn;
use crate::screen::grid::run::Run;
use std::ops::{Deref, DerefMut, Index, IndexMut};

/// A single row of `T`, left to right.
///
/// Storage rows are `Row<Cell>` and emitted rows are [`Row<Run>`]; a
/// cell spans one column, and a run one column per `char` of its text.
///
/// [`Row<Run>`]: crate::screen::grid::run::Run
#[derive(Debug, Clone, PartialEq)]
pub struct Row<T>(Vec<T>);

impl<T: Clone> Row<T> {
    /// Builds a row of `len` copies of `fill`.
    pub fn filled(len: u16, fill: T) -> Self {
        Self(vec![fill; usize::from(len)])
    }

    /// Grows the row to `len` with copies of `fill`, or truncates it
    /// to `len`, keeping the elements the two lengths share.
    pub fn resize(&mut self, len: u16, fill: T) {
        self.0.resize(usize::from(len), fill);
    }
}

impl Row<Cell> {
    /// How many runs [`Row::to_runs`] reserves up front.
    const RUNS_RESERVE: usize = 8;

    /// Coalesces the row's cells into the attribute runs a frame
    /// carries.
    ///
    /// Adjacent cells sharing foreground, background, and style become
    /// one [`Run`], and the runs together span every column of the row.
    pub fn to_runs(&self) -> Row<Run> {
        let mut runs: Vec<Run> = Vec::with_capacity(self.0.len().min(Self::RUNS_RESERVE));
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
/// [`Run`](crate::screen::grid::run::Run) spans one column per `char`
/// of its text.
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

    /// Asserts that a resize to a longer length appends copies of the
    /// fill and leaves the existing elements untouched.
    ///
    /// Case: the user widens the terminal window.
    #[test]
    fn a_resize_to_a_longer_length_appends_the_fill() {
        let mut row = Row::from(vec![plain('a'), plain('b')]);
        row.resize(4, plain('.'));
        assert_eq!(row.len(), 4);
        assert_eq!(row[0].c, 'a');
        assert_eq!(row[2].c, '.');
        assert_eq!(row[3].c, '.');
    }

    /// Asserts that a resize to a shorter length drops the elements past
    /// the new end and keeps the ones before it.
    ///
    /// Case: the user drags the window narrower.
    #[test]
    fn a_resize_to_a_shorter_length_drops_the_tail() {
        let mut row = Row::from(vec![plain('a'), plain('b'), plain('c'), plain('d')]);
        row.resize(2, Cell::default());
        assert_eq!(row.len(), 2);
        assert_eq!(row[0].c, 'a');
        assert_eq!(row[1].c, 'b');
    }

    /// Asserts that a resize to the length the row already has leaves it
    /// unchanged.
    ///
    /// Case: the window manager replays the same geometry after a focus
    /// change.
    #[test]
    fn a_resize_to_the_same_length_changes_nothing() {
        let mut row = Row::from(vec![plain('a'), plain('b'), plain('c')]);
        row.resize(3, Cell::default());
        assert_eq!(row.len(), 3);
        assert_eq!(row[1].c, 'b');
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

    /// Asserts that a single-attribute row's run vector reserves at most
    /// `RUNS_RESERVE` runs rather than one per column.
    ///
    /// Case: a frame carries one unstyled blank line of a wide terminal,
    /// which coalesces into a single run.
    #[test]
    fn a_single_attribute_row_reserves_at_most_the_run_reserve() {
        let row = Row::filled(200, Cell::default());
        let runs = row.to_runs();
        assert_eq!(runs.len(), 1);
        assert!(
            runs.0.capacity() <= Row::RUNS_RESERVE,
            "capacity {} exceeds the reserve of {}",
            runs.0.capacity(),
            Row::RUNS_RESERVE
        );
    }
}
