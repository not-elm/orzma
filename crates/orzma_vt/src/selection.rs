//! Selection vocabulary: the operations a host applies to the VT's
//! selection ([`SelectionOp`]) and the renderable range the VT reports
//! back ([`SelectionRange`]).

/// A viewport cell named by an input operation, `x` = column and
/// `y` = row, both 0-based.
///
/// Input-side coordinate: a pointer always sits inside the viewport, so
/// both axes are unsigned. The output-side counterpart is
/// [`ViewportPoint`], whose row is signed because a selection endpoint
/// can scroll out of the viewport.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Position {
    /// 0-based viewport column.
    pub x: usize,
    /// 0-based viewport row.
    pub y: usize,
}

/// A selection endpoint projected into viewport coordinates.
///
/// Endpoints can lie outside the viewport once the user scrolls: the row
/// is clamped to `-1` when the endpoint sits above the first visible row
/// and to the viewport row count when it sits below the last one.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ViewportPoint {
    /// Viewport row; `-1` = above the viewport, the viewport row count =
    /// below it.
    pub row: i16,
    /// 0-based viewport column.
    pub column: u16,
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

/// One selection operation.
///
/// The two `Start` variants differ in where the anchor comes from: a mouse
/// drag names an explicit cell, while vi mode anchors at the vi cursor, whose
/// position only the VT knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionOp {
    /// Anchor a new selection at an explicit viewport cell (mouse press).
    StartAt {
        /// Viewport cell, `x` = column and `y` = row, both 0-based.
        cell: Position,
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
        /// Viewport cell, `x` = column and `y` = row, both 0-based.
        cell: Position,
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
    /// A rectangular column block.
    Block,
    /// Snapped outward to word boundaries.
    Semantic,
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
