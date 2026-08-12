//! Viewport coordinates: the cell addressing shared by selection,
//! cursor, and backend grid translation.

#[cfg(feature = "alacritty")]
use alacritty_terminal::index::Point;
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
