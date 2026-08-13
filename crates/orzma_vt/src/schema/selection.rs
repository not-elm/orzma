//! Selection vocabulary: the parameter types of the VT's selection
//! operations ([`SelectionKind`], [`CellSide`]) and the renderable
//! range the VT reports back ([`SelectionRange`]).

use crate::schema::ViewportPoint;
#[cfg(feature = "alacritty")]
use alacritty_terminal::{index::Side, selection::SelectionType};

/// A renderable selection: normalized viewport endpoints plus the shape
/// they span.
///
/// `start` is the top-left and `end` the bottom-right of the selected
/// cells, both inclusive — anchor/moving-end order is already resolved
/// and cell-side trimming applied by the VT.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SelectionRange {
    /// Top-left selected cell (inclusive).
    pub start: ViewportPoint,
    /// Bottom-right selected cell (inclusive).
    pub end: ViewportPoint,
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

#[cfg(feature = "alacritty")]
impl From<SelectionType> for SelectionKind {
    fn from(value: SelectionType) -> Self {
        match value {
            SelectionType::Simple => SelectionKind::Simple,
            SelectionType::Lines => SelectionKind::Lines,
            _ => todo!("Not supported yet"),
        }
    }
}

#[cfg(feature = "alacritty")]
impl From<SelectionKind> for SelectionType {
    fn from(value: SelectionKind) -> Self {
        match value {
            SelectionKind::Simple => Self::Simple,
            SelectionKind::Lines => Self::Lines,
        }
    }
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

#[cfg(feature = "alacritty")]
impl From<CellSide> for Side {
    fn from(value: CellSide) -> Self {
        match value {
            CellSide::Left => Self::Left,
            CellSide::Right => Self::Right,
        }
    }
}
