//! Viewport state: where the visible window sits relative to the live
//! tail, the coordinates measured from its top, and the motions that
//! move it.

/// Number of scrollback rows the viewport sits above the live tail.
///
/// `0` means the viewport is pinned to the live tail; a positive value
/// counts the scrollback rows showing above it. The unit is grid rows.
///
/// # Invariants
///
/// The producing backend keeps the value within the scrollback
/// capacity and at `0` while the alternate screen is active; this type
/// does not enforce either bound itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayOffset(pub u32);

/// A line in viewport coordinates: `0` is the topmost visible row.
///
/// It is the viewport projection of a [`crate::prelude::GridLine`],
/// related by `viewport_line = grid_line + display_offset`. Negative
/// values sit above the viewport, values at or past the viewport row
/// count sit below it. Clamping off-viewport values to the `-1` /
/// row-count sentinels is the responsibility of the conversion that
/// produces the value, not of this type.
/// The ordering is spatial — where the row sits in the window this
/// frame — not an identity: the same `ViewportLine` names different
/// content once the user scrolls or the grid is resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ViewportLine(pub u16);

/// A viewport motion over the scrollback, clamped by the VT at both
/// the oldest retained line and the live tail.
///
/// `Delta` is signed: positive moves toward older output (deeper into
/// scrollback), negative toward the live tail. Page-sized variants are
/// resolved against the live grid height by the VT backend, so callers
/// never need to know the row count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scroll {
    /// Moves by a signed line count: positive toward older output,
    /// negative toward the live tail.
    Delta(i32),
    /// One screenful toward older output.
    PageUp,
    /// One screenful toward the live tail.
    PageDown,
    /// Half a screenful toward older output.
    HalfPageUp,
    /// Half a screenful toward the live tail.
    HalfPageDown,
    /// The oldest line still in scrollback.
    Top,
    /// The live tail.
    Bottom,
}

#[derive(Debug, Default, PartialEq)]
pub struct Viewport {
    pub offset: DisplayOffset,
}
