//! The cursor snapshot a screen reports.

use crate::device::modes::CursorShape;
use crate::screen::grid::coords::GridPoint;

/// Cursor state at snapshot time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    /// The grid position
    pub point: GridPoint,
    /// Visual shape selected by DECSCUSR.
    pub shape: CursorShape,
    /// True when the cursor blinks rather than being drawn
    /// continuously.
    pub blinking: bool,
    /// True when the application wants the cursor drawn, which DECTCEM alone decides.
    pub visible: bool,
}
