//! Selection vocabulary: the operations a host applies to the VT's
//! selection ([`SelectionOp`]) and the renderable range the VT reports
//! back ([`SelectionRange`]).

#[cfg(feature = "alacritty")]
use alacritty_terminal::{
    index::{Point, Side},
    selection::SelectionType,
};

/// A cell in viewport coordinates: `row` counted from the top of the
/// visible area, `column` from its left edge.
///
/// Carries selection endpoints in both directions. The row is signed
/// because an endpoint — or the moving end of a drag — can leave the
/// viewport: negative rows sit above the first visible row, rows at or
/// past the viewport row count sit below the last one. The two
/// directions treat those out-of-range rows differently: on output the
/// VT clamps them to `-1` / the row count so the renderer only has to
/// handle two sentinels, while on input the value is taken literally,
/// so `-1` means exactly one row above the viewport.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ViewportPoint {
    /// Viewport row. Negative = above the viewport, at or past the
    /// viewport row count = below it.
    pub row: i16,
    /// 0-based viewport column.
    pub column: u16,
}

#[cfg(feature = "alacritty")]
impl ViewportPoint {
    pub fn from_alacritty_point(p: Point, display_offset: u32) -> Self {
        Self {
            row: (p.line.0 as i64 + display_offset as i64) as i16,
            column: p.column.0 as u16,
        }
    }
}

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

/// One selection operation.
///
/// The two `Start` variants differ in where the anchor comes from: a mouse
/// drag names an explicit cell, while vi mode anchors at the vi cursor, whose
/// position only the VT knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionOp {
    /// Anchor a new selection at an explicit viewport cell (mouse press).
    StartAt {
        /// Viewport cell the press landed on.
        cell: ViewportPoint,
        /// Which half of the cell the anchor sits in.
        side: CellSide,
        /// Granularity of the new selection.
        kind: SelectionKind,
    },
    /// Anchor a new selection at the vi cursor (vi-mode `v` / `V`).
    StartAtViCursor {
        /// Granularity of the new selection.
        kind: SelectionKind,
    },
    /// Move the moving end of the active selection to a viewport cell
    /// (mouse drag). No-op when nothing is selected.
    UpdateTo {
        /// Viewport cell the moving end is dragged to. May sit outside
        /// the viewport when the drag leaves it.
        cell: ViewportPoint,
        /// Which half of the cell the moving end sits in.
        side: CellSide,
    },
    /// Switch granularity while keeping the anchor (vi-mode `v` while `V` is
    /// active, and the reverse).
    ChangeKind(SelectionKind),
    /// Drop any active selection.
    Clear,
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
