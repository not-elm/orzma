//! Selection vocabulary: the span a selection covers, how it is shaped,
//! and which side of a cell an endpoint sits on.

use crate::screen::grid::coords::GridPoint;

/// A renderable selection: normalized active-grid endpoints plus the
/// shape they span.
///
/// `start` is the top-left and `end` the bottom-right of the selected
/// cells, both inclusive — anchor/moving-end order is already resolved
/// and cell-side trimming applied by the VT. The endpoints are raw
/// grid positions and do not move when the user scrolls; project them
/// with [`crate::prelude::GridLine::to_viewport`] to place the
/// highlight on screen.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SelectionRange {
    /// Top-left selected cell (inclusive).
    pub start: GridPoint,
    /// Bottom-right selected cell (inclusive).
    pub end: GridPoint,
    /// The shape spanned between the endpoints.
    pub geometry: SelectionGeometry,
}

/// The shape a [`SelectionRange`] spans between its endpoints.
///
/// Wider than [`SelectionKind`]: the two current selection kinds only
/// ever convert to `Linear` or `Lines`. `Block` exists for the
/// renderer to support once a future selection kind needs it, but the
/// conversion does not produce it yet.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SelectionGeometry {
    /// A cell run wrapping at the end of each row.
    Linear,
    /// A rectangular column block.
    Block,
    /// Whole rows.
    Lines,
}

impl From<SelectionKind> for SelectionGeometry {
    fn from(value: SelectionKind) -> Self {
        match value {
            SelectionKind::Lines => Self::Lines,
            _ => Self::Linear,
        }
    }
}

/// Selection granularity.
// TODO: Add the `Block` (rectangular column) and `Semantic` (snapped to
// word boundaries) kinds once the selection capability trait lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// Cell-by-cell, wrapping at the end of each line.
    Simple,
    /// Whole lines.
    Lines,
}

/// Which half of a cell a selection endpoint sits in.
///
/// Decides whether the cell under the cursor is included: an endpoint on the
/// far side of a cell takes that cell, an endpoint on the near side stops
/// before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellSide {
    /// Left half.
    Left,
    /// Right half.
    Right,
}
