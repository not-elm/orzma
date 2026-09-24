//! Rewrapping rows at a new width: the logical lines rows form, the
//! positions a rewrap carries, and what a resize does with the rows it
//! frees.

use crate::screen::cell::{Cell, CellWidth, Pen};
use crate::screen::grid::coords::{GridColumn, GridLine, ScreenLine};
use crate::screen::grid::row::Row;
use crate::screen::grid::{GridRow, LineId};
use std::mem;

/// What a resize does with the rows it frees at the bottom of the
/// screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollbackOnGrow {
    /// Rows come back from scrollback, keeping the content anchored to
    /// the bottom.
    #[default]
    Reclaim,
    /// Scrollback stays put and blank rows fill the bottom.
    Keep,
}

/// A position a reflow carries from the old layout to the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TrackedPoint {
    /// The row, in active-grid coordinates.
    line: GridLine,
    /// A cell boundary in `0..=cols`, where `cols` is the row's right
    /// edge.
    boundary: u16,
}

impl TrackedPoint {
    /// Builds the position at cell boundary `boundary`, in `0..=cols` for
    /// a row `cols` wide, on the active-grid row `line`.
    pub fn new(line: GridLine, boundary: u16) -> Self {
        Self { line, boundary }
    }

    /// The position a cursor at `line` and `column` stands for; an armed
    /// deferred wrap puts it on the right edge, `cols`.
    pub fn cursor(line: ScreenLine, column: GridColumn, pending_wrap: bool, cols: u16) -> Self {
        Self {
            line: GridLine::from(line),
            boundary: if pending_wrap { cols } else { column.0 },
        }
    }

    /// The position's row, in active-grid coordinates.
    pub fn line(&self) -> GridLine {
        self.line
    }

    /// The position's cell boundary, in `0..=cols` for a row `cols` wide.
    pub fn boundary(&self) -> u16 {
        self.boundary
    }
}

/// A position inside one run of rows being rewrapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlicePoint {
    /// The row's index inside the run.
    row: usize,
    /// A cell boundary in `0..=cols`, where `cols` is the row's right
    /// edge.
    boundary: u16,
}

/// Where a position falls inside one logical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Spot {
    /// Cells from the start of the line to the position.
    offset: usize,
    /// Whether the position sat on a row's right edge, so that a cut
    /// landing on it keeps it at the end of the earlier row.
    at_edge: bool,
}

/// One logical line gathered out of the rows that hold it.
struct Gathered {
    /// The id of the line's first row.
    id: LineId,
    /// The line's cells, without those past each row's recorded wrap.
    cells: Vec<Cell>,
    /// Each gathered row's share of `cells`.
    rows: Vec<GatheredRow>,
    /// Whether the last row gathered ends the line.
    ends: bool,
}

/// One row's share of a [`Gathered`] line.
struct GatheredRow {
    /// The row's index in the run.
    index: usize,
    /// Where the row's first cell sits in the line.
    start: usize,
    /// How many of the row's leading cells the line takes.
    taken: usize,
}

impl Gathered {
    /// Opens a line on the run's row `index`.
    fn open(index: usize, row: GridRow) -> Self {
        let mut line = Self {
            id: row.id,
            cells: Vec::new(),
            rows: Vec::new(),
            ends: true,
        };
        line.push(index, row);
        line
    }

    /// Whether the last row gathered continues on the next row.
    fn continues(&self) -> bool {
        !self.ends
    }

    /// Appends the run's row `index` to the line, taking its cells up to
    /// its recorded wrap, and never taking a wide glyph without its
    /// continuation column.
    fn push(&mut self, index: usize, row: GridRow) {
        let len = row.cells.len();
        let recorded = row.wrap_at.map_or(len, |cells| usize::from(cells).min(len));
        let splits_a_pair = recorded > 0
            && recorded < len
            && row
                .cells
                .get(recorded - 1)
                .is_some_and(|cell| cell.width == CellWidth::Wide);
        let taken = if splits_a_pair {
            recorded + 1
        } else {
            recorded
        };
        let start = self.cells.len();
        let mut cells = row.cells.into_inner();
        cells.truncate(taken);
        cells.iter_mut().for_each(normalize);
        if self.cells.is_empty() {
            self.cells = cells;
        } else {
            self.cells.append(&mut cells);
        }
        self.rows.push(GatheredRow {
            index,
            start,
            taken,
        });
        self.ends = row.wrap_at.is_none();
    }

    /// Where a position on the run's row `point.row` falls in the line;
    /// `None` when that row is not part of the line. `old_cols` is the
    /// width the position was taken at.
    fn spot(&self, point: SlicePoint, old_cols: u16) -> Option<Spot> {
        let row = self.rows.iter().find(|row| row.index == point.row)?;
        Some(Spot {
            offset: row.start + usize::from(point.boundary).min(row.taken),
            at_edge: point.boundary >= old_cols,
        })
    }

    /// Cuts the line at `cols` onto the end of `out` and reports where the
    /// positions that sat on the line landed.
    ///
    /// `cursor` and `points` are the positions as the run held them before
    /// any line was cut, so each is looked up by its original row only.
    fn cut_into(
        self,
        out: &mut Vec<GridRow>,
        mint: &mut impl FnMut() -> LineId,
        cursor: Option<SlicePoint>,
        points: &[Option<SlicePoint>],
        old_cols: u16,
        cols: u16,
    ) -> Landed {
        let base = out.len();
        let cursor_spot = cursor.and_then(|point| self.spot(point, old_cols));
        let point_spots: Vec<(usize, Spot)> = points
            .iter()
            .enumerate()
            .filter_map(|(index, point)| {
                point
                    .and_then(|point| self.spot(point, old_cols))
                    .map(|spot| (index, spot))
            })
            .collect();
        let Self {
            id,
            mut cells,
            ends,
            ..
        } = self;
        let keep_end = if ends {
            text_end(&cells)
                .max(cursor_spot.map_or(0, |spot| spot.offset))
                .min(cells.len())
        } else {
            cells.len()
        };
        let fill = match cells.last() {
            Some(last) if keep_end < cells.len() => Pen {
                fg: last.fg,
                bg: last.bg,
                style: last.style,
            }
            .erase_cell(),
            _ => Cell::default(),
        };
        cells.truncate(keep_end);
        let cut = Cut::of(cells, cols);
        let width = usize::from(cols);
        let last = cut.rows.len().saturating_sub(1);
        let cursor_placed = cursor_spot.map(|spot| cut.place(spot, width));
        let extra = matches!(cursor_placed, Some(Placed::PastFullEnd));
        let placed_points: Vec<(usize, Placed)> = point_spots
            .into_iter()
            .map(|(index, spot)| {
                let clamped = Spot {
                    offset: spot.offset.min(keep_end),
                    ..spot
                };
                (index, cut.place(clamped, width))
            })
            .collect();
        for (k, (mut row, count)) in cut.rows.into_iter().zip(cut.counts).enumerate() {
            let pad = if k == last {
                fill.clone()
            } else {
                Cell::default()
            };
            row.resize(width, pad);
            let recorded = u16::try_from(count).unwrap_or(cols);
            let wrap_at = if k < last || extra || !ends {
                Some(recorded)
            } else {
                None
            };
            let id = if k == 0 { id } else { mint() };
            out.push(GridRow {
                id,
                cells: Row::from(row),
                wrap_at,
            });
        }
        if extra {
            out.push(GridRow {
                id: mint(),
                cells: Row::filled(cols, Cell::default()),
                wrap_at: None,
            });
        }
        let to_point = |placed: Placed| match placed {
            Placed::At { row, boundary } => SlicePoint {
                row: base + row,
                boundary: u16::try_from(boundary).unwrap_or(cols),
            },
            Placed::PastFullEnd => SlicePoint {
                row: base + last,
                boundary: cols,
            },
        };
        Landed {
            cursor: cursor_placed.map(|placed| match placed {
                Placed::PastFullEnd => SlicePoint {
                    row: base + last + 1,
                    boundary: 0,
                },
                placed => to_point(placed),
            }),
            points: placed_points
                .into_iter()
                .map(|(index, placed)| (index, to_point(placed)))
                .collect(),
        }
    }
}

/// Where the positions that sat on one cut line landed.
struct Landed {
    /// The cursor's new position, when it sat on the line.
    cursor: Option<SlicePoint>,
    /// Each position that sat on the line, by its index, and where it
    /// landed.
    points: Vec<(usize, SlicePoint)>,
}

/// A logical line's cells cut into rows of one width.
struct Cut {
    /// Each row's cells, not yet filled out to the width.
    rows: Vec<Vec<Cell>>,
    /// Where each row starts in the line.
    starts: Vec<usize>,
    /// How many of the line's cells each row holds.
    counts: Vec<usize>,
}

/// Where a position lands in a [`Cut`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placed {
    /// On the cut's row `row`, at `boundary`.
    At { row: usize, boundary: usize },
    /// Just past the last row, which is full.
    PastFullEnd,
}

impl Cut {
    /// Cuts `cells` into rows `cols` wide. A wide glyph that would start
    /// in the last column leaves a filler there and opens the next row.
    fn of(cells: Vec<Cell>, cols: u16) -> Self {
        let width = usize::from(cols);
        if cells.len() <= width {
            let count = cells.len();
            return Self {
                rows: vec![cells],
                starts: vec![0],
                counts: vec![count],
            };
        }
        let mut cut = Self {
            rows: Vec::new(),
            starts: Vec::new(),
            counts: Vec::new(),
        };
        let mut row: Vec<Cell> = Vec::with_capacity(width);
        let mut start = 0;
        let mut offset = 0;
        let mut cells = cells.into_iter().peekable();
        while let Some(cell) = cells.next() {
            let span = if cell.width == CellWidth::Wide { 2 } else { 1 };
            if row.len() + span > width {
                let count = row.len();
                if span == 2 && row.len() + 1 == width {
                    row.push(
                        Pen {
                            fg: cell.fg,
                            bg: cell.bg,
                            style: cell.style,
                        }
                        .filler(),
                    );
                }
                cut.close(
                    mem::replace(&mut row, Vec::with_capacity(width)),
                    start,
                    count,
                );
                start = offset;
            }
            if span == 2 {
                let spacer = cells
                    .next_if(|next| next.width == CellWidth::Spacer)
                    .unwrap_or_else(|| cell.continuation());
                row.push(cell);
                row.push(spacer);
            } else {
                row.push(cell);
            }
            offset += span;
        }
        let count = row.len();
        cut.close(row, start, count);
        cut
    }

    /// Where `spot` lands: an edge position at the end of the row its
    /// preceding cell sits on, any other at the cell it precedes.
    fn place(&self, spot: Spot, width: usize) -> Placed {
        let last = self.rows.len().saturating_sub(1);
        if spot.at_edge {
            let row = self
                .starts
                .iter()
                .zip(&self.counts)
                .position(|(start, count)| spot.offset <= start + count)
                .unwrap_or(last);
            let start = self.starts.get(row).copied().unwrap_or(0);
            return Placed::At {
                row,
                boundary: spot.offset.saturating_sub(start),
            };
        }
        let row = self
            .starts
            .iter()
            .rposition(|start| *start <= spot.offset)
            .unwrap_or(0);
        let start = self.starts.get(row).copied().unwrap_or(0);
        let count = self.counts.get(row).copied().unwrap_or(0);
        let boundary = spot.offset.saturating_sub(start);
        if row == last && boundary >= width && count >= width {
            return Placed::PastFullEnd;
        }
        Placed::At {
            row,
            boundary: boundary.min(width),
        }
    }

    /// Closes one row of `count` line cells that starts at `start`.
    fn close(&mut self, row: Vec<Cell>, start: usize, count: usize) {
        self.rows.push(row);
        self.starts.push(start);
        self.counts.push(count);
    }
}

/// Rewraps `rows`, each `old_cols` wide, at `cols`, rewriting `cursor`
/// and `points` from indices into `rows` to indices into the result.
///
/// Rows joined by `wrap_at` form one logical line, cut afresh at `cols`.
/// A line that ends loses its trailing blanks, judged by text, but never
/// those before the cursor, and its last row is filled out with the
/// colors of the last blank it lost. A line the run cuts off keeps every
/// cell. The first row of each line keeps its id; `mint` names every row
/// the cut adds. Each position moves exactly once, from the row it stood
/// on before the rewrap.
fn rewrap(
    mut cursor: Option<&mut SlicePoint>,
    points: &mut [Option<SlicePoint>],
    mint: &mut impl FnMut() -> LineId,
    rows: Vec<GridRow>,
    old_cols: u16,
    cols: u16,
) -> Vec<GridRow> {
    let cursor_from = cursor.as_deref().copied();
    let points_from = points.to_vec();
    let mut out = Vec::with_capacity(rows.len());
    let mut rows = rows.into_iter().enumerate();
    while let Some((index, row)) = rows.next() {
        let mut line = Gathered::open(index, row);
        while line.continues() {
            let Some((index, row)) = rows.next() else {
                break;
            };
            line.push(index, row);
        }
        let landed = line.cut_into(&mut out, mint, cursor_from, &points_from, old_cols, cols);
        if let (Some(point), Some(moved)) = (cursor.as_deref_mut(), landed.cursor) {
            *point = moved;
        }
        for (index, moved) in landed.points {
            if let Some(slot) = points.get_mut(index) {
                *slot = Some(moved);
            }
        }
    }
    out
}

/// Whether `cell` shows no text: a blank glyph without marks, whatever
/// its colors.
fn is_blank(cell: &Cell) -> bool {
    cell.c == ' ' && cell.extra.is_none() && cell.width != CellWidth::Wide
}

/// Whether any cell of `row` shows text.
fn has_text(row: &GridRow) -> bool {
    row.cells.iter().any(|cell| !is_blank(cell))
}

/// The length of `cells` up to and including its last cell that shows
/// text, a wide glyph's continuation included.
fn text_end(cells: &[Cell]) -> usize {
    let Some(last) = cells.iter().rposition(|cell| !is_blank(cell)) else {
        return 0;
    };
    if cells[last].width == CellWidth::Wide {
        (last + 2).min(cells.len())
    } else {
        last + 1
    }
}

/// Turns a wrap filler found inside a line into a plain blank.
fn normalize(cell: &mut Cell) {
    if cell.width == CellWidth::LeadingSpacer {
        cell.width = CellWidth::Narrow;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::color::Color;
    use crate::screen::cell::{BodyWidth, GlyphClass};

    /// A row `cols` wide holding `text`, a wide glyph taking two columns.
    fn row_of(text: &str, cols: u16, wrap_at: Option<u16>, id: u64) -> GridRow {
        let mut cells: Vec<Cell> = Vec::new();
        for c in text.chars() {
            if GlyphClass::of(c) == Some(GlyphClass::Wide) {
                let body = Pen::default().stamp(c, BodyWidth::Wide, None);
                let continuation = body.continuation();
                cells.push(body);
                cells.push(continuation);
            } else {
                cells.push(Pen::default().stamp(c, BodyWidth::Narrow, None));
            }
        }
        cells.resize(usize::from(cols), Cell::default());
        GridRow {
            id: LineId(id),
            cells: Row::from(cells),
            wrap_at,
        }
    }

    fn text_of(row: &GridRow) -> String {
        row.cells
            .iter()
            .flat_map(Cell::chars)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn texts(rows: &[GridRow]) -> Vec<String> {
        rows.iter().map(text_of).collect()
    }

    fn wraps(rows: &[GridRow]) -> Vec<Option<u16>> {
        rows.iter().map(|row| row.wrap_at).collect()
    }

    fn minter() -> impl FnMut() -> LineId {
        let mut next = 100;
        move || {
            next += 1;
            LineId(next)
        }
    }

    fn at(row: usize, boundary: u16) -> SlicePoint {
        SlicePoint { row, boundary }
    }

    /// Asserts that narrowing splits a line into rows of the new width,
    /// records each wrap, keeps the first row's id, and mints the rest.
    ///
    /// Case: the user narrows the window under a long line of output.
    #[test]
    fn narrowing_splits_a_line_and_records_the_wraps() {
        let rows = vec![row_of("abcdef", 8, None, 1)];
        let out = rewrap(None, &mut [], &mut minter(), rows, 8, 4);
        assert_eq!(texts(&out), ["abcd", "ef"]);
        assert_eq!(wraps(&out), [Some(4), None]);
        assert_eq!(out[0].id, LineId(1));
        assert_ne!(out[1].id, LineId(1));
        assert!(out.iter().all(|row| row.cells.len() == 4));
    }

    /// Asserts that widening joins the rows a wrap links into one row.
    ///
    /// Case: the user widens the window back after a long line wrapped.
    #[test]
    fn widening_joins_wrapped_rows() {
        let rows = vec![row_of("abcd", 4, Some(4), 1), row_of("ef", 4, None, 2)];
        let out = rewrap(None, &mut [], &mut minter(), rows, 4, 8);
        assert_eq!(texts(&out), ["abcdef"]);
        assert_eq!(wraps(&out), [None]);
        assert_eq!(out[0].id, LineId(1));
    }

    /// Asserts that a wide glyph that would start in the last column
    /// leaves a filler there and moves to the next row.
    ///
    /// Case: Japanese text is narrowed so a character straddles the edge.
    #[test]
    fn a_wide_glyph_moves_to_the_next_row_leaving_a_filler() {
        let rows = vec![row_of("abあ", 4, None, 1)];
        let out = rewrap(None, &mut [], &mut minter(), rows, 4, 3);
        assert_eq!(texts(&out), ["ab", "あ"]);
        assert_eq!(wraps(&out), [Some(2), None]);
        assert_eq!(out[0].cells[2u16].width, CellWidth::LeadingSpacer);
        assert!(out.iter().all(|row| row.cells.wide_pairs_intact()));
    }

    /// Asserts that a join leaves out the filler a wide glyph left at the
    /// end of the earlier row.
    ///
    /// Case: the user widens the window back after Japanese text wrapped
    /// with a filler.
    #[test]
    fn a_join_leaves_out_the_filler() {
        let mut first = row_of("ab", 3, Some(2), 1);
        first.cells[2u16] = Pen::default().filler();
        let rows = vec![first, row_of("あ", 3, None, 2)];
        let out = rewrap(None, &mut [], &mut minter(), rows, 3, 6);
        assert_eq!(texts(&out), ["abあ"]);
        assert!(
            out[0]
                .cells
                .iter()
                .all(|cell| cell.width != CellWidth::LeadingSpacer)
        );
    }

    /// Asserts that blanks an erase left with a foreground color are
    /// trimmed by text, and their colors fill the end of the rewrapped
    /// row.
    ///
    /// Case: PowerShell erases the rest of its input line while the
    /// input's highlight color is active, and the user narrows the window.
    #[test]
    fn erased_blanks_are_trimmed_by_text_and_fill_the_row_end() {
        let mut row = row_of("ab", 8, None, 1);
        let erased = Pen {
            fg: Color::Indexed(3),
            ..Pen::default()
        }
        .erase_cell();
        for column in 2..8u16 {
            row.cells[column] = erased.clone();
        }
        let out = rewrap(None, &mut [], &mut minter(), vec![row], 8, 4);
        assert_eq!(texts(&out), ["ab"]);
        assert_eq!(out[0].cells[3u16].fg, Color::Indexed(3));
    }

    /// Asserts that blanks up to the cursor are kept, and a cursor left
    /// just past a full last row gets a fresh row to sit on.
    ///
    /// Case: a prompt ending in a space exactly fills the new width.
    #[test]
    fn a_cursor_past_a_full_last_row_gets_a_fresh_row() {
        let rows = vec![row_of("PS>", 8, None, 1)];
        let mut cursor = at(0, 4);
        let out = rewrap(Some(&mut cursor), &mut [], &mut minter(), rows, 8, 4);
        assert_eq!(texts(&out), ["PS>", ""]);
        assert_eq!(wraps(&out), [Some(4), None]);
        assert_eq!(cursor, at(1, 0));
    }

    /// Asserts that a cursor parked on a right edge stays on the right
    /// edge of the row its text ends on.
    ///
    /// Case: a prompt fills its row and arms the deferred wrap, then the
    /// user narrows the window.
    #[test]
    fn a_parked_cursor_stays_on_the_right_edge() {
        let rows = vec![row_of("abcd", 4, None, 1)];
        let mut cursor = at(0, 4);
        let out = rewrap(Some(&mut cursor), &mut [], &mut minter(), rows, 4, 2);
        assert_eq!(texts(&out), ["ab", "cd"]);
        assert_eq!(cursor, at(1, 2));
    }

    /// Asserts that a cursor at the start of a continuation row stays at
    /// the start of a row rather than parking on the previous row's edge.
    ///
    /// Case: the user deletes back to the wrap point of a long command and
    /// narrows the window.
    #[test]
    fn a_cursor_at_a_row_start_stays_at_a_row_start() {
        let rows = vec![row_of("abcd", 4, Some(4), 1), row_of("ef", 4, None, 2)];
        let mut cursor = at(1, 0);
        let out = rewrap(Some(&mut cursor), &mut [], &mut minter(), rows, 4, 2);
        assert_eq!(texts(&out), ["ab", "cd", "ef"]);
        assert_eq!(cursor, at(2, 0));
    }

    /// Asserts that a passive position past the kept text moves to the
    /// text's end instead of shaping the layout.
    ///
    /// Case: a selection was dragged to the right edge of a short line
    /// before the window narrowed.
    #[test]
    fn a_passive_point_past_the_text_moves_to_its_end() {
        let rows = vec![row_of("ab", 8, None, 1)];
        let mut points = [Some(at(0, 6))];
        let out = rewrap(None, &mut points, &mut minter(), rows, 8, 4);
        assert_eq!(texts(&out), ["ab"]);
        assert_eq!(points, [Some(at(0, 2))]);
    }

    /// Asserts that a line cut at the end of the run keeps its trailing
    /// cells and records how many continue.
    ///
    /// Case: history is rewrapped while its newest line continues onto
    /// the screen.
    #[test]
    fn a_cut_line_keeps_its_cells_and_records_its_length() {
        let rows = vec![row_of("ab", 4, Some(4), 1)];
        let out = rewrap(None, &mut [], &mut minter(), rows, 4, 8);
        assert_eq!(wraps(&out), [Some(4)]);
        assert_eq!(out[0].cells.len(), 8);
    }

    /// Asserts that each position moves once, from the row it stood on,
    /// even when a narrowing gives it an index a later line held.
    ///
    /// Case: the user has a row selected from its right edge into the next
    /// line and narrows the window.
    #[test]
    fn each_position_moves_once() {
        let rows = vec![row_of("abcd", 4, None, 1), row_of("efgh", 4, None, 2)];
        let mut points = [Some(at(0, 4)), Some(at(1, 0))];
        let out = rewrap(None, &mut points, &mut minter(), rows, 4, 2);
        assert_eq!(texts(&out), ["ab", "cd", "ef", "gh"]);
        assert_eq!(points, [Some(at(1, 2)), Some(at(2, 0))]);
    }

    /// Asserts that a recorded wrap ending on a wide glyph's body still
    /// takes the glyph's continuation, keeping every wide pair intact.
    ///
    /// Case: a line editor inserted a blank into a row whose wrap stops
    /// before a Japanese character, and the user widens the window.
    #[test]
    fn a_recorded_wrap_never_splits_a_wide_pair() {
        let rows = vec![row_of(" xあ", 4, Some(3), 1), row_of("あ", 4, None, 2)];
        let out = rewrap(None, &mut [], &mut minter(), rows, 4, 6);
        assert_eq!(texts(&out), [" xああ"]);
        assert!(out.iter().all(|row| row.cells.wide_pairs_intact()));
    }

    /// Asserts that an empty row stays one row and keeps its fill color.
    ///
    /// Case: a colored blank line in the output is rewrapped.
    #[test]
    fn an_empty_row_stays_one_row_with_its_color() {
        let mut row = row_of("", 4, None, 1);
        let erased = Pen {
            bg: Color::Indexed(4),
            ..Pen::default()
        }
        .erase_cell();
        for column in 0..4u16 {
            row.cells[column] = erased.clone();
        }
        let out = rewrap(None, &mut [], &mut minter(), vec![row], 4, 8);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].cells[7u16].bg, Color::Indexed(4));
    }
}
