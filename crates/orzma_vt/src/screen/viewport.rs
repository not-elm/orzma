//! Viewport state: where the visible window sits relative to the live
//! tail, and how it moves.

use crate::screen::grid::coords::GridLine;

/// Number of scrollback rows the viewport sits above the live tail.
///
/// `0` means the viewport is pinned to the live tail; a positive value
/// counts the scrollback rows showing above it. The unit is grid rows.
///
/// This type bounds nothing: its producer must keep the value within
/// the scrollback capacity and at `0` while the alternate screen is
/// active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayOffset(pub u32);

/// A line in viewport coordinates: `0` is the topmost visible row.
///
/// It is the viewport projection of a [`crate::prelude::GridLine`],
/// related by `viewport_line = grid_line + display_offset`, and only
/// rows the viewport actually shows are representable. The ordering is
/// spatial — where the row sits in the window this frame — not an
/// identity: the same `ViewportLine` names different content once the
/// user scrolls or the grid is resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ViewportLine(pub u16);

impl ViewportLine {
    /// Projects the line into active-grid coordinates, the inverse of
    /// [`GridLine::to_viewport`]: `grid_line = viewport_line - offset`.
    ///
    /// # Panics
    ///
    /// Panics rather than wrapping when the offset does not fit `i32`.
    #[inline]
    pub fn to_grid(self, offset: DisplayOffset) -> GridLine {
        let offset = i32::try_from(offset.0).expect("scrollback never exceeds i32::MAX rows");
        GridLine(i32::from(self.0) - offset)
    }
}

/// A viewport motion over the scrollback, clamped by the VT at both
/// the oldest retained line and the live tail.
///
/// Page-sized variants are resolved against the live grid height.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a viewport line projects to the grid line the
    /// display offset places it on, and that the projection round-trips
    /// through `GridLine::to_viewport`.
    ///
    /// Case: the user has scrolled three rows back and the renderer
    /// materializes the second visible row.
    #[test]
    fn to_grid_inverts_to_viewport() {
        let offset = DisplayOffset(3);
        let line = ViewportLine(1).to_grid(offset);
        assert_eq!(line, GridLine(-2));
        assert_eq!(line.to_viewport(offset, 5), Some(ViewportLine(1)));
    }
}
