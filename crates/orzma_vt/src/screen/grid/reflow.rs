//! Rewrapping rows at a new width: the logical lines rows form, the
//! positions a rewrap carries, and what a resize does with the rows it
//! frees.

use crate::screen::cell::{Cell, CellWidth, Pen};
use crate::screen::grid::coords::{GridColumn, GridLine, ScreenLine};
use crate::screen::grid::row::Row;
use crate::screen::grid::{Grid, GridRow, GridSize, LineId};
use std::collections::VecDeque;
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

impl Grid {
    /// Resizes the grid to `size`, rewrapping its rows at the new width
    /// and carrying `cursor`, `saved`, and `points` to the text they stood
    /// on.
    ///
    /// The row holding the old top row's first cell stays on top where the
    /// text allows: rows move into history only as far as the cursor, or
    /// the text below it, needs to stay on screen, and the rest past the
    /// bottom are dropped, leaving the bottom row to end its logical line.
    /// Rows a resize frees at the bottom come back from history under
    /// [`ScrollbackOnGrow::Reclaim`] and stay blank under
    /// [`ScrollbackOnGrow::Keep`]. A width change rewraps history as well;
    /// a height-only change rewraps nothing. Under
    /// [`ScrollbackOnGrow::Keep`], history and the screen are rewrapped
    /// apart, so a line split between them joins only once both halves sit
    /// in history. Under [`ScrollbackOnGrow::Reclaim`], the history rows of
    /// a line that runs onto the screen are rewrapped with the screen, and
    /// the rows of that line above the old top row go back to history.
    /// `size.cols` must be at least two.
    ///
    /// Blank rows below both the cursor and the last row showing text are
    /// not kept. `cursor` always lands on the screen. `saved` lands on the
    /// screen too: on row zero when its row moved into history or past
    /// the cap, and on the last row when its row fell off the bottom. Each
    /// of `points` on a blank row that is not kept keeps its distance below
    /// the last row that is kept, its boundary clamped to the new width.
    /// Each of `points` becomes `None` when the row it lands on falls past
    /// the history cap or off the bottom of the screen, and may otherwise
    /// land in history.
    ///
    /// # Invariants
    ///
    /// Every row is `size.cols` wide afterwards, history never exceeds its
    /// cap, and every row this adds carries a freshly minted id.
    pub fn reflow(
        &mut self,
        cursor: &mut TrackedPoint,
        saved: &mut TrackedPoint,
        points: &mut [Option<TrackedPoint>],
        size: GridSize,
        policy: ScrollbackOnGrow,
    ) {
        let old = self.size;
        let new_rows = usize::from(size.rows);
        let rewraps = old.cols != size.cols;
        let fit = |boundary: u16| {
            if boundary >= old.cols {
                size.cols
            } else {
                boundary.min(size.cols.saturating_sub(1))
            }
        };
        let history_len = self.history_len();
        let mut history: Vec<GridRow> = Vec::from(mem::take(&mut self.rows));
        let mut screen: Vec<GridRow> = history.split_off(history_len.min(history.len()));
        let old_rows = screen.len();
        let mut carried: Vec<Option<Carried>> = points
            .iter()
            .map(|point| point.map(|point| Carried::at(point, history_len, old_rows)))
            .collect();
        carried.push(Some(Carried::at(*saved, history_len, old_rows)));
        let mut cursor_at = match Carried::at(*cursor, history_len, old_rows) {
            Carried::Screen(point) => point,
            _ => SlicePoint {
                row: 0,
                boundary: 0,
            },
        };

        let last_text = screen.iter().rposition(has_text).unwrap_or(0);
        let mut extent = last_text
            .max(cursor_at.row)
            .min(screen.len().saturating_sub(1));
        while extent + 1 < screen.len() && screen[extent].wrap_at.is_some() {
            extent += 1;
        }
        screen.truncate(extent + 1);
        for slot in carried.iter_mut().flatten() {
            if let Carried::Screen(point) = *slot
                && point.row > extent
            {
                *slot = Carried::Below {
                    rows: point.row - extent,
                    boundary: point.boundary,
                };
            }
        }

        let mut old_top = SlicePoint {
            row: 0,
            boundary: 0,
        };
        if policy == ScrollbackOnGrow::Reclaim && rewraps {
            let joined = continued_tail(&history);
            if joined > 0 {
                let from = history.len() - joined;
                let mut run: Vec<GridRow> = history.drain(from..).collect();
                run.append(&mut screen);
                screen = run;
                old_top.row += joined;
                cursor_at.row += joined;
                for slot in carried.iter_mut().flatten() {
                    *slot = match *slot {
                        Carried::History(point) if point.row >= from => {
                            Carried::Screen(SlicePoint {
                                row: point.row - from,
                                ..point
                            })
                        }
                        Carried::Screen(point) => Carried::Screen(SlicePoint {
                            row: point.row + joined,
                            ..point
                        }),
                        other => other,
                    };
                }
            }
        }

        if rewraps {
            let mut screen_points: Vec<Option<SlicePoint>> = carried
                .iter()
                .map(|slot| slot.and_then(Carried::screen))
                .chain([Some(old_top)])
                .collect();
            screen = rewrap(
                Some(&mut cursor_at),
                &mut screen_points,
                &mut || self.mint(),
                screen,
                old.cols,
                size.cols,
            );
            if let Some(Some(moved)) = screen_points.pop() {
                old_top = moved;
            }
            for (slot, point) in carried.iter_mut().zip(screen_points) {
                if let (Some(Carried::Screen(_)), Some(point)) = (*slot, point) {
                    *slot = Some(Carried::Screen(point));
                }
            }
        }

        let reflowed = screen.len();
        for slot in carried.iter_mut().flatten() {
            if let Carried::Below { rows, boundary } = *slot {
                *slot = Carried::Screen(SlicePoint {
                    row: reflowed.saturating_sub(1) + rows,
                    boundary: fit(boundary),
                });
            }
        }

        let last_row = screen
            .iter()
            .rposition(has_text)
            .unwrap_or(0)
            .max(cursor_at.row);
        let top = (last_row + 1)
            .saturating_sub(new_rows)
            .max(old_top.row)
            .min(cursor_at.row);
        let padded = new_rows.saturating_sub(reflowed.saturating_sub(top));
        let reclaimed = if policy == ScrollbackOnGrow::Reclaim {
            let grown = i64::from(size.rows) - i64::from(old.rows);
            let saved_rows = i64::try_from(extent + 1).unwrap_or(0)
                - (i64::try_from(reflowed).unwrap_or(0) - i64::try_from(old_top.row).unwrap_or(0));
            usize::try_from(grown + saved_rows).unwrap_or(0).min(padded)
        } else {
            0
        };

        if rewraps {
            let mut history_points: Vec<Option<SlicePoint>> = carried
                .iter()
                .map(|slot| slot.and_then(Carried::history))
                .collect();
            // NOTE: `needed` must stay at least the number of history rows
            // the push, pull and cap steps below can keep, or rows that
            // should survive the cap are dropped without being rewrapped.
            let needed = self
                .max_history
                .saturating_add(reclaimed)
                .saturating_sub(top);
            history = rewrap_newest(
                None,
                &mut history_points,
                &mut || self.mint(),
                history,
                old.cols,
                size.cols,
                needed,
            );
            for (slot, point) in carried.iter_mut().zip(history_points) {
                if let Some(Carried::History(_)) = *slot {
                    *slot = Some(point.map_or(Carried::LostAbove, Carried::History));
                }
            }
        }

        let pushed_from = history.len();
        history.extend(screen.drain(..top));
        cursor_at.row -= top;
        for slot in carried.iter_mut().flatten() {
            if let Carried::Screen(point) = *slot {
                *slot = if point.row < top {
                    Carried::History(SlicePoint {
                        row: pushed_from + point.row,
                        ..point
                    })
                } else if point.row - top >= new_rows {
                    Carried::LostBelow
                } else {
                    Carried::Screen(SlicePoint {
                        row: point.row - top,
                        ..point
                    })
                };
            }
        }
        screen.truncate(new_rows);
        if let Some(last) = screen.last_mut() {
            last.wrap_at = None;
        }
        for _ in 0..padded {
            let id = self.mint();
            screen.push(GridRow {
                id,
                cells: Row::filled(size.cols, Cell::default()),
                wrap_at: None,
            });
        }

        let pulled = reclaimed.min(history.len());
        if pulled > 0 {
            let from = history.len() - pulled;
            screen.truncate(screen.len() - pulled);
            let mut rebuilt: Vec<GridRow> = history.drain(from..).collect();
            rebuilt.append(&mut screen);
            screen = rebuilt;
            cursor_at.row += pulled;
            for slot in carried.iter_mut().flatten() {
                *slot = match *slot {
                    Carried::History(point) if point.row >= from => Carried::Screen(SlicePoint {
                        row: point.row - from,
                        ..point
                    }),
                    Carried::Screen(point) if point.row + pulled >= new_rows => Carried::LostBelow,
                    Carried::Screen(point) => Carried::Screen(SlicePoint {
                        row: point.row + pulled,
                        ..point
                    }),
                    other => other,
                };
            }
        }

        let excess = history.len().saturating_sub(self.max_history);
        if excess > 0 {
            history.drain(..excess);
            for slot in carried.iter_mut().flatten() {
                if let Carried::History(point) = *slot {
                    *slot = if point.row < excess {
                        Carried::LostAbove
                    } else {
                        Carried::History(SlicePoint {
                            row: point.row - excess,
                            ..point
                        })
                    };
                }
            }
        }

        self.history_index.rebuild(history.iter().map(|row| row.id));
        let history_len = history.len();
        let mut rows = VecDeque::from(history);
        rows.extend(screen);
        self.rows = rows;
        self.size = size;

        let line_of = |row: usize| GridLine(i32::try_from(row).unwrap_or(i32::MAX));
        let history_line = |row: usize| {
            GridLine(
                i32::try_from(row).unwrap_or(i32::MAX)
                    - i32::try_from(history_len).unwrap_or(i32::MAX),
            )
        };
        *cursor = TrackedPoint {
            line: line_of(cursor_at.row),
            boundary: cursor_at.boundary,
        };
        let saved_slot = carried.pop().flatten();
        *saved = match saved_slot {
            Some(Carried::Screen(point)) => TrackedPoint {
                line: line_of(point.row),
                boundary: point.boundary,
            },
            Some(Carried::LostBelow) => TrackedPoint {
                line: line_of(new_rows.saturating_sub(1)),
                boundary: fit(saved.boundary),
            },
            Some(Carried::History(point)) => TrackedPoint {
                line: GridLine(0),
                boundary: point.boundary,
            },
            _ => TrackedPoint {
                line: GridLine(0),
                boundary: fit(saved.boundary),
            },
        };
        for (point, slot) in points.iter_mut().zip(carried) {
            *point = match slot {
                Some(Carried::History(slice)) => Some(TrackedPoint {
                    line: history_line(slice.row),
                    boundary: slice.boundary,
                }),
                Some(Carried::Screen(slice)) => Some(TrackedPoint {
                    line: line_of(slice.row),
                    boundary: slice.boundary,
                }),
                _ => None,
            };
        }
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

/// Where one carried position stands while the ring is rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carried {
    /// On the history row at this index, oldest first.
    History(SlicePoint),
    /// On the screen row at this index.
    Screen(SlicePoint),
    /// This many rows below the last row the screen reflow kept.
    Below { rows: usize, boundary: u16 },
    /// Gone with a row the history cap dropped.
    LostAbove,
    /// Gone with a row dropped off the bottom of the screen.
    LostBelow,
}

impl Carried {
    /// Where `point` stands in a ring holding `history` history rows above
    /// `rows` screen rows.
    fn at(point: TrackedPoint, history: usize, rows: usize) -> Self {
        let index = i64::try_from(history).unwrap_or(i64::MAX) + i64::from(point.line.0);
        let Ok(index) = usize::try_from(index) else {
            return Self::LostAbove;
        };
        let slice = |row| SlicePoint {
            row,
            boundary: point.boundary,
        };
        if index < history {
            Self::History(slice(index))
        } else if index - history < rows {
            Self::Screen(slice(index - history))
        } else {
            Self::LostBelow
        }
    }

    /// The history position this stands for, if any.
    fn history(self) -> Option<SlicePoint> {
        match self {
            Self::History(point) => Some(point),
            _ => None,
        }
    }

    /// The screen position this stands for, if any.
    fn screen(self) -> Option<SlicePoint> {
        match self {
            Self::Screen(point) => Some(point),
            _ => None,
        }
    }
}

/// How many rows at the end of `history` hold a logical line that
/// continues onto the screen; zero when the newest history row ends its
/// line.
fn continued_tail(history: &[GridRow]) -> usize {
    if history.last().is_none_or(|row| row.wrap_at.is_none()) {
        return 0;
    }
    let start = history
        .iter()
        .rposition(|row| row.wrap_at.is_none())
        .map_or(0, |index| index + 1);
    history.len() - start
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

impl Spot {
    /// Where a position on the run's row `point.row` falls in the line
    /// whose rows' shares are `shares`; `None` when that row is not part
    /// of the line. `old_cols` is the width the position was taken at.
    fn on(shares: &[Share], point: SlicePoint, old_cols: u16) -> Option<Self> {
        let share = shares.iter().find(|share| share.index == point.row)?;
        Some(Self {
            offset: share.start + usize::from(point.boundary).min(share.taken),
            at_edge: point.boundary >= old_cols,
        })
    }
}

/// One row's share of the logical line it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Share {
    /// The row's index in the run.
    index: usize,
    /// Where the row's first cell sits in the line.
    start: usize,
    /// How many of the row's leading cells the line takes.
    taken: usize,
}

/// One row a logical line is cut into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CutRow {
    /// Where the row starts in the line.
    start: usize,
    /// How many of the line's cells the row holds.
    count: usize,
    /// The pen of the filler left in the row's last column when the wide
    /// glyph that would start there opens the next row.
    filler: Option<Pen>,
}

/// Where a position lands in a cut line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placed {
    /// On the cut's row `row`, at `boundary`.
    At { row: usize, boundary: usize },
    /// Just past the last row, which is full.
    PastFullEnd,
}

impl Placed {
    /// Where `spot` lands among the rows of `cut`, each `width` wide: an
    /// edge position at the end of the row its preceding cell sits on, any
    /// other at the cell it precedes.
    fn of(cut: &[CutRow], spot: Spot, width: usize) -> Self {
        let last = cut.len().saturating_sub(1);
        if spot.at_edge {
            let row = cut
                .iter()
                .position(|row| spot.offset <= row.start + row.count)
                .unwrap_or(last);
            let start = cut.get(row).map_or(0, |row| row.start);
            return Self::At {
                row,
                boundary: spot.offset.saturating_sub(start),
            };
        }
        let row = cut
            .iter()
            .rposition(|row| row.start <= spot.offset)
            .unwrap_or(0);
        let start = cut.get(row).map_or(0, |row| row.start);
        let count = cut.get(row).map_or(0, |row| row.count);
        let boundary = spot.offset.saturating_sub(start);
        if row == last && boundary >= width && count >= width {
            return Self::PastFullEnd;
        }
        Self::At {
            row,
            boundary: boundary.min(width),
        }
    }
}

/// How the wide glyphs among a line's kept cells pair up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pairing {
    /// No kept cell is a wide glyph or a continuation column.
    Narrow,
    /// Every wide glyph is followed by its continuation column, and every
    /// continuation column follows its glyph.
    Intact,
    /// Some wide glyph or continuation column lacks its partner.
    Broken,
}

/// The buffers a rewrap reuses from one logical line to the next.
struct Rewrap {
    /// The width the run's rows have.
    old_cols: u16,
    /// The width the run is rewrapped at.
    cols: u16,
    /// The cursor as the run held it before any line was cut.
    cursor_from: Option<SlicePoint>,
    /// The positions as the run held them before any line was cut.
    points_from: Vec<Option<SlicePoint>>,
    /// The rows of the line being cut.
    line: Vec<GridRow>,
    /// Each of those rows' share of the line.
    shares: Vec<Share>,
    /// The line's kept cells, one buffer per row they came from.
    segments: Vec<Vec<Cell>>,
    /// The rows the line is cut into.
    cut: Vec<CutRow>,
    /// The cells of the rows the line is cut into, in the order of `cut`.
    built: Vec<Vec<Cell>>,
    /// The positions that sit on the line, by their index.
    spots: Vec<(usize, Spot)>,
    /// Cell buffers a cut takes before it allocates.
    spare: Vec<Vec<Cell>>,
}

impl Rewrap {
    /// Builds empty buffers for rewrapping rows `old_cols` wide at `cols`,
    /// with `cursor_from` and `points_from` the positions as the run holds
    /// them before any line is cut.
    fn new(
        old_cols: u16,
        cols: u16,
        cursor_from: Option<SlicePoint>,
        points_from: Vec<Option<SlicePoint>>,
    ) -> Self {
        Self {
            old_cols,
            cols,
            cursor_from,
            points_from,
            line: Vec::new(),
            shares: Vec::new(),
            segments: Vec::new(),
            cut: Vec::new(),
            built: Vec::new(),
            spots: Vec::new(),
            spare: Vec::new(),
        }
    }

    /// The first of the fewest newest lines of `rows` whose cuts are sure to
    /// make at least `needed` rows between them, as an index into
    /// `starts`, and for each line from the newest back to it how many
    /// cells the line keeps, when that was measured.
    ///
    /// A line counts as the fewest rows its kept cells can fill, and a line
    /// no wider than `self.cols` counts as one row without being measured.
    /// The first line is zero when all lines together fall short, and with
    /// `needed` of `usize::MAX` no line is counted.
    fn newest_lines(
        &mut self,
        rows: &[GridRow],
        starts: &[usize],
        needed: usize,
    ) -> (usize, Vec<Option<usize>>) {
        let mut keep_ends = Vec::new();
        if needed == usize::MAX {
            return (0, keep_ends);
        }
        let width = usize::from(self.cols).max(1);
        let mut made = 0;
        let mut end = rows.len();
        for (line, &start) in starts.iter().enumerate().rev() {
            if made >= needed {
                return (line + 1, keep_ends);
            }
            let Some(line_rows) = rows.get(start..end) else {
                break;
            };
            let total = share_out(&mut self.shares, line_rows, start);
            let kept = (total > width).then(|| {
                let ends = line_rows.last().is_none_or(|row| row.wrap_at.is_none());
                let cursor_spot = self
                    .cursor_from
                    .and_then(|point| Spot::on(&self.shares, point, self.old_cols));
                let text_end = if ends {
                    let rows = line_rows.iter().zip(&self.shares).map(|(row, share)| {
                        (row.cells.get(..share.taken).unwrap_or_default(), share)
                    });
                    text_end(rows, total)
                } else {
                    total
                };
                kept_len(total, ends, text_end, cursor_spot)
            });
            made += kept.map_or(1, |kept| kept.div_ceil(width).max(1));
            keep_ends.push(kept);
            end = start;
        }
        (0, keep_ends)
    }

    /// Cuts the line held in `self.line`, whose first row is the run's row
    /// `first`, at `self.cols` onto the end of `out`, and moves `cursor`
    /// and each of `points` that sat on the line.
    ///
    /// Each position is looked up by the row it stood on before any line
    /// was cut. `known_keep_end` is how many cells the line keeps, when
    /// [`Rewrap::newest_lines`] already measured it.
    fn cut_line(
        &mut self,
        out: &mut Vec<GridRow>,
        mint: &mut impl FnMut() -> LineId,
        cursor: Option<&mut SlicePoint>,
        points: &mut [Option<SlicePoint>],
        first: usize,
        known_keep_end: Option<usize>,
    ) {
        let Some(id) = self.line.first().map(|row| row.id) else {
            return;
        };
        let total = share_out(&mut self.shares, &self.line, first);
        let ends = self.line.last().is_none_or(|row| row.wrap_at.is_none());
        let cursor_spot = self
            .cursor_from
            .and_then(|point| Spot::on(&self.shares, point, self.old_cols));
        self.find_spots();
        self.gather();
        let first_special = self.first_special();
        let keep_end = known_keep_end.unwrap_or_else(|| {
            let text_end = if ends {
                text_end(
                    self.segments.iter().map(Vec::as_slice).zip(&self.shares),
                    total,
                )
            } else {
                total
            };
            kept_len(total, ends, text_end, cursor_spot)
        });
        let fill = self.fill(keep_end, total);
        self.keep(keep_end);
        self.cut(keep_end, first_special);
        let extra = self.place(cursor, points, cursor_spot, keep_end, out.len());
        self.emit(out, mint, id, &fill, ends, extra);
    }

    /// Records in `self.spots` where each position that sat on the line
    /// falls in it.
    fn find_spots(&mut self) {
        self.spots.clear();
        for (index, point) in self.points_from.iter().enumerate() {
            if let Some(spot) = point.and_then(|point| Spot::on(&self.shares, point, self.old_cols))
            {
                self.spots.push((index, spot));
            }
        }
    }

    /// Moves the cells of the rows in `self.line` into `self.segments`,
    /// each row's up to its share.
    fn gather(&mut self) {
        for (row, share) in self.line.drain(..).zip(&self.shares) {
            let mut cells = row.cells.into_inner();
            cells.truncate(share.taken);
            self.segments.push(cells);
        }
    }

    /// Lets go of the gathered cells past the line's first `keep_end`.
    fn keep(&mut self, keep_end: usize) {
        while self.segments.len() > 1
            && self
                .shares
                .get(self.segments.len() - 1)
                .is_some_and(|share| share.start >= keep_end)
        {
            if let Some(cells) = self.segments.pop() {
                recycle(&mut self.spare, cells, usize::from(self.cols));
            }
        }
        let start = self
            .shares
            .get(self.segments.len().saturating_sub(1))
            .map_or(0, |share| share.start);
        if let Some(cells) = self.segments.last_mut() {
            cells.truncate(keep_end.saturating_sub(start));
        }
    }

    /// Where the first gathered cell other than a plain narrow one sits in
    /// the line.
    fn first_special(&self) -> Option<usize> {
        self.segments
            .iter()
            .zip(&self.shares)
            .find_map(|(cells, share)| {
                cells
                    .iter()
                    .position(|cell| cell.width != CellWidth::Narrow)
                    .map(|index| share.start + index)
            })
    }

    /// The blank a cut fills out the line's last row with: one in the
    /// colors of the last of the line's `total` gathered cells when the cut
    /// keeps only `keep_end` of them, and a default blank otherwise.
    fn fill(&self, keep_end: usize, total: usize) -> Cell {
        if keep_end >= total {
            return Cell::default();
        }
        self.segments
            .iter()
            .rev()
            .find_map(|cells| cells.last())
            .map_or_else(Cell::default, |last| {
                Pen {
                    fg: last.fg,
                    bg: last.bg,
                    style: last.style,
                }
                .erase_cell()
            })
    }

    /// Cuts the `keep_end` gathered cells into `self.cut` and `self.built`,
    /// where `first_special` is where the first cell other than a plain
    /// narrow one sits in the line.
    fn cut(&mut self, keep_end: usize, first_special: Option<usize>) {
        let pairing = if first_special.is_none_or(|first| first >= keep_end) {
            Pairing::Narrow
        } else {
            self.pair_up()
        };
        if pairing == Pairing::Broken || self.cols < 2 {
            self.cut_cell_by_cell();
        } else {
            self.cut_in_bulk(keep_end, pairing);
        }
    }

    /// Moves `cursor`, whose place in the line is `cursor_spot`, and each of
    /// `points` that sat on the line onto the rows of `self.cut`, the first
    /// of which lands at index `base`; reports whether the cursor sits just
    /// past a full last row and needs a fresh row.
    ///
    /// A position other than the cursor past the line's first `keep_end`
    /// cells moves to the end of those cells.
    fn place(
        &self,
        cursor: Option<&mut SlicePoint>,
        points: &mut [Option<SlicePoint>],
        cursor_spot: Option<Spot>,
        keep_end: usize,
        base: usize,
    ) -> bool {
        let width = usize::from(self.cols);
        let last = self.cut.len().saturating_sub(1);
        let cols = self.cols;
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
        let cursor_placed = cursor_spot.map(|spot| Placed::of(&self.cut, spot, width));
        if let (Some(point), Some(placed)) = (cursor, cursor_placed) {
            *point = match placed {
                Placed::PastFullEnd => SlicePoint {
                    row: base + last + 1,
                    boundary: 0,
                },
                placed => to_point(placed),
            };
        }
        for &(index, spot) in &self.spots {
            let clamped = Spot {
                offset: spot.offset.min(keep_end),
                ..spot
            };
            if let Some(slot) = points.get_mut(index) {
                *slot = Some(to_point(Placed::of(&self.cut, clamped, width)));
            }
        }
        matches!(cursor_placed, Some(Placed::PastFullEnd))
    }

    /// Pushes the rows of `self.built` onto `out`, each filled out to
    /// `self.cols` with default blanks and the last with `fill`, and a fresh
    /// blank row after them when `extra` is set.
    ///
    /// The first row keeps `id`, and `mint` names the rest. Every row but
    /// the last records how many of the line's cells it holds as its wrap,
    /// and so does the last when `extra` is set or the line does not end,
    /// as `ends` says.
    fn emit(
        &mut self,
        out: &mut Vec<GridRow>,
        mint: &mut impl FnMut() -> LineId,
        id: LineId,
        fill: &Cell,
        ends: bool,
        extra: bool,
    ) {
        let width = usize::from(self.cols);
        let last = self.cut.len().saturating_sub(1);
        for (k, (mut row, cut_row)) in self.built.drain(..).zip(&self.cut).enumerate() {
            let pad = if k == last {
                fill.clone()
            } else {
                Cell::default()
            };
            row.reserve_exact(width.saturating_sub(row.len()));
            row.resize(width, pad);
            let wrap_at = (k < last || extra || !ends)
                .then(|| u16::try_from(cut_row.count).unwrap_or(self.cols));
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
                cells: Row::filled(self.cols, Cell::default()),
                wrap_at: None,
            });
        }
        self.cut.clear();
    }

    /// Turns each filler among the gathered cells into a plain blank and
    /// reports how their wide glyphs pair up.
    fn pair_up(&mut self) -> Pairing {
        let mut wide = false;
        let mut open = false;
        let mut broken = false;
        for cell in self.segments.iter_mut().flatten() {
            match cell.width {
                CellWidth::Narrow => {
                    broken |= open;
                    open = false;
                }
                CellWidth::LeadingSpacer => {
                    cell.width = CellWidth::Narrow;
                    broken |= open;
                    open = false;
                }
                CellWidth::Wide => {
                    broken |= open;
                    open = true;
                    wide = true;
                }
                CellWidth::Spacer => {
                    broken |= !open;
                    open = false;
                    wide = true;
                }
            }
        }
        if broken || open {
            Pairing::Broken
        } else if wide {
            Pairing::Intact
        } else {
            Pairing::Narrow
        }
    }

    /// Cuts the gathered `keep_end` cells, whose wide glyphs pair up as
    /// `pairing` says, into `self.cut` and `self.built`.
    ///
    /// A wide glyph that would start in the last column leaves a filler
    /// there and opens the next row. `pairing` must not be
    /// [`Pairing::Broken`], and `self.cols` must be at least two.
    fn cut_in_bulk(&mut self, keep_end: usize, pairing: Pairing) {
        let width = usize::from(self.cols);
        let mut start = 0;
        while keep_end - start > width {
            let edge = start + width - 1;
            let wide_at_edge = if pairing == Pairing::Intact {
                self.cell_at(edge)
                    .filter(|cell| cell.width == CellWidth::Wide)
                    .map(|cell| Pen {
                        fg: cell.fg,
                        bg: cell.bg,
                        style: cell.style,
                    })
            } else {
                None
            };
            if wide_at_edge.is_some() {
                self.cut.push(CutRow {
                    start,
                    count: width - 1,
                    filler: wide_at_edge,
                });
                start = edge;
            } else {
                self.cut.push(CutRow {
                    start,
                    count: width,
                    filler: None,
                });
                start += width;
            }
        }
        self.cut.push(CutRow {
            start,
            count: keep_end - start,
            filler: None,
        });
        for k in (0..self.cut.len()).rev() {
            let Some(&CutRow { start, filler, .. }) = self.cut.get(k) else {
                continue;
            };
            let mut row = self.take_tail(start);
            if let Some(pen) = filler {
                row.push(pen.filler());
            }
            self.built.push(row);
        }
        self.built.reverse();
        for cells in self.segments.drain(..) {
            recycle(&mut self.spare, cells, width);
        }
    }

    /// Cuts the gathered cells into `self.cut` and `self.built`, giving a
    /// wide glyph without its continuation column a fresh one and keeping
    /// a continuation column without its glyph as a cell of its own. A
    /// wide glyph that would start in the last column leaves a filler
    /// there and opens the next row.
    fn cut_cell_by_cell(&mut self) {
        let mut cells = self.segments.first_mut().map(mem::take).unwrap_or_default();
        let width = usize::from(self.cols);
        for mut later in self.segments.drain(1..) {
            cells.append(&mut later);
            recycle(&mut self.spare, later, width);
        }
        self.segments.clear();
        if cells.len() <= width {
            let count = cells.len();
            self.cut.push(CutRow {
                start: 0,
                count,
                filler: None,
            });
            self.built.push(cells);
            return;
        }
        let mut row: Vec<Cell> = Vec::with_capacity(width);
        let mut start = 0;
        let mut offset = 0;
        let mut cells = cells.into_iter().peekable();
        while let Some(cell) = cells.next() {
            let span = if cell.width == CellWidth::Wide { 2 } else { 1 };
            if row.len() + span > width {
                let count = row.len();
                let filler = (span == 2 && row.len() + 1 == width).then_some(Pen {
                    fg: cell.fg,
                    bg: cell.bg,
                    style: cell.style,
                });
                if let Some(pen) = filler {
                    row.push(pen.filler());
                }
                self.cut.push(CutRow {
                    start,
                    count,
                    filler,
                });
                self.built
                    .push(mem::replace(&mut row, Vec::with_capacity(width)));
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
        self.cut.push(CutRow {
            start,
            count,
            filler: None,
        });
        self.built.push(row);
    }

    /// The gathered cell `offset` cells into the line.
    fn cell_at(&self, offset: usize) -> Option<&Cell> {
        let segment = self.shares[..self.segments.len().min(self.shares.len())]
            .iter()
            .rposition(|share| share.start <= offset)?;
        let start = self.shares.get(segment)?.start;
        self.segments.get(segment)?.get(offset - start)
    }

    /// Takes the gathered cells from `start` cells into the line to its
    /// end, as one buffer with room for a whole row.
    fn take_tail(&mut self, start: usize) -> Vec<Cell> {
        let width = usize::from(self.cols);
        let held = self.segments.len().min(self.shares.len());
        let segment = self.shares[..held]
            .iter()
            .rposition(|share| share.start <= start)
            .unwrap_or(0);
        let offset = start.saturating_sub(self.shares.get(segment).map_or(0, |share| share.start));
        let mut row = if offset == 0 {
            self.segments
                .get_mut(segment)
                .map(mem::take)
                .unwrap_or_default()
        } else {
            let mut row = self.spare_buffer();
            if let Some(cells) = self.segments.get_mut(segment) {
                row.extend(cells.drain(offset.min(cells.len())..));
            }
            row
        };
        if row.capacity() < width {
            let mut grown = Vec::with_capacity(width);
            grown.append(&mut row);
            row = grown;
        }
        for mut later in self.segments.drain((segment + 1).min(held)..) {
            row.append(&mut later);
            recycle(&mut self.spare, later, width);
        }
        if offset == 0 {
            self.segments.truncate(segment);
        }
        row
    }

    /// An empty buffer with room for a row `self.cols` wide, reusing a
    /// spare one when it has the room.
    fn spare_buffer(&mut self) -> Vec<Cell> {
        let width = usize::from(self.cols);
        match self.spare.pop() {
            Some(mut cells) if cells.capacity() >= width => {
                cells.clear();
                cells
            }
            _ => Vec::with_capacity(width),
        }
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
    cursor: Option<&mut SlicePoint>,
    points: &mut [Option<SlicePoint>],
    mint: &mut impl FnMut() -> LineId,
    rows: Vec<GridRow>,
    old_cols: u16,
    cols: u16,
) -> Vec<GridRow> {
    rewrap_newest(cursor, points, mint, rows, old_cols, cols, usize::MAX)
}

/// Rewraps the newest lines of `rows`, each `old_cols` wide, at `cols`,
/// and drops the older lines whole; returns the rows made, oldest first.
///
/// The lines kept are the fewest newest ones whose cut is sure to make at
/// least `needed` rows; the cut may make more. Every line is kept when
/// they all fall short, and when `needed` is `usize::MAX`. Rows joined by
/// `wrap_at` form one logical line, cut afresh at `cols`. A line that ends
/// loses its trailing blanks, judged by text, but never those before the
/// cursor, and its last row is filled out with the colors of the last
/// blank it lost. A line the run cuts off keeps every cell. The first row
/// of each kept line keeps its id; `mint` names every row the cut adds.
/// Each position moves exactly once, from the row it stood on before the
/// rewrap, and each of `points` on a dropped row becomes `None`.
fn rewrap_newest(
    mut cursor: Option<&mut SlicePoint>,
    points: &mut [Option<SlicePoint>],
    mint: &mut impl FnMut() -> LineId,
    rows: Vec<GridRow>,
    old_cols: u16,
    cols: u16,
    needed: usize,
) -> Vec<GridRow> {
    let starts = line_starts(&rows);
    let mut rewrap = Rewrap::new(old_cols, cols, cursor.as_deref().copied(), points.to_vec());
    let (kept_from, mut keep_ends) = rewrap.newest_lines(&rows, &starts, needed);
    let row_count = rows.len();
    let dropped = starts.get(kept_from).copied().unwrap_or(row_count);
    for point in points.iter_mut() {
        if point.is_some_and(|point| point.row < dropped) {
            *point = None;
        }
    }
    let mut source = rows.into_iter();
    for row in source.by_ref().take(dropped) {
        recycle(&mut rewrap.spare, row.cells.into_inner(), usize::from(cols));
    }
    let mut out = Vec::with_capacity(row_count - dropped);
    for (line, &start) in starts.iter().enumerate().skip(kept_from) {
        let end = starts.get(line + 1).copied().unwrap_or(row_count);
        rewrap.line.extend(source.by_ref().take(end - start));
        let known_keep_end = keep_ends.pop().flatten();
        rewrap.cut_line(
            &mut out,
            mint,
            cursor.as_deref_mut(),
            points,
            start,
            known_keep_end,
        );
    }
    out
}

/// Keeps `cells` in `spare` for a later cut when it has room for a row
/// `width` wide, and lets it go at once otherwise.
fn recycle(spare: &mut Vec<Vec<Cell>>, cells: Vec<Cell>, width: usize) {
    if cells.capacity() >= width {
        spare.push(cells);
    }
}

/// The index of each logical line's first row in `rows`, in order.
fn line_starts(rows: &[GridRow]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut continues = false;
    for (index, row) in rows.iter().enumerate() {
        if !continues {
            starts.push(index);
        }
        continues = row.wrap_at.is_some();
    }
    starts
}

/// Records in `shares` each of `line`'s rows' share of the line, the
/// first row being the run's row `first`, and returns how many cells the
/// line takes in all.
fn share_out(shares: &mut Vec<Share>, line: &[GridRow], first: usize) -> usize {
    shares.clear();
    let mut start = 0;
    for (k, row) in line.iter().enumerate() {
        let taken = taken(row);
        shares.push(Share {
            index: first + k,
            start,
            taken,
        });
        start += taken;
    }
    start
}

/// How many of `row`'s leading cells its logical line takes: those up to
/// its recorded wrap, never a wide glyph without its continuation column.
fn taken(row: &GridRow) -> usize {
    let len = row.cells.len();
    let recorded = row.wrap_at.map_or(len, |cells| usize::from(cells).min(len));
    let splits_a_pair = recorded > 0
        && recorded < len
        && row
            .cells
            .get(recorded - 1)
            .is_some_and(|cell| cell.width == CellWidth::Wide);
    if splits_a_pair {
        recorded + 1
    } else {
        recorded
    }
}

/// How many of a line's `total` cells a cut keeps: all of them when the
/// line does not end, and otherwise those up to `text_end`, the end of its
/// last text, or the cursor at `cursor`, whichever is further.
fn kept_len(total: usize, ends: bool, text_end: usize, cursor: Option<Spot>) -> usize {
    if !ends {
        return total;
    }
    text_end
        .max(cursor.map_or(0, |spot| spot.offset))
        .min(total)
}

/// The length of a line of `total` cells, given as each row's cells with
/// that row's share of the line, up to and including its last cell that
/// shows text, a wide glyph's continuation included.
fn text_end<'a>(
    rows: impl DoubleEndedIterator<Item = (&'a [Cell], &'a Share)>,
    total: usize,
) -> usize {
    for (cells, share) in rows.rev() {
        if let Some(last) = cells.iter().rposition(|cell| !is_blank(cell)) {
            let span = if cells[last].width == CellWidth::Wide {
                2
            } else {
                1
            };
            return (share.start + last + span).min(total);
        }
    }
    0
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::color::Color;
    use crate::screen::cell::{BodyWidth, GlyphClass};
    use proptest::prelude::*;
    use proptest::sample::Index;
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

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

    /// Asserts that rewrapping only the newest lines drops the older lines
    /// whole once the newer ones make the rows asked for, and loses the
    /// positions on the dropped rows.
    ///
    /// Case: the user narrows a window whose scrollback is at its cap, so
    /// the oldest lines would fall to the cap anyway.
    #[test]
    fn rewrapping_the_newest_lines_drops_the_older_ones_whole() {
        let rows = vec![
            row_of("abcd", 4, None, 1),
            row_of("efgh", 4, None, 2),
            row_of("ij", 4, None, 3),
        ];
        let mut points = [Some(at(0, 1)), Some(at(1, 3))];
        let out = rewrap_newest(None, &mut points, &mut minter(), rows, 4, 2, 3);
        assert_eq!(texts(&out), ["ef", "gh", "ij"]);
        assert_eq!(out[0].id, LineId(2));
        assert_eq!(points, [None, Some(at(1, 1))]);
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

    fn grid(cols: u16, rows: u16, max_history: usize) -> Grid {
        Grid::new(GridSize { cols, rows }, max_history)
    }

    /// Writes `text` from column zero of `line`, spilling onto the rows
    /// below and recording each wrap as autowrap would.
    fn write(grid: &mut Grid, line: u16, text: &str) {
        let cols = usize::from(grid.size().cols);
        let chars: Vec<char> = text.chars().collect();
        let chunks: Vec<&[char]> = chars.chunks(cols.max(1)).collect();
        for (k, chunk) in chunks.iter().enumerate() {
            let row = line + u16::try_from(k).expect("a small test");
            for (column, c) in (0u16..).zip(chunk.iter()) {
                grid[ScreenLine(row)][column].c = *c;
            }
            if k + 1 < chunks.len() {
                grid.set_wrap_at(GridLine::from(ScreenLine(row)), grid.size().cols);
            }
        }
    }

    /// Every row of the ring, oldest first, trimmed.
    fn all_rows(grid: &Grid) -> Vec<String> {
        let history = i32::try_from(grid.history_len()).expect("a small history");
        (-history..i32::from(grid.size().rows))
            .map(|line| {
                grid.row(GridLine(line))
                    .iter()
                    .flat_map(Cell::chars)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn cursor_at(line: i32, boundary: u16) -> TrackedPoint {
        TrackedPoint {
            line: GridLine(line),
            boundary,
        }
    }

    fn reflow(
        grid: &mut Grid,
        cursor: &mut TrackedPoint,
        cols: u16,
        rows: u16,
        policy: ScrollbackOnGrow,
    ) {
        let mut saved = cursor_at(0, 0);
        grid.reflow(cursor, &mut saved, &mut [], GridSize { cols, rows }, policy);
    }

    /// Asserts that under `Keep` a narrowing moves the prompt down into
    /// blank rows without pushing anything into history, and widening
    /// back restores the rows and the cursor.
    ///
    /// Case: the prompt sits near the top of a fresh window, the user
    /// narrows it until the prompt wraps, then widens it back.
    #[test]
    fn a_prompt_near_the_top_round_trips_without_touching_history() {
        let mut grid = grid(8, 6, 10);
        write(&mut grid, 0, "abcdefgh");
        write(&mut grid, 2, "PS>");
        let mut cursor = cursor_at(2, 4);
        reflow(&mut grid, &mut cursor, 4, 6, ScrollbackOnGrow::Keep);
        assert_eq!(all_rows(&grid), ["abcd", "efgh", "", "PS>", "", ""]);
        assert_eq!(cursor, cursor_at(4, 0));
        reflow(&mut grid, &mut cursor, 8, 6, ScrollbackOnGrow::Keep);
        assert_eq!(all_rows(&grid), ["abcdefgh", "", "PS>", "", "", ""]);
        assert_eq!(cursor, cursor_at(2, 4));
        grid.assert_history_index_matches_ring();
    }

    /// Asserts that under `Keep` a narrowing with the prompt on the bottom
    /// row pushes only the overflow into history, and a widening leaves
    /// that history in place with blank rows at the bottom.
    ///
    /// Case: the screen is full of output with the prompt on the last row
    /// when the user narrows the window and widens it back under ConPTY.
    #[test]
    fn keep_pushes_only_the_overflow_and_never_pulls_it_back() {
        let mut grid = grid(8, 3, 10);
        write(&mut grid, 0, "line1");
        write(&mut grid, 1, "line2");
        write(&mut grid, 2, "PS>");
        let mut cursor = cursor_at(2, 4);
        reflow(&mut grid, &mut cursor, 4, 3, ScrollbackOnGrow::Keep);
        assert_eq!(all_rows(&grid), ["line", "1", "line", "2", "PS>", ""]);
        assert_eq!(cursor, cursor_at(2, 0));
        reflow(&mut grid, &mut cursor, 8, 3, ScrollbackOnGrow::Keep);
        assert_eq!(all_rows(&grid), ["line1", "line", "2", "PS>", ""]);
        assert_eq!(cursor, cursor_at(1, 4));
        grid.assert_history_index_matches_ring();
    }

    /// Asserts that the halves of a line split between history and the
    /// screen join without a gap once both sit in history.
    ///
    /// Case: after a narrow-then-wide resize under ConPTY, more output
    /// scrolls the rest of the split line into history and the user
    /// widens the window again.
    #[test]
    fn a_split_line_joins_without_a_gap_once_both_halves_are_in_history() {
        let mut grid = grid(8, 3, 10);
        write(&mut grid, 0, "line1");
        write(&mut grid, 1, "line2");
        write(&mut grid, 2, "PS>");
        let mut cursor = cursor_at(2, 4);
        reflow(&mut grid, &mut cursor, 4, 3, ScrollbackOnGrow::Keep);
        reflow(&mut grid, &mut cursor, 8, 3, ScrollbackOnGrow::Keep);
        for _ in 0..3 {
            grid.scroll_up_one(ScreenLine(0), ScreenLine(2), Cell::default());
        }
        reflow(&mut grid, &mut cursor, 10, 3, ScrollbackOnGrow::Keep);
        assert!(all_rows(&grid).contains(&"line2".to_string()));
    }

    /// Asserts that under `Reclaim` narrowing and widening back restores
    /// the rows and the cursor, rejoining the line the narrowing split
    /// between history and the screen.
    ///
    /// Case: on macOS the user narrows a full window and widens it back.
    #[test]
    fn reclaim_restores_a_line_split_across_the_boundary() {
        let mut grid = grid(8, 3, 10);
        write(&mut grid, 0, "line1");
        write(&mut grid, 1, "line2");
        write(&mut grid, 2, "PS>");
        let mut cursor = cursor_at(2, 4);
        reflow(&mut grid, &mut cursor, 4, 3, ScrollbackOnGrow::Reclaim);
        reflow(&mut grid, &mut cursor, 8, 3, ScrollbackOnGrow::Reclaim);
        assert_eq!(all_rows(&grid), ["line1", "line2", "PS>"]);
        assert_eq!(cursor, cursor_at(2, 4));
        grid.assert_history_index_matches_ring();
    }

    /// Asserts that under `Reclaim` a widening that joins a history row
    /// onto the screen's top line pulls no other row back from history.
    ///
    /// Case: the newest history row wraps onto the top row of the screen,
    /// above the prompt, when the user widens the window on macOS.
    #[test]
    fn a_widening_that_joins_history_pulls_no_other_row_back() {
        let mut grid = grid(4, 3, 10);
        write(&mut grid, 0, "zz");
        write(&mut grid, 1, "abcdef");
        for _ in 0..2 {
            grid.scroll_up_one(ScreenLine(0), ScreenLine(2), Cell::default());
        }
        write(&mut grid, 1, "PS>");
        assert_eq!(all_rows(&grid), ["zz", "abcd", "ef", "PS>", ""]);
        assert_eq!(grid.wrap_at(GridLine(-1)), Some(4));
        let mut cursor = cursor_at(1, 3);
        reflow(&mut grid, &mut cursor, 8, 3, ScrollbackOnGrow::Reclaim);
        assert_eq!(all_rows(&grid), ["zz", "abcdef", "PS>", ""]);
        assert_eq!(grid.history_len(), 1);
        assert_eq!(cursor, cursor_at(1, 3));
        grid.assert_history_index_matches_ring();
    }

    /// Asserts that under `Reclaim` a narrowing that joins a history row
    /// onto the screen's top line keeps the old top row on top and returns
    /// the rest of that line to history, still continuing onto the screen.
    ///
    /// Case: the newest history row wraps onto the top row of the screen,
    /// above the prompt, when the user narrows the window on macOS.
    #[test]
    fn a_narrowing_that_joins_history_keeps_the_old_top_row_on_top() {
        let mut grid = grid(8, 3, 10);
        write(&mut grid, 0, "h1");
        write(&mut grid, 1, "abcdefghij");
        for _ in 0..2 {
            grid.scroll_up_one(ScreenLine(0), ScreenLine(2), Cell::default());
        }
        write(&mut grid, 1, "PS>");
        assert_eq!(all_rows(&grid), ["h1", "abcdefgh", "ij", "PS>", ""]);
        assert_eq!(grid.wrap_at(GridLine(-1)), Some(8));
        let mut cursor = cursor_at(1, 3);
        reflow(&mut grid, &mut cursor, 4, 3, ScrollbackOnGrow::Reclaim);
        assert_eq!(all_rows(&grid), ["h1", "abcd", "efgh", "ij", "PS>", ""]);
        assert_eq!(grid.history_len(), 3);
        assert_eq!(grid.wrap_at(GridLine(-1)), Some(4));
        assert_eq!(cursor, cursor_at(1, 3));
        grid.assert_history_index_matches_ring();
    }

    /// Asserts that a height-only growth under `Reclaim` gives the result
    /// the truncating resize gives, and under `Keep` adds blank rows.
    ///
    /// Case: the user drags the window taller after output scrolled off
    /// the top.
    #[test]
    fn a_height_only_growth_matches_the_policy() {
        let mut reclaim = grid(4, 2, 10);
        write(&mut reclaim, 0, "a");
        write(&mut reclaim, 1, "b");
        reclaim.scroll_up_one(ScreenLine(0), ScreenLine(1), Cell::default());
        let mut keep = grid(4, 2, 10);
        write(&mut keep, 0, "a");
        write(&mut keep, 1, "b");
        keep.scroll_up_one(ScreenLine(0), ScreenLine(1), Cell::default());
        let mut cursor = cursor_at(1, 0);
        reflow(&mut reclaim, &mut cursor, 4, 3, ScrollbackOnGrow::Reclaim);
        assert_eq!(reclaim.history_len(), 0);
        assert_eq!(cursor, cursor_at(2, 0));
        let mut cursor = cursor_at(1, 0);
        reflow(&mut keep, &mut cursor, 4, 3, ScrollbackOnGrow::Keep);
        assert_eq!(all_rows(&keep), ["a", "b", "", ""]);
        assert_eq!(keep.history_len(), 1);
        assert_eq!(cursor, cursor_at(1, 0));
    }

    /// Asserts that a height shrink with text below the cursor pushes rows
    /// into history as far as the cursor allows and drops the rest.
    ///
    /// Case: a program left text below the prompt and the user drags the
    /// window shorter.
    #[test]
    fn a_shrink_with_text_below_the_cursor_pushes_up_to_the_cursor() {
        let mut grid = grid(4, 4, 10);
        for (line, text) in [(0, "a"), (1, "b"), (2, "c"), (3, "d")] {
            write(&mut grid, line, text);
        }
        let mut cursor = cursor_at(1, 0);
        reflow(&mut grid, &mut cursor, 4, 2, ScrollbackOnGrow::Keep);
        assert_eq!(all_rows(&grid), ["a", "b", "c"]);
        assert_eq!(cursor, cursor_at(0, 0));
    }

    /// Asserts that rows past the history cap are dropped and a position on
    /// them is lost, while the saved cursor lands on the top row.
    ///
    /// Case: a short-scrollback terminal holds a selection and a saved
    /// cursor on its top rows when the user narrows it.
    #[test]
    fn the_history_cap_drops_rows_and_their_positions() {
        let mut grid = grid(4, 2, 1);
        write(&mut grid, 0, "abcd");
        write(&mut grid, 1, "efgh");
        let mut cursor = cursor_at(1, 4);
        let mut saved = cursor_at(0, 0);
        let mut points = [Some(cursor_at(0, 0))];
        grid.reflow(
            &mut cursor,
            &mut saved,
            &mut points,
            GridSize { cols: 2, rows: 2 },
            ScrollbackOnGrow::Keep,
        );
        assert_eq!(grid.history_len(), 1);
        assert_eq!(points, [None]);
        assert_eq!(saved.line, GridLine(0));
        grid.assert_history_index_matches_ring();
    }

    /// Asserts that a grid without scrollback drops what it pushes and
    /// keeps the cursor on the screen.
    ///
    /// Case: the user turned scrollback off and narrows the window under
    /// a full screen.
    #[test]
    fn a_grid_without_history_drops_what_it_pushes() {
        let mut grid = grid(4, 2, 0);
        write(&mut grid, 0, "abcd");
        write(&mut grid, 1, "efgh");
        let mut cursor = cursor_at(1, 4);
        reflow(&mut grid, &mut cursor, 2, 2, ScrollbackOnGrow::Keep);
        assert_eq!(grid.history_len(), 0);
        assert_eq!(all_rows(&grid), ["ef", "gh"]);
        assert_eq!(cursor, cursor_at(1, 2));
    }

    /// Asserts that a narrowing whose rewrapped history outgrows the cap
    /// keeps only the newest rows, splitting the line the cap cuts through,
    /// and loses the positions on the rows it drops.
    ///
    /// Case: a terminal whose scrollback is at its cap holds a selection in
    /// its oldest lines when the user narrows the window.
    #[test]
    fn a_narrowing_past_the_cap_keeps_only_the_newest_rows() {
        let mut grid = grid(4, 2, 3);
        for text in ["aaaa", "bbbb", "cccc", "dddd"] {
            write(&mut grid, 1, text);
            grid.scroll_up_one(ScreenLine(0), ScreenLine(1), Cell::default());
        }
        write(&mut grid, 1, "PS>");
        assert_eq!(all_rows(&grid), ["aaaa", "bbbb", "cccc", "dddd", "PS>"]);
        let mut cursor = cursor_at(1, 3);
        let mut saved = cursor_at(0, 0);
        let mut points = [
            Some(cursor_at(-2, 1)),
            Some(cursor_at(-1, 1)),
            Some(cursor_at(-1, 3)),
            Some(cursor_at(0, 2)),
        ];
        grid.reflow(
            &mut cursor,
            &mut saved,
            &mut points,
            GridSize { cols: 2, rows: 2 },
            ScrollbackOnGrow::Keep,
        );
        assert_eq!(all_rows(&grid), ["cc", "dd", "dd", "PS", ">"]);
        assert_eq!(cursor, cursor_at(1, 1));
        assert_eq!(
            points,
            [None, None, Some(cursor_at(-3, 1)), Some(cursor_at(-1, 0))]
        );
        grid.assert_history_index_matches_ring();
    }

    /// Asserts that a passive position on a blank row below the text keeps
    /// its distance from the last reflowed row.
    ///
    /// Case: a webview is anchored to a blank row under the prompt when the
    /// user widens the window.
    #[test]
    fn a_position_below_the_text_keeps_its_distance() {
        let mut grid = grid(4, 4, 10);
        write(&mut grid, 0, "abcdef");
        let mut cursor = cursor_at(1, 2);
        let mut saved = cursor_at(0, 0);
        let mut points = [Some(cursor_at(3, 1))];
        grid.reflow(
            &mut cursor,
            &mut saved,
            &mut points,
            GridSize { cols: 8, rows: 4 },
            ScrollbackOnGrow::Keep,
        );
        assert_eq!(cursor, cursor_at(0, 6));
        assert_eq!(points, [Some(cursor_at(2, 1))]);
    }

    /// One piece of a generated row.
    #[derive(Debug, Clone)]
    enum Piece {
        Letter(char),
        Wide,
        Blank,
        Erased(u8),
    }

    /// How a generated row's logical line goes on past it.
    #[derive(Debug, Clone, Copy)]
    enum GoesOn {
        No,
        Full,
        At(u16),
    }

    /// A generated run of rows `cols` wide; `broken` names a cell to turn
    /// into a wide glyph's body, or its continuation when the flag is set,
    /// which may break a wide pair.
    #[derive(Debug, Clone)]
    struct Run {
        cols: u16,
        rows: Vec<(Vec<Piece>, GoesOn)>,
        broken: Option<(Index, Index, bool)>,
    }

    impl Run {
        /// The rows, ids counting up from zero, laid out as autowrap would
        /// lay them: a wide glyph that would start in the last column
        /// leaves a filler there, and a full row goes on after its filler.
        fn build(&self) -> Vec<GridRow> {
            let width = usize::from(self.cols);
            let wide = Pen::default().stamp('あ', BodyWidth::Wide, None);
            let mut rows: Vec<GridRow> = (0u64..)
                .zip(&self.rows)
                .map(|(id, (pieces, goes_on))| {
                    let mut cells: Vec<Cell> = Vec::with_capacity(width);
                    for piece in pieces {
                        let room = width - cells.len();
                        match piece {
                            Piece::Letter(c) if room > 0 => {
                                cells.push(Pen::default().stamp(*c, BodyWidth::Narrow, None));
                            }
                            Piece::Wide if room >= 2 => {
                                cells.push(wide.clone());
                                cells.push(wide.continuation());
                            }
                            Piece::Wide if room == 1 => cells.push(Pen::default().filler()),
                            Piece::Blank if room > 0 => cells.push(Cell::default()),
                            Piece::Erased(color) if room > 0 => cells.push(
                                Pen {
                                    bg: Color::Indexed(*color),
                                    ..Pen::default()
                                }
                                .erase_cell(),
                            ),
                            _ => {}
                        }
                    }
                    cells.resize(width, Cell::default());
                    let filled_out = cells
                        .last()
                        .is_some_and(|cell| cell.width == CellWidth::LeadingSpacer);
                    let wrap_at = match goes_on {
                        GoesOn::No => None,
                        GoesOn::Full if filled_out => Some(self.cols - 1),
                        GoesOn::Full => Some(self.cols),
                        GoesOn::At(count) => Some((*count).min(self.cols)),
                    };
                    GridRow {
                        id: LineId(id),
                        cells: Row::from(cells),
                        wrap_at,
                    }
                })
                .collect();
            if let Some((row, column, continuation)) = &self.broken {
                let index = row.index(rows.len());
                let column = u16::try_from(column.index(width)).expect("a narrow row");
                rows[index].cells[column] = if *continuation {
                    wide.continuation()
                } else {
                    wide.clone()
                };
            }
            rows
        }
    }

    fn piece() -> impl Strategy<Value = Piece> {
        prop_oneof![
            8 => prop::char::range('a', 'z').prop_map(Piece::Letter),
            3 => Just(Piece::Wide),
            3 => Just(Piece::Blank),
            1 => (1u8..4).prop_map(Piece::Erased),
        ]
    }

    fn goes_on() -> impl Strategy<Value = GoesOn> {
        prop_oneof![
            3 => Just(GoesOn::No),
            4 => Just(GoesOn::Full),
            1 => (0u16..=12).prop_map(GoesOn::At),
        ]
    }

    fn run() -> impl Strategy<Value = Run> {
        (
            2u16..=10,
            prop::collection::vec((prop::collection::vec(piece(), 0..=14), goes_on()), 1..=12),
            prop::option::weighted(0.25, (any::<Index>(), any::<Index>(), any::<bool>())),
        )
            .prop_map(|(cols, rows, broken)| Run { cols, rows, broken })
    }

    /// Positions on random rows of a run, each at a boundary to be clamped
    /// to the run's width.
    fn spots() -> impl Strategy<Value = Vec<Option<(Index, u16)>>> {
        prop::collection::vec(prop::option::of((any::<Index>(), 0u16..=12)), 0..4)
    }

    /// `spots` as positions inside `run`.
    fn slice_points(run: &Run, spots: &[Option<(Index, u16)>]) -> Vec<Option<SlicePoint>> {
        spots
            .iter()
            .map(|spot| {
                spot.map(|(row, boundary)| at(row.index(run.rows.len()), boundary.min(run.cols)))
            })
            .collect()
    }

    /// Each row as its id, its cells, and its recorded wrap.
    fn laid_out(rows: &[GridRow]) -> Vec<(LineId, Row<Cell>, Option<u16>)> {
        rows.iter()
            .map(|row| (row.id, row.cells.clone(), row.wrap_at))
            .collect()
    }

    /// Each row as its cells and recorded wrap, and its id when the id is
    /// one of the first `original` rather than minted.
    fn with_original_ids(
        rows: impl Iterator<Item = (LineId, Row<Cell>, Option<u16>)>,
        original: u64,
    ) -> Vec<(Option<LineId>, Row<Cell>, Option<u16>)> {
        rows.map(|(id, cells, wrap_at)| ((id.0 < original).then_some(id), cells, wrap_at))
            .collect()
    }

    /// Rewraps `rows` as [`rewrap`] does, but cuts every line cell by cell.
    fn rewrap_cell_by_cell(
        mut cursor: Option<&mut SlicePoint>,
        points: &mut [Option<SlicePoint>],
        mint: &mut impl FnMut() -> LineId,
        rows: Vec<GridRow>,
        old_cols: u16,
        cols: u16,
    ) -> Vec<GridRow> {
        let starts = line_starts(&rows);
        let mut rewrap = Rewrap::new(old_cols, cols, cursor.as_deref().copied(), points.to_vec());
        let row_count = rows.len();
        let mut source = rows.into_iter();
        let mut out = Vec::new();
        for (line, &start) in starts.iter().enumerate() {
            let end = starts.get(line + 1).copied().unwrap_or(row_count);
            rewrap.line.extend(source.by_ref().take(end - start));
            let Some(id) = rewrap.line.first().map(|row| row.id) else {
                continue;
            };
            let total = share_out(&mut rewrap.shares, &rewrap.line, start);
            let ends = rewrap.line.last().is_none_or(|row| row.wrap_at.is_none());
            let cursor_spot = rewrap
                .cursor_from
                .and_then(|point| Spot::on(&rewrap.shares, point, old_cols));
            rewrap.find_spots();
            rewrap.gather();
            let first_special = rewrap.first_special();
            let text_end = if ends {
                text_end(
                    rewrap
                        .segments
                        .iter()
                        .map(Vec::as_slice)
                        .zip(&rewrap.shares),
                    total,
                )
            } else {
                total
            };
            let keep_end = kept_len(total, ends, text_end, cursor_spot);
            let fill = rewrap.fill(keep_end, total);
            rewrap.keep(keep_end);
            if first_special.is_some_and(|first| first < keep_end) {
                let _ = rewrap.pair_up();
            }
            rewrap.cut_cell_by_cell();
            let extra = rewrap.place(
                cursor.as_deref_mut(),
                points,
                cursor_spot,
                keep_end,
                out.len(),
            );
            rewrap.emit(&mut out, mint, id, &fill, ends, extra);
        }
        out
    }

    /// A grid of `size` whose ring holds `rows`, the first `history` of
    /// them as history, with history capped at `cap`.
    fn seeded(rows: Vec<GridRow>, size: GridSize, history: usize, cap: usize) -> Grid {
        let mut grid = Grid::new(size, cap);
        grid.history_index
            .rebuild(rows.iter().take(history).map(|row| row.id));
        grid.next_line_id = u64::try_from(rows.len()).expect("a short run");
        grid.rows = VecDeque::from(rows);
        grid
    }

    /// Every row of the ring, oldest first, as its id, cells, and wrap.
    fn ring_of(grid: &Grid) -> Vec<(LineId, Row<Cell>, Option<u16>)> {
        grid.rows
            .iter()
            .map(|row| (row.id, row.cells.clone(), row.wrap_at))
            .collect()
    }

    /// Checks that no id repeats in `grid`'s ring and that its history
    /// index names exactly its history rows.
    fn check_ids(grid: &Grid) -> Result<(), TestCaseError> {
        let ids: HashSet<LineId> = grid.rows.iter().map(|row| row.id).collect();
        prop_assert_eq!(ids.len(), grid.rows.len(), "an id repeats in the ring");
        grid.assert_history_index_matches_ring();
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Asserts that cutting lines in bulk makes the rows, ids, wraps,
        /// cursor, and positions that cutting them cell by cell makes.
        ///
        /// Case: a scrollback of wrapped lines mixing ASCII, Japanese
        /// glyphs, fillers, erase colors, and now and then a broken wide
        /// pair is rewrapped at another width while the cursor and a
        /// selection sit on it.
        #[test]
        fn bulk_cuts_match_cell_by_cell_cuts(
            run in run(),
            cols in 2u16..=12,
            cursor in prop::option::of((any::<Index>(), 0u16..=12)),
            spots in spots(),
        ) {
            let cursor = slice_points(&run, &[cursor]).pop().flatten();
            let points = slice_points(&run, &spots);
            let (mut bulk_cursor, mut bulk_points) = (cursor, points.clone());
            let bulk = rewrap(
                bulk_cursor.as_mut(),
                &mut bulk_points,
                &mut minter(),
                run.build(),
                run.cols,
                cols,
            );
            let (mut slow_cursor, mut slow_points) = (cursor, points);
            let slow = rewrap_cell_by_cell(
                slow_cursor.as_mut(),
                &mut slow_points,
                &mut minter(),
                run.build(),
                run.cols,
                cols,
            );
            prop_assert_eq!(laid_out(&bulk), laid_out(&slow));
            prop_assert_eq!(bulk_cursor, slow_cursor);
            prop_assert_eq!(bulk_points, slow_points);
        }

        /// Asserts that rewrapping only the newest lines drops whole lines,
        /// makes at least the rows asked for, makes for the lines it keeps
        /// the rows and positions a full rewrap makes, and loses exactly the
        /// positions on the lines it drops.
        ///
        /// Case: a capped scrollback of wrapped lines is rewrapped at another
        /// width while a selection spans its oldest and newest lines.
        #[test]
        fn rewrapping_the_newest_lines_matches_a_full_rewrap(
            run in run(),
            cols in 2u16..=12,
            needed in 0usize..30,
            spots in spots(),
        ) {
            let points = slice_points(&run, &spots);
            let mut full_points = points.clone();
            let full = rewrap(None, &mut full_points, &mut minter(), run.build(), run.cols, cols);
            let mut newest_points = points;
            let newest = rewrap_newest(
                None,
                &mut newest_points,
                &mut minter(),
                run.build(),
                run.cols,
                cols,
                needed,
            );
            prop_assert!(newest.len() <= full.len());
            let dropped = full.len() - newest.len();
            prop_assert!(newest.len() >= needed.min(full.len()), "made {} of {}", newest.len(), needed);
            prop_assert!(
                dropped == 0 || dropped == full.len() || full[dropped - 1].wrap_at.is_none(),
                "cut a line apart"
            );
            let original = u64::try_from(run.rows.len()).expect("a short run");
            prop_assert_eq!(
                with_original_ids(laid_out(&newest).into_iter(), original),
                with_original_ids(laid_out(&full[dropped..]).into_iter(), original)
            );
            for (newest_point, full_point) in newest_points.iter().zip(&full_points) {
                let expected = full_point.and_then(|point| {
                    point.row.checked_sub(dropped).map(|row| SlicePoint { row, ..point })
                });
                prop_assert_eq!(*newest_point, expected);
            }
        }

        /// Asserts that a reflow under a small history cap leaves the newest
        /// history rows, the screen, the cursor, and the positions an
        /// uncapped reflow leaves, and loses exactly the positions on the
        /// rows past the cap.
        ///
        /// Case: a terminal whose scrollback holds from none to a few rows
        /// is resized to another width and height under either policy while
        /// the cursor, the saved cursor, and a selection sit in its rows.
        #[test]
        fn a_capped_reflow_keeps_the_newest_rows_an_uncapped_one_makes(
            run in run(),
            cap in prop_oneof![
                Just(0usize), Just(1usize), Just(2usize), Just(3usize), Just(5usize), Just(8usize)
            ],
            history in any::<Index>(),
            to in (2u16..=12, 1u16..=8),
            keep in any::<bool>(),
            cursor in (any::<Index>(), 0u16..=12),
            saved in (any::<Index>(), 0u16..=12),
            spots in spots(),
        ) {
            let policy = if keep { ScrollbackOnGrow::Keep } else { ScrollbackOnGrow::Reclaim };
            let total = run.rows.len();
            let history = history.index(cap.min(total - 1) + 1);
            let visible = u16::try_from(total - history).expect("a short run");
            let size = GridSize { cols: run.cols, rows: visible };
            let rows = || {
                let mut rows = run.build();
                if let Some(bottom) = rows.last_mut() {
                    bottom.wrap_at = None;
                }
                rows
            };
            let on_screen = |(row, boundary): (Index, u16)| {
                cursor_at(
                    i32::try_from(row.index(usize::from(visible))).expect("a short run"),
                    boundary.min(run.cols),
                )
            };
            let depth = i32::try_from(history).expect("a short run");
            let points: Vec<Option<TrackedPoint>> = spots
                .iter()
                .map(|spot| {
                    spot.map(|(row, boundary)| {
                        cursor_at(
                            i32::try_from(row.index(total)).expect("a short run") - depth,
                            boundary.min(run.cols),
                        )
                    })
                })
                .collect();
            let to = GridSize { cols: to.0, rows: to.1 };
            let mut capped = seeded(rows(), size, history, cap);
            let (mut capped_cursor, mut capped_saved) = (on_screen(cursor), on_screen(saved));
            let mut capped_points = points.clone();
            capped.reflow(&mut capped_cursor, &mut capped_saved, &mut capped_points, to, policy);
            let mut uncapped = seeded(rows(), size, history, usize::MAX);
            let (mut uncapped_cursor, mut uncapped_saved) = (on_screen(cursor), on_screen(saved));
            let mut uncapped_points = points;
            uncapped.reflow(&mut uncapped_cursor, &mut uncapped_saved, &mut uncapped_points, to, policy);

            check_ids(&capped)?;
            check_ids(&uncapped)?;
            let kept = capped.history_len();
            let all = uncapped.history_len();
            prop_assert_eq!(kept, all.min(cap));
            let original = u64::try_from(total).expect("a short run");
            prop_assert_eq!(
                with_original_ids(ring_of(&capped).into_iter(), original),
                with_original_ids(ring_of(&uncapped).into_iter().skip(all - kept), original)
            );
            prop_assert_eq!(capped_cursor, uncapped_cursor);
            prop_assert_eq!(capped_saved.line, uncapped_saved.line);
            if uncapped_saved.line != GridLine(0) {
                prop_assert_eq!(capped_saved, uncapped_saved);
            }
            let oldest_kept = -i32::try_from(kept).expect("a short history");
            for (capped_point, uncapped_point) in capped_points.iter().zip(&uncapped_points) {
                let expected = uncapped_point.filter(|point| point.line.0 >= oldest_kept);
                prop_assert_eq!(*capped_point, expected);
            }
        }
    }

    /// A grid `cols` wide and fifty rows tall whose ten thousand history
    /// rows and top screen rows hold lines of `text_len` characters, each
    /// wrapped over as many rows as it needs, with blank rows, the cursor's
    /// among them, at the bottom.
    fn full_scrollback(cols: u16, text_len: usize) -> Grid {
        let mut grid = grid(cols, 50, 10_000);
        let text: String = ('a'..='z').cycle().take(text_len).collect();
        let rows_per_line = text_len.div_ceil(usize::from(cols)).max(1);
        let first_row = 50 - u16::try_from(rows_per_line).expect("a line shorter than the screen");
        for _ in 0..10_050usize.div_ceil(rows_per_line) {
            write(&mut grid, first_row, &text);
            for _ in 0..rows_per_line {
                grid.scroll_up_one(ScreenLine(0), ScreenLine(49), Cell::default());
            }
        }
        grid
    }

    /// One timed resize of a [`full_scrollback`]: rows `from` columns wide
    /// holding lines of `text_len` characters, reflowed to `to` columns,
    /// after which history holds `history_after` rows. The median of a drag
    /// that is not `held_to_budget` is reported but not asserted.
    #[derive(Debug, Clone, Copy)]
    struct Drag {
        name: &'static str,
        from: u16,
        text_len: usize,
        to: u16,
        history_after: usize,
        held_to_budget: bool,
    }

    /// How long one reflow of a freshly filled [`full_scrollback`] takes
    /// for `drag`; the fill stays outside the timed region.
    fn time_one_full_reflow(drag: Drag) -> Duration {
        let mut grid = full_scrollback(drag.from, drag.text_len);
        assert_eq!(grid.history_len(), 10_000);
        let mut cursor = cursor_at(49, 0);
        let mut saved = cursor_at(0, 0);
        let start = Instant::now();
        grid.reflow(
            &mut cursor,
            &mut saved,
            &mut [],
            GridSize {
                cols: drag.to,
                rows: 50,
            },
            ScrollbackOnGrow::Keep,
        );
        let elapsed = start.elapsed();
        assert_eq!(grid.history_len(), drag.history_after);
        elapsed
    }

    /// Asserts that reflowing a freshly filled scrollback of ten thousand
    /// rows fits the per-pane frame budget of 8 ms, judged by the median
    /// of fresh samples, in every drag scenario but the one that rewraps
    /// two-row lines at a width where they still take two rows, whose
    /// median is reported as over the budget rather than asserted.
    ///
    /// Case: a user whose scrollback is full of long output lines drags
    /// the window narrower, widens it for the first time, drags it across
    /// the width the lines wrap at, and maximizes a window whose output was
    /// printed in a narrow split.
    #[test]
    #[ignore = "timing; meaningful only in a release build: run with `cargo test -p orzma_vt --release -- --ignored reflowing_a_full --nocapture`"]
    fn reflowing_a_full_scrollback_fits_the_frame_budget() {
        const SAMPLES: usize = 7;
        let budget = Duration::from_millis(8);
        let drags = [
            Drag {
                name: "narrow 200 -> 120, 150-char lines",
                from: 200,
                text_len: 150,
                to: 120,
                history_after: 10_000,
                held_to_budget: true,
            },
            Drag {
                name: "first grow 199 -> 200, 150-char lines",
                from: 199,
                text_len: 150,
                to: 200,
                history_after: 10_000,
                held_to_budget: true,
            },
            Drag {
                name: "cross the wrap 150 -> 149, 150-char lines",
                from: 150,
                text_len: 150,
                to: 149,
                history_after: 10_000,
                held_to_budget: true,
            },
            Drag {
                name: "cross the wrap 200 -> 199, 200-char lines",
                from: 200,
                text_len: 200,
                to: 199,
                history_after: 10_000,
                held_to_budget: true,
            },
            Drag {
                name: "join 100 -> 200, 150-char lines",
                from: 100,
                text_len: 150,
                to: 200,
                history_after: 5_000,
                held_to_budget: true,
            },
            Drag {
                name: "join 120 -> 200, 1000-char lines",
                from: 120,
                text_len: 1000,
                to: 200,
                history_after: 5_557,
                held_to_budget: true,
            },
            Drag {
                name: "join 199 -> 200, 1000-char lines",
                from: 199,
                text_len: 1000,
                to: 200,
                history_after: 8_334,
                held_to_budget: true,
            },
            Drag {
                name: "narrow 200 -> 120, 1000-char lines",
                from: 200,
                text_len: 1000,
                to: 120,
                history_after: 10_000,
                held_to_budget: true,
            },
            Drag {
                name: "widen 160 -> 200, 300-char lines",
                from: 160,
                text_len: 300,
                to: 200,
                history_after: 10_000,
                held_to_budget: false,
            },
        ];
        let mut medians: Vec<(Drag, Duration)> = Vec::new();
        for drag in drags {
            let mut samples: Vec<Duration> =
                (0..SAMPLES).map(|_| time_one_full_reflow(drag)).collect();
            samples.sort();
            let median = samples[SAMPLES / 2];
            let max = samples[SAMPLES - 1];
            eprintln!(
                "{}: median {median:?}, max {max:?}, samples {samples:?}",
                drag.name
            );
            medians.push((drag, median));
        }
        let (worst, _) = *medians
            .iter()
            .max_by_key(|(_, median)| *median)
            .expect("at least one drag");
        let four_panes: Duration = (0..4).map(|_| time_one_full_reflow(worst)).sum();
        eprintln!("four panes of {}: {four_panes:?}", worst.name);
        for (drag, median) in &medians {
            if !drag.held_to_budget && *median > budget {
                eprintln!(
                    "OVER BUDGET (reported, not asserted): {}: median {median:?} > {budget:?}",
                    drag.name
                );
            }
        }
        let over: Vec<(&str, Duration)> = medians
            .iter()
            .filter(|(drag, median)| drag.held_to_budget && *median > budget)
            .map(|(drag, median)| (drag.name, *median))
            .collect();
        assert!(over.is_empty(), "over the {budget:?} budget: {over:?}");
    }
}
