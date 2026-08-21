//! The DECSTBM scroll region.

use crate::schema::ScreenLine;

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
