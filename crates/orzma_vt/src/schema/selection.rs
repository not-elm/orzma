//! Selection vocabulary: the parameter types of the VT's selection
//! operations ([`SelectionKind`], [`CellSide`]) and the renderable
//! range the VT reports back ([`SelectionRange`]).

use crate::schema::GridPoint;

/// A renderable selection: normalized active-grid endpoints plus the
/// shape they span.
///
/// `start` is the top-left and `end` the bottom-right of the selected
/// cells, both inclusive — anchor/moving-end order is already resolved
/// and cell-side trimming applied by the VT. The endpoints are raw
/// grid positions and do not move when the user scrolls; project them
/// with [`crate::schema::GridLine::to_viewport`] to place the
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
/// Deliberately narrower than [`SelectionKind`]: `Simple` and `Semantic`
/// differ only in how the range is built (word snapping) and both render
/// as `Linear`, so the renderer needs this three-way split rather than
/// the four-way input granularity.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// Cell-by-cell, wrapping at the end of each line.
    Simple,
    // /// A rectangular column block.
    // Block,
    /// Snapped outward to word boundaries.
    // Semantic,
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
