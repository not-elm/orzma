//! `Scroll`: the viewport motion vocabulary a VT applies to its grid.

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
