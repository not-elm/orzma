//! Vi mode: the cursor it adds, the switch that enters or leaves it, and
//! how the screen keeps that cursor on its text.

mod motion;

use self::motion::MotionGrid;
use crate::frame::damage::DamageSpan;
use crate::screen::Screen;
use crate::screen::cell::CellWidth;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint};
use crate::screen::selection::{CellSide, SelectionKind};
use crate::screen::viewport::Scroll;

/// Vi-mode cursor position in active-grid coordinates.
///
/// The line goes negative while the vi cursor sits in scrollback history.
/// The point always lies inside the viewport, and it never names the
/// continuation column of a wide glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViCursor {
    /// Grid cell the vi cursor sits on.
    pub point: GridPoint,
}

/// The direction of a vi-mode switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViModeSwitch {
    /// Enter vi mode.
    Enter,
    /// Leave vi mode.
    Exit,
}

/// A vi-cursor motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViMotion {
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// One cell left.
    Left,
    /// One cell right.
    Right,
    /// First column of the line.
    First,
    /// Last column of the line.
    Last,
    /// First non-blank column of the line.
    FirstOccupied,
    /// Top line of the viewport.
    High,
    /// Middle line of the viewport.
    Middle,
    /// Bottom line of the viewport.
    Low,
    /// Start of the previous semantic word.
    SemanticLeft,
    /// Start of the next semantic word.
    SemanticRight,
    /// End of the previous semantic word.
    SemanticLeftEnd,
    /// End of the next semantic word.
    SemanticRightEnd,
    /// Start of the previous whitespace-delimited word.
    WordLeft,
    /// Start of the next whitespace-delimited word.
    WordRight,
    /// End of the previous whitespace-delimited word.
    WordLeftEnd,
    /// End of the next whitespace-delimited word.
    WordRightEnd,
    /// Matching bracket of the one under the cursor.
    Bracket,
    /// Previous paragraph break.
    ParagraphUp,
    /// Next paragraph break.
    ParagraphDown,
}

/// The characters that end a word for the semantic vi motions, besides
/// whitespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticEscapeChars(String);

impl SemanticEscapeChars {
    /// Builds the separator set from every character of `chars`.
    pub fn new(chars: &str) -> Self {
        Self(chars.to_owned())
    }

    /// Whether `c` ends a semantic word: a blank, a tab, or one of the
    /// characters the set was built from.
    pub fn contains(&self, c: char) -> bool {
        is_blank_char(c) || self.0.contains(c)
    }
}

/// The separator set `` ,│`|:"' ()[]{}<> `` plus the tab.
impl Default for SemanticEscapeChars {
    fn default() -> Self {
        Self::new(DEFAULT_SEMANTIC_ESCAPE_CHARS)
    }
}

/// What a host-driven vi operation changed, and so what the next frame
/// carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewChange {
    /// Nothing a frame carries changed.
    Unchanged,
    /// The vi cursor or the selection changed while the viewport stayed.
    Carried,
    /// The viewport moved, so every row repaints.
    Repainted,
}

impl ViewChange {
    /// Classifies an operation from whether the vi cursor or the selection
    /// changed and from the damage its viewport motion reported.
    pub fn classify(carried: bool, viewport: Option<DamageSpan>) -> Self {
        if viewport.is_some() {
            Self::Repainted
        } else if carried {
            Self::Carried
        } else {
            Self::Unchanged
        }
    }

    /// The damage the change owes: [`DamageSpan::Full`] exactly when the
    /// viewport moved.
    pub fn damage(self) -> Option<DamageSpan> {
        match self {
            Self::Repainted => Some(DamageSpan::Full),
            Self::Unchanged | Self::Carried => None,
        }
    }

    /// Whether anything a frame carries changed.
    pub fn is_changed(self) -> bool {
        self != Self::Unchanged
    }
}

/// The vi cursor one screen owns; `None` outside vi mode.
#[derive(Debug)]
pub(crate) struct ScreenVi {
    point: Option<GridPoint>,
}

impl ScreenVi {
    /// Builds the state of a screen outside vi mode.
    pub fn new() -> Self {
        Self { point: None }
    }

    /// The stored vi-cursor position; `None` outside vi mode.
    pub fn point(&self) -> Option<GridPoint> {
        self.point
    }

    /// Stores `point`; returns whether the stored position changed.
    pub fn set(&mut self, point: GridPoint) -> bool {
        let changed = self.point != Some(point);
        self.point = Some(point);
        changed
    }

    /// Drops the vi cursor; returns whether there was one.
    pub fn clear(&mut self) -> bool {
        self.point.take().is_some()
    }

    /// Pulls the vi cursor onto a row in `top..=bottom` and onto
    /// `last_column` or before; returns whether it moved. Nothing happens
    /// outside vi mode.
    pub fn clamp(&mut self, top: GridLine, bottom: GridLine, last_column: u16) -> bool {
        let Some(point) = self.point else {
            return false;
        };
        let clamped = GridPoint {
            line: GridLine(point.line.0.clamp(top.0, bottom.0)),
            column: GridColumn(point.column.0.min(last_column)),
        };
        self.set(clamped)
    }

    /// Moves a vi cursor on a row in `top..=bottom` by `delta` rows, down
    /// when positive, stopping at `top` and at `bottom`.
    pub fn follow_rows(&mut self, top: GridLine, bottom: GridLine, delta: i32) {
        if let Some(point) = &mut self.point
            && (top.0..=bottom.0).contains(&point.line.0)
        {
            point.line = GridLine((point.line.0 + delta).clamp(top.0, bottom.0));
        }
    }

    /// Moves the vi cursor by `rows` rows, down when positive.
    pub fn shift(&mut self, rows: i32) {
        if let Some(point) = &mut self.point {
            point.line = GridLine(point.line.0.saturating_add(rows));
        }
    }
}

/// Vi mode.
impl Screen {
    /// The vi cursor as a frame reports it, on the body of a wide glyph
    /// rather than its continuation column; `None` outside vi mode.
    pub fn vi_cursor(&self) -> Option<ViCursor> {
        let mut point = self.vi.point()?;
        let on_continuation = self
            .grid
            .cell_at(point)
            .is_some_and(|cell| cell.width == CellWidth::Spacer);
        if on_continuation {
            point.column = GridColumn(point.column.0.saturating_sub(1));
        }
        Some(ViCursor { point })
    }

    /// Whether this screen holds the vi cursor.
    pub fn is_vi_mode(&self) -> bool {
        self.vi.point().is_some()
    }

    /// Seats the vi cursor on the write cursor, or on the viewport's
    /// top-left cell when the viewport is scrolled back past the write
    /// cursor.
    pub fn seat_vi_cursor(&mut self) {
        let (top, bottom) = self.viewport_lines();
        let cursor_line = GridLine::from(self.state.line);
        let point = if cursor_line.0 > bottom.0 {
            GridPoint {
                line: top,
                column: GridColumn(0),
            }
        } else {
            GridPoint {
                line: cursor_line,
                column: self.state.column,
            }
        };
        let _ = self.vi.set(point);
    }

    /// Removes the vi cursor; returns whether there was one.
    pub fn drop_vi_cursor(&mut self) -> bool {
        self.vi.clear()
    }

    /// The first and the last active-grid lines the viewport shows.
    pub fn viewport_lines(&self) -> (GridLine, GridLine) {
        let offset = i32::try_from(self.viewport.offset.0).unwrap_or(i32::MAX);
        let rows = i32::from(self.grid.size().rows);
        (GridLine(-offset), GridLine(rows - 1 - offset))
    }

    /// Enters vi mode: drops the selection and seats the vi cursor on the
    /// write cursor, or on the viewport's top-left cell when the viewport
    /// is scrolled back past the write cursor. Returns `true` when it
    /// entered vi mode and `false` when vi mode was already on.
    pub fn enter_vi_mode(&mut self) -> bool {
        if self.is_vi_mode() {
            return false;
        }
        let _ = self.selection.clear();
        self.seat_vi_cursor();
        true
    }

    /// Leaves vi mode: drops the vi cursor and the selection and returns
    /// the viewport to the live tail. Nothing changes outside vi mode.
    pub fn exit_vi_mode(&mut self) -> ViewChange {
        if !self.vi.clear() {
            return ViewChange::Unchanged;
        }
        let _ = self.selection.clear();
        ViewChange::classify(true, self.scroll(Scroll::Bottom))
    }

    /// Pulls the vi cursor inside the viewport and onto the last column or
    /// before; returns whether it moved.
    pub fn clamp_vi_cursor(&mut self) -> bool {
        if !self.is_vi_mode() {
            return false;
        }
        let (top, bottom) = self.viewport_lines();
        let last_column = self.grid.size().cols.saturating_sub(1);
        self.vi.clamp(top, bottom, last_column)
    }

    /// Moves the vi cursor by `motion` and scrolls the viewport just far
    /// enough to show it. Nothing changes outside vi mode.
    ///
    /// A selection that covers a cell follows the vi cursor, covering both
    /// of its end cells. The damage is [`DamageSpan::Full`] exactly when
    /// the viewport moved.
    pub fn vi_motion(
        &mut self,
        motion: ViMotion,
        escape_chars: &SemanticEscapeChars,
    ) -> ViewChange {
        let Some(from) = self.vi.point() else {
            return ViewChange::Unchanged;
        };
        let to =
            MotionGrid::new(&self.grid, self.viewport_lines(), escape_chars).apply(from, motion);
        let moved = self.vi.set(to);
        let damage = self.scroll_to_vi_cursor();
        let followed = self.follow_vi_cursor();
        ViewChange::classify(moved || followed, damage)
    }

    /// Applies a viewport motion in vi mode and moves the vi cursor with
    /// it.
    ///
    /// A line motion leaves the vi cursor on its line and column, pulling
    /// it onto the nearest viewport edge row when that line scrolls out of
    /// view. A page or half-page motion moves the vi cursor by the same
    /// number of rows onto that row's first non-blank cell, or onto its
    /// first column when the row is blank. `Top` and `Bottom` put the vi
    /// cursor on the oldest row and on the bottom row: it lands on the
    /// row's first non-blank cell, or on the last column when the row is
    /// blank, and for `Bottom` on the first non-blank cell of the logical
    /// line when the bottom row continues a wrapped line. Outside vi mode it
    /// moves only the viewport.
    ///
    /// A selection that covers a cell follows the vi cursor, covering both
    /// of its end cells. The damage is [`DamageSpan::Full`] exactly when
    /// the viewport moved.
    pub fn vi_scroll(&mut self, scroll: Scroll, escape_chars: &SemanticEscapeChars) -> ViewChange {
        let Some(from) = self.vi.point() else {
            return ViewChange::classify(false, self.scroll(scroll));
        };
        let rows = i32::from(self.grid.size().rows);
        let target = {
            let grid = MotionGrid::new(&self.grid, self.viewport_lines(), escape_chars);
            match scroll {
                Scroll::Delta(_) => None,
                Scroll::PageUp => Some(grid.scroll_target(from, rows)),
                Scroll::PageDown => Some(grid.scroll_target(from, -rows)),
                Scroll::HalfPageUp => Some(grid.scroll_target(from, rows / 2)),
                Scroll::HalfPageDown => Some(grid.scroll_target(from, -(rows / 2))),
                Scroll::Top => {
                    let top = GridPoint {
                        line: grid.topmost(),
                        column: from.column,
                    };
                    Some(grid.apply(top, ViMotion::FirstOccupied))
                }
                Scroll::Bottom => {
                    let bottom = GridPoint {
                        line: grid.bottommost(),
                        column: from.column,
                    };
                    let once = grid.apply(bottom, ViMotion::FirstOccupied);
                    Some(grid.apply(once, ViMotion::FirstOccupied))
                }
            }
        };
        let moved = target.is_some_and(|point| self.vi.set(point));
        let damage = self.scroll(scroll);
        let clamped = self.clamp_vi_cursor();
        let followed = self.follow_vi_cursor();
        ViewChange::classify(moved || clamped || followed, damage)
    }

    /// Starts, re-kinds, or clears a selection of `kind` at the vi cursor;
    /// returns whether the selection changed. Returns `false` outside vi
    /// mode.
    ///
    /// A selection of the same kind that covers a cell is cleared; one of
    /// another kind switches to `kind` and keeps its anchor; otherwise a
    /// new selection starts on the vi cursor's cell. A selection left in
    /// place covers both of its end cells.
    pub fn toggle_vi_selection(&mut self, kind: SelectionKind) -> bool {
        let Some(point) = self.vi_cursor().map(|cursor| cursor.point) else {
            return false;
        };
        let current = self
            .selection
            .kind()
            .filter(|_| self.selection_range().is_some());
        match current {
            Some(current) if current == kind => self.selection.clear(),
            Some(_) => {
                self.selection.set_kind(kind);
                let _ = self.include_selection_cells();
                true
            }
            None => {
                let Some(end) = self.selection_end(point, CellSide::Left) else {
                    return false;
                };
                let _ = self.selection.start(end, kind);
                let _ = self.include_selection_cells();
                true
            }
        }
    }

    /// Moves each selection end onto the far side of the cell it was set
    /// from, so the selection covers both end cells; returns whether a
    /// boundary moved.
    pub fn include_selection_cells(&mut self) -> bool {
        let Some((anchor, moving)) = self.selection.ends() else {
            return false;
        };
        let (Some(anchor_line), Some(moving_line)) = (
            self.grid.grid_line(anchor.line()),
            self.grid.grid_line(moving.line()),
        ) else {
            return false;
        };
        let last_column = self.grid.size().cols.saturating_sub(1);
        let anchor_cell = (anchor_line.0, anchor.column().min(last_column));
        let moving_cell = (moving_line.0, moving.column().min(last_column));
        self.selection
            .include_both_cells(anchor_cell <= moving_cell, last_column)
    }

    /// Scrolls the viewport just far enough to show the vi cursor; `None`
    /// when it was already shown.
    fn scroll_to_vi_cursor(&mut self) -> Option<DamageSpan> {
        let point = self.vi.point()?;
        let (top, bottom) = self.viewport_lines();
        let delta = if point.line.0 < top.0 {
            top.0 - point.line.0
        } else if point.line.0 > bottom.0 {
            bottom.0 - point.line.0
        } else {
            return None;
        };
        self.scroll(Scroll::Delta(delta))
    }

    /// Moves a selection that covers a cell so its moving end sits on the
    /// vi cursor's cell and both end cells are covered; returns whether the
    /// selection changed.
    fn follow_vi_cursor(&mut self) -> bool {
        let Some(point) = self.vi_cursor().map(|cursor| cursor.point) else {
            return false;
        };
        if self.selection_range().is_none() {
            return false;
        }
        let Some(end) = self.selection_end(point, CellSide::Left) else {
            return false;
        };
        self.extend_covering(end)
    }
}

/// The word separators of the built-in semantic motions, besides
/// whitespace.
const DEFAULT_SEMANTIC_ESCAPE_CHARS: &str = ",│`|:\"' ()[]{}<>\t";

/// Whether `c` is a blank or a tab.
fn is_blank_char(c: char) -> bool {
    matches!(c, ' ' | '\t')
}

#[cfg(test)]
mod tests;
