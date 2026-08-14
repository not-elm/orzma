//! Viewport coordinates: the cell addressing selection input carries
//! across the VT boundary.

/// A cell in viewport coordinates: `row` counted from the top of the
/// visible area, `column` from its left edge.
///
/// Carries selection input (mouse presses and drags) into the VT. The
/// row is signed because the moving end of a drag can leave the
/// viewport: negative rows sit above the first visible row, rows at
/// or past the viewport row count sit below the last one, and the
/// value is taken literally — `-1` means exactly one row above the
/// viewport.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ViewportPoint {
    /// Viewport row. Negative = above the viewport, at or past the
    /// viewport row count = below it.
    pub row: i16,
    /// 0-based viewport column.
    pub column: u16,
}
