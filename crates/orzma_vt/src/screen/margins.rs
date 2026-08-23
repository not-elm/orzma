//! The DECSTBM scroll region and the DECOM cursor origin.

use crate::schema::ScreenLine;

/// The scrolling region paired with the cursor origin it may impose.
///
/// DECSTBM and DECOM are orthogonal: the margins bound scrolling
/// whichever way the origin mode is set, and the origin mode only
/// decides whether the cursor is homed and clamped to them. Holding the
/// two together keeps that interaction in one place instead of leaving
/// every caller to re-derive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollRegion {
    /// The rows scrolling is confined to.
    pub margins: Margins,
    /// Whether the cursor origin follows those margins.
    pub origin_mode: OriginMode,
}

/// Whether the cursor origin and its motion bounds follow the margins.
///
/// # Control Functions
///
/// - `DECOM` (`CSI ? 6 h` / `CSI ? 6 l`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OriginMode {
    /// Home sits at the top margin, and the cursor cannot move outside
    /// the margins.
    WithinMargins,
    /// Home sits at the upper-left corner of the screen, and the cursor
    /// can move outside the margins. This is the state a power-up or a
    /// reset leaves behind.
    #[default]
    UpperLeftCorner,
}

/// DECSTBM scroll region; `bottom` is the inclusive last row index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Margins {
    /// First row of the scroll region (0 = top of screen).
    pub top: ScreenLine,
    /// Inclusive last row of the scroll region (default `rows - 1`).
    pub bottom: ScreenLine,
}

impl Margins {
    /// Builds the default full-screen region for a grid of `rows` rows.
    pub fn new(rows: u16) -> Self {
        Self {
            top: ScreenLine(0),
            bottom: ScreenLine(rows - 1),
        }
    }
}
