//! Vi mode: the cursor it adds, the switch that enters or leaves it, and
//! how the screen keeps that cursor on its text.

use crate::frame::damage::DamageSpan;
use crate::screen::Screen;
use crate::screen::cell::CellWidth;
use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint};
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
}

/// Vi mode.
impl Screen {
    /// The vi cursor as a frame reports it, on the body of a wide glyph
    /// rather than its continuation column; `None` outside vi mode.
    pub fn vi_cursor(&self) -> Option<ViCursor> {
        let mut point = self.vi.point()?;
        let on_continuation = self
            .grid
            .row_at(point.line)
            .and_then(|row| row.get(usize::from(point.column.0)))
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
    /// cursor; returns whether the vi cursor appeared or moved.
    pub fn seat_vi_cursor(&mut self) -> bool {
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
        self.vi.set(point)
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

    /// Enters vi mode: drops the selection and seats the vi cursor as
    /// [`Self::seat_vi_cursor`] does; returns `false` when already in vi
    /// mode.
    pub fn enter_vi_mode(&mut self) -> bool {
        if self.is_vi_mode() {
            return false;
        }
        let _ = self.selection.clear();
        self.seat_vi_cursor()
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
        let (top, bottom) = self.viewport_lines();
        let last_column = self.grid.size().cols.saturating_sub(1);
        self.vi.clamp(top, bottom, last_column)
    }
}

#[cfg(test)]
mod tests;
