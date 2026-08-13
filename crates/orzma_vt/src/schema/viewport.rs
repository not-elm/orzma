//! Viewport coordinates: the cell addressing shared by selection,
//! cursor, and backend grid translation.

#[cfg(feature = "alacritty")]
use crate::schema::DisplayOffset;
#[cfg(feature = "alacritty")]
use alacritty_terminal::index::Point;

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
    /// Converts an alacritty grid `Point` into viewport coordinates,
    /// clamping off-viewport rows to the `-1` / `rows` sentinels.
    ///
    /// `p.line` counts from the top of the active area and goes negative
    /// into scrollback; `display_offset` shifts that back into a row
    /// relative to the top of the currently visible viewport. The clamp
    /// implements the output half of the type's contract and keeps the
    /// `i16` cast lossless for arbitrarily deep scrollback.
    ///
    /// The below-viewport sentinel is itself capped at [`i16::MAX`]: a
    /// grid taller than that cannot be addressed by an `i16` row, and
    /// letting the cast wrap would report a below-viewport endpoint as
    /// an above-viewport one.
    pub fn from_alacritty_point(p: Point, display_offset: DisplayOffset, rows: u16) -> Self {
        let below_viewport = i64::from(rows).min(i64::from(i16::MAX));
        let row = (p.line.0 as i64 + i64::from(display_offset.0)).clamp(-1, below_viewport);
        Self {
            row: row as i16,
            column: p.column.0 as u16,
        }
    }
}
