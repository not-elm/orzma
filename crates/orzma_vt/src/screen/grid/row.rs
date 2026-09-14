//! One row of elements, ordered left to right.

use crate::error::{StampError, VtResult};
use crate::hyperlink::HyperlinkId;
use crate::screen::cell::{BodyWidth, Cell, CellExtra, CellWidth, Pen};
use crate::screen::grid::coords::GridColumn;
use crate::screen::grid::run::Run;
use std::ops::{Deref, DerefMut, Index, IndexMut};

/// A single row of `T`, left to right.
///
/// Storage rows are `Row<Cell>` and emitted rows are [`Row<Run>`]; a
/// cell spans one column, and a run the columns its widths sum to.
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
    /// Adjacent cells sharing foreground, background, style, and
    /// hyperlink become one [`Run`], and the runs together span every
    /// column of the row. A continuation column adds nothing to a run's
    /// text, a filler adds one blank, and a cell's marks follow its glyph.
    pub fn to_runs(&self) -> Row<Run> {
        let mut runs: Vec<Run> = Vec::with_capacity(self.0.len().min(Self::RUNS_RESERVE));
        let mut chars_in_run = 0usize;
        for cell in self.0.iter() {
            if cell.width == CellWidth::Spacer {
                continue;
            }
            let width: u8 = if cell.width == CellWidth::Wide { 2 } else { 1 };
            let starts_new = !matches!(runs.last(), Some(run) if run.continues_with(cell));
            if starts_new {
                runs.push(Run {
                    cols: 0,
                    fg: cell.fg,
                    bg: cell.bg,
                    style: cell.style,
                    text: String::new(),
                    widths: Vec::new(),
                    hyperlink_id: cell.hyperlink_id,
                });
                chars_in_run = 0;
            }
            let run = runs.last_mut().expect("the row has a run to extend");
            run.cols += u16::from(width);
            Self::push_width(run, &mut chars_in_run, width);
            run.text.push(cell.c);
            for mark in cell.extra.as_deref().map_or(&[][..], CellExtra::marks) {
                Self::push_width(run, &mut chars_in_run, 0);
                run.text.push(*mark);
            }
        }
        Row(runs)
    }

    /// Stamps `c` at `column` with `pen`'s attributes inside
    /// `hyperlink_id`, adding the continuation column when `width` is
    /// `BodyWidth::Wide`, and restores the wide-pair invariant on both
    /// sides of the write.
    ///
    /// # Errors
    ///
    /// [`StampError::OutOfRow`] when `column`, or the continuation of a
    /// `Wide` glyph, lies past the end of the row. The row is left
    /// unchanged.
    pub fn stamp_at(
        &mut self,
        column: u16,
        c: char,
        width: BodyWidth,
        pen: &Pen,
        hyperlink_id: Option<HyperlinkId>,
    ) -> VtResult {
        let start = usize::from(column);
        let end = match width {
            BodyWidth::Narrow => start,
            BodyWidth::Wide => start + 1,
        };
        if end >= self.0.len() {
            return Err(StampError::OutOfRow.into());
        }
        let cell = pen.stamp(c, width, hyperlink_id);
        if width == BodyWidth::Wide {
            self.0[end] = cell.continuation();
        }
        self.0[start] = cell;
        if start > 0 {
            self.heal_joint(start - 1);
        }
        self.heal_joint(end);
        Ok(())
    }

    /// Stamps the last column as the blank a wide glyph leaves behind
    /// when it does not fit there, carrying `pen`'s attributes, and
    /// restores the wide-pair invariant to its left.
    pub fn place_filler(&mut self, pen: &Pen) {
        debug_assert!(!self.0.is_empty(), "a filler needs a column");
        let last = self.0.len() - 1;
        self.0[last] = pen.filler();
        if last > 0 {
            self.heal_joint(last - 1);
        }
        debug_assert!(
            last == 0 || self.joint_intact(last - 1),
            "a filler left a broken joint"
        );
    }

    /// Restores the wide-pair invariant across the whole row.
    pub fn normalize_wide_pairs(&mut self) {
        let cols = self.0.len();
        for at in 0..cols {
            match self.0[at].width {
                CellWidth::Wide => {
                    if at + 1 >= cols || self.0[at + 1].width != CellWidth::Spacer {
                        self.blank_in_place(at);
                    }
                }
                CellWidth::Spacer => {
                    if at == 0 || self.0[at - 1].width != CellWidth::Wide {
                        self.blank_in_place(at);
                    } else {
                        let continuation = self.0[at - 1].continuation();
                        self.0[at] = continuation;
                    }
                }
                CellWidth::LeadingSpacer => {
                    if at + 1 != cols {
                        self.blank_in_place(at);
                    } else {
                        let filler = &mut self.0[at];
                        filler.c = ' ';
                        filler.extra = None;
                    }
                }
                CellWidth::Narrow => {}
            }
        }
        debug_assert!(
            self.wide_pairs_intact(),
            "a normalization left a broken wide pair"
        );
    }

    /// Restores the single joint a column resize from `old_cols` can
    /// break.
    ///
    /// `old_cols` is the row's length before the resize that already
    /// happened, and the row must have held an intact wide-pair
    /// invariant at that length.
    pub fn repair_after_resize(&mut self, old_cols: u16) {
        if self.0.is_empty() {
            return;
        }
        if self.0.len() == usize::from(old_cols) {
            return;
        }
        let last = self.0.len() - 1;
        if self.0.len() < usize::from(old_cols) {
            if self.0[last].width == CellWidth::Wide {
                self.blank_in_place(last);
            }
        } else {
            let old_last = usize::from(old_cols).saturating_sub(1);
            if old_last < self.0.len() && self.0[old_last].width == CellWidth::LeadingSpacer {
                self.blank_in_place(old_last);
            }
        }
    }

    /// Whether every wide body in the row is followed by its
    /// continuation, every continuation is preceded by its body, every
    /// filler sits in the last column, and every continuation or filler
    /// holds a blank glyph with no combining marks.
    pub fn wide_pairs_intact(&self) -> bool {
        let cols = self.0.len();
        self.0
            .iter()
            .enumerate()
            .all(|(at, cell)| match cell.width {
                CellWidth::Wide => self.joint_intact(at),
                CellWidth::Spacer => {
                    at > 0 && self.joint_intact(at - 1) && cell.c == ' ' && cell.extra.is_none()
                }
                CellWidth::LeadingSpacer => at + 1 == cols && cell.c == ' ' && cell.extra.is_none(),
                CellWidth::Narrow => true,
            })
    }

    /// Restores the wide-pair invariant at the joint between `left` and
    /// `left + 1`, blanking whichever half lost its partner.
    fn heal_joint(&mut self, left: usize) {
        let right = left + 1;
        if right >= self.0.len() {
            if self.0[left].width == CellWidth::Wide {
                self.blank_in_place(left);
            }
            return;
        }
        let body = self.0[left].width == CellWidth::Wide;
        let continuation = self.0[right].width == CellWidth::Spacer;
        if body && !continuation {
            self.blank_in_place(left);
        } else if continuation && !body {
            self.blank_in_place(right);
        }
    }

    /// Whether the joint between `left` and `left + 1` pairs up.
    fn joint_intact(&self, left: usize) -> bool {
        let right = left + 1;
        let body = self.0[left].width == CellWidth::Wide;
        if right >= self.0.len() {
            return !body;
        }
        body == (self.0[right].width == CellWidth::Spacer)
    }

    /// Turns the cell into a blank narrow cell, keeping its colors and
    /// styling.
    fn blank_in_place(&mut self, at: usize) {
        let cell = &mut self.0[at];
        cell.c = ' ';
        cell.width = CellWidth::Narrow;
        cell.extra = None;
    }

    /// Records the width of the next `char` of `run`, creating the width
    /// list on the first width other than one.
    fn push_width(run: &mut Run, chars_in_run: &mut usize, width: u8) {
        if width != 1 || !run.widths.is_empty() {
            run.widths.resize(*chars_in_run, 1);
            run.widths.push(width);
        }
        *chars_in_run += 1;
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
/// [`Run`](crate::screen::grid::run::Run) spans the columns its widths
/// sum to.
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
    use crate::error::VtError;
    use crate::hyperlink::HyperlinkId;
    use crate::screen::grid::run::Style;

    fn cell(c: char, fg: Color, bg: Color, style: Style) -> Cell {
        Cell {
            c,
            fg,
            bg,
            style,
            ..Cell::default()
        }
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
            let runs = Row::from(vec![base.clone(), second.clone()]).to_runs();
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

    fn wide_body(c: char) -> Cell {
        Cell {
            c,
            width: CellWidth::Wide,
            ..Cell::default()
        }
    }

    /// Asserts that stamping a narrow cell over a wide body blanks the
    /// continuation column the body left behind.
    ///
    /// Case: a program overwrites the left half of a fullwidth character
    /// with an ASCII letter.
    #[test]
    fn stamping_over_a_wide_body_blanks_its_continuation() {
        let body = wide_body('あ');
        let spacer = body.continuation();
        let mut row = Row::from(vec![body, spacer, Cell::default()]);
        row.stamp_at(0, 'a', BodyWidth::Narrow, &Pen::default(), None)
            .expect("the stamp fits");
        assert_eq!(row[GridColumn(1)].width, CellWidth::Narrow);
        assert_eq!(row[GridColumn(1)].c, ' ');
        assert!(row.wide_pairs_intact());
    }

    /// Asserts that stamping a narrow cell over a continuation column
    /// blanks the wide body to its left.
    ///
    /// Case: a program overwrites the right half of a fullwidth
    /// character.
    #[test]
    fn stamping_over_a_continuation_blanks_its_body() {
        let body = wide_body('あ');
        let spacer = body.continuation();
        let mut row = Row::from(vec![body, spacer, Cell::default()]);
        row.stamp_at(1, 'a', BodyWidth::Narrow, &Pen::default(), None)
            .expect("the stamp fits");
        assert_eq!(row[GridColumn(0)].width, CellWidth::Narrow);
        assert_eq!(row[GridColumn(0)].c, ' ');
        assert!(row.wide_pairs_intact());
    }

    /// Asserts that a repair keeps the colors and styling of the cell it
    /// blanks.
    ///
    /// Case: a fullwidth character inside a colored region is half
    /// overwritten.
    #[test]
    fn a_repair_keeps_the_colors_of_the_cell_it_blanks() {
        let mut body = wide_body('あ');
        body.bg = Color::Indexed(4);
        body.style = Style::BOLD;
        let spacer = body.continuation();
        let mut row = Row::from(vec![body, spacer, Cell::default()]);
        row.stamp_at(1, 'a', BodyWidth::Narrow, &Pen::default(), None)
            .expect("the stamp fits");
        assert_eq!(row[GridColumn(0)].bg, Color::Indexed(4));
        assert_eq!(row[GridColumn(0)].style, Style::BOLD);
    }

    /// Asserts that stamping a wide cell writes its continuation column.
    ///
    /// Case: a fullwidth character is printed with both of its columns
    /// available.
    #[test]
    fn stamping_a_wide_cell_writes_its_continuation() {
        let mut row = Row::from(vec![Cell::default(), Cell::default()]);
        row.stamp_at(0, '界', BodyWidth::Wide, &Pen::default(), None)
            .expect("the stamp fits");
        assert_eq!(row[GridColumn(0)].width, CellWidth::Wide);
        assert_eq!(row[GridColumn(1)].width, CellWidth::Spacer);
        assert!(row.wide_pairs_intact());
    }

    /// Asserts that a wide body whose continuation would fall past the
    /// end of the row is refused, leaving the row unchanged.
    ///
    /// Case: a caller stamps a fullwidth character into the last column
    /// without wrapping first.
    #[test]
    fn a_wide_body_at_the_last_column_is_refused() {
        let mut row = Row::from(vec![Cell::default(), Cell::default()]);
        let before = row.clone();
        let result = row.stamp_at(1, 'あ', BodyWidth::Wide, &Pen::default(), None);
        assert!(matches!(result, Err(VtError::Stamp(StampError::OutOfRow))));
        assert_eq!(row, before);
    }

    /// Asserts that a full-row normalization repairs a pair broken in
    /// the middle of the row.
    ///
    /// Case: an insert shifts the right half of a fullwidth character
    /// away from its body.
    #[test]
    fn normalization_repairs_a_pair_broken_mid_row() {
        let body = wide_body('あ');
        let spacer = body.continuation();
        let mut row = Row::from(vec![body, Cell::default(), spacer, Cell::default()]);
        row.normalize_wide_pairs();
        assert_eq!(row[GridColumn(0)].width, CellWidth::Narrow);
        assert_eq!(row[GridColumn(2)].width, CellWidth::Narrow);
        assert!(row.wide_pairs_intact());
    }

    /// Asserts that a normalization gives a kept continuation the pen of
    /// its body.
    ///
    /// Case: an edit leaves a continuation column carrying colors that no
    /// longer match the glyph it covers.
    #[test]
    fn normalization_repens_a_kept_continuation() {
        let mut body = wide_body('あ');
        body.bg = Color::Indexed(4);
        let mut spacer = body.continuation();
        spacer.bg = Color::Indexed(1);
        let mut row = Row::from(vec![body, spacer]);
        row.normalize_wide_pairs();
        assert_eq!(row[GridColumn(1)].bg, Color::Indexed(4));
    }

    /// Asserts that a shrink blanks a wide body whose continuation the
    /// truncation removed.
    ///
    /// Case: the user drags the window narrow enough to cut a fullwidth
    /// character in half.
    #[test]
    fn a_shrink_blanks_a_body_whose_continuation_was_cut() {
        let body = wide_body('あ');
        let spacer = body.continuation();
        let mut row = Row::from(vec![Cell::default(), body, spacer]);
        row.resize(2, Cell::default());
        row.repair_after_resize(3);
        assert_eq!(row[GridColumn(1)].width, CellWidth::Narrow);
        assert!(row.wide_pairs_intact());
    }

    /// Asserts that a growth blanks a leading spacer that is no longer in
    /// the last column.
    ///
    /// Case: the user widens the window after a fullwidth character
    /// wrapped at the old right edge.
    #[test]
    fn a_growth_blanks_a_leading_spacer_that_is_no_longer_last() {
        let filler = Cell {
            width: CellWidth::LeadingSpacer,
            ..Cell::default()
        };
        let mut row = Row::from(vec![Cell::default(), filler]);
        row.resize(4, Cell::default());
        row.repair_after_resize(2);
        assert_eq!(row[GridColumn(1)].width, CellWidth::Narrow);
        assert!(row.wide_pairs_intact());
    }

    /// Asserts that a repair called with the row's unchanged length keeps
    /// a legal last-column leading spacer intact.
    ///
    /// Case: the window manager replays the same geometry after a
    /// fullwidth character wrapped at the right edge.
    #[test]
    fn a_repair_at_the_same_length_keeps_a_last_column_leading_spacer() {
        let filler = Cell {
            width: CellWidth::LeadingSpacer,
            ..Cell::default()
        };
        let mut row = Row::from(vec![Cell::default(), filler]);
        row.repair_after_resize(2);
        assert_eq!(row[GridColumn(1)].width, CellWidth::LeadingSpacer);
    }

    /// Asserts that a normalization blanks a filler in the last column
    /// that carries a non-blank glyph.
    ///
    /// Case: an in-row insert shifts a glyph into the last column that a
    /// wrapped fullwidth character had left as a filler.
    #[test]
    fn normalization_blanks_a_last_column_filler_carrying_a_glyph() {
        let filler = Cell {
            c: 'X',
            width: CellWidth::LeadingSpacer,
            ..Cell::default()
        };
        let mut row = Row::from(vec![Cell::default(), filler]);
        row.normalize_wide_pairs();
        assert_eq!(row[GridColumn(1)].c, ' ');
        assert!(row.wide_pairs_intact());
    }

    /// Asserts that placing a filler stamps the last column as a leading
    /// spacer carrying the given pen and a blank glyph.
    ///
    /// Case: a fullwidth character arrives with one column left on a row
    /// whose background color is set.
    #[test]
    fn placing_a_filler_stamps_the_last_column_with_the_pen() {
        let pen = Pen {
            fg: Color::Indexed(1),
            bg: Color::Indexed(4),
            style: Style::BOLD,
        };
        let mut row = Row::from(vec![plain('a'), plain('b'), plain('c')]);
        row.place_filler(&pen);
        let filler = &row[GridColumn(2)];
        assert_eq!(filler.width, CellWidth::LeadingSpacer);
        assert_eq!(filler.c, ' ');
        assert_eq!(filler.extra, None);
        assert_eq!(
            (filler.fg, filler.bg, filler.style),
            (pen.fg, pen.bg, pen.style)
        );
        assert_eq!(row[GridColumn(1)].c, 'b');
        assert!(row.wide_pairs_intact());
    }

    /// Asserts that placing a filler over a continuation column blanks
    /// the wide body to its left.
    ///
    /// Case: a fullwidth character wraps on a row whose last two columns
    /// hold another fullwidth character.
    #[test]
    fn placing_a_filler_over_a_continuation_blanks_its_body() {
        let body = wide_body('あ');
        let spacer = body.continuation();
        let mut row = Row::from(vec![plain('a'), body, spacer]);
        row.place_filler(&Pen::default());
        assert_eq!(row[GridColumn(1)].width, CellWidth::Narrow);
        assert_eq!(row[GridColumn(1)].c, ' ');
        assert_eq!(row[GridColumn(2)].width, CellWidth::LeadingSpacer);
        assert!(row.wide_pairs_intact());
    }

    fn marked(c: char, marks: &[char]) -> Cell {
        let mut extra = CellExtra::default();
        for mark in marks {
            assert!(extra.push(*mark));
        }
        Cell {
            c,
            extra: Some(Box::new(extra)),
            ..Cell::default()
        }
    }

    /// Asserts that a row of one-column glyphs without marks emits runs
    /// with an empty width list.
    ///
    /// Case: a shell prints an ASCII command line.
    #[test]
    fn a_row_of_narrow_glyphs_emits_no_widths() {
        let row = Row::from(vec![plain('a'), plain('b'), plain('c')]);
        let runs = row.to_runs();
        assert_eq!(runs[0].text, "abc");
        assert_eq!(runs[0].cols, 3);
        assert!(runs[0].widths.is_empty());
    }

    /// Asserts that a wide body emits its glyph once at width two and its
    /// continuation column contributes nothing to the text.
    ///
    /// Case: a shell echoes a Japanese character.
    #[test]
    fn a_wide_pair_emits_one_glyph_at_width_two() {
        let body = wide_body('あ');
        let spacer = body.continuation();
        let row = Row::from(vec![body, spacer]);
        let runs = row.to_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "あ");
        assert_eq!(runs[0].cols, 2);
        assert_eq!(runs[0].widths, vec![2]);
    }

    /// Asserts that combining marks follow their base in the text at
    /// width zero.
    ///
    /// Case: a shell echoes `e` followed by a combining acute accent.
    #[test]
    fn combining_marks_follow_their_base_at_width_zero() {
        let row = Row::from(vec![marked('e', &['\u{0301}']), plain('x')]);
        let runs = row.to_runs();
        assert_eq!(runs[0].text, "e\u{0301}x");
        assert_eq!(runs[0].cols, 2);
        assert_eq!(runs[0].widths, vec![1, 0, 1]);
    }

    /// Asserts that narrow glyphs before the first wide one are filled in
    /// at width one when the width list is created.
    ///
    /// Case: a shell prints `ab界` with one pen.
    #[test]
    fn widths_created_mid_run_cover_the_earlier_glyphs() {
        let body = wide_body('界');
        let spacer = body.continuation();
        let row = Row::from(vec![plain('a'), plain('b'), body, spacer]);
        let runs = row.to_runs();
        assert_eq!(runs[0].text, "ab界");
        assert_eq!(runs[0].cols, 4);
        assert_eq!(runs[0].widths, vec![1, 1, 2]);
    }

    /// Asserts that a leading spacer is emitted as one blank column and
    /// keeps an otherwise plain row on the empty-width path.
    ///
    /// Case: an ASCII row whose last column a wrapped Japanese character
    /// left blank.
    #[test]
    fn a_leading_spacer_emits_one_blank_column() {
        let filler = Cell {
            width: CellWidth::LeadingSpacer,
            ..Cell::default()
        };
        let row = Row::from(vec![plain('a'), filler]);
        let runs = row.to_runs();
        assert_eq!(runs[0].text, "a ");
        assert_eq!(runs[0].cols, 2);
        assert!(runs[0].widths.is_empty());
    }

    /// Asserts that the column count of every run equals the sum of its
    /// widths, across a pen change inside a row that mixes every class.
    ///
    /// Case: a colored prompt is followed by Japanese text and an
    /// accented letter.
    #[test]
    fn every_run_spans_the_sum_of_its_widths() {
        let mut colored = plain('$');
        colored.fg = Color::Indexed(2);
        let body = wide_body('あ');
        let spacer = body.continuation();
        let row = Row::from(vec![colored, body, spacer, marked('e', &['\u{0301}'])]);
        let runs = row.to_runs();
        assert_eq!(runs.len(), 2);
        for run in runs.iter() {
            let sum: u16 = run.widths.iter().map(|w| u16::from(*w)).sum();
            let expected = if run.widths.is_empty() {
                run.text.chars().count() as u16
            } else {
                sum
            };
            assert_eq!(run.cols, expected, "run {:?}", run.text);
        }
        assert_eq!(runs.iter().map(|run| run.cols).sum::<u16>(), 4);
    }

    /// Asserts that a row splits into separate runs where the hyperlink
    /// changes, even when every other attribute matches.
    ///
    /// Case: a directory listing prints one clickable file name straight
    /// after another with no styling between them.
    #[test]
    fn runs_split_where_the_hyperlink_changes() {
        let link = HyperlinkId::new(7).expect("nonzero");
        let other = HyperlinkId::new(9).expect("nonzero");
        let mut row = Row::filled(3, Cell::default());
        row[0].hyperlink_id = Some(link);
        row[1].hyperlink_id = Some(other);
        let runs = row.to_runs();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].hyperlink_id, Some(link));
        assert_eq!(runs[1].hyperlink_id, Some(other));
        assert_eq!(runs[2].hyperlink_id, None);
    }

    /// Asserts that neighbouring cells sharing one hyperlink coalesce
    /// into a single run.
    ///
    /// Case: a program prints a multi-word link, and the renderer draws
    /// it as one underlined span.
    #[test]
    fn cells_sharing_a_hyperlink_coalesce_into_one_run() {
        let link = HyperlinkId::new(7).expect("nonzero");
        let mut row = Row::filled(3, Cell::default());
        for column in 0..3 {
            row[column].hyperlink_id = Some(link);
        }
        let runs = row.to_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].cols, 3);
        assert_eq!(runs[0].hyperlink_id, Some(link));
    }
}
