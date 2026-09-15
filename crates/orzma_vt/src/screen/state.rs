//! The write cursor's mutable state: position, pen, and deferred wrap.

use crate::screen::cell::Pen;
use crate::screen::grid::coords::{GridColumn, ScreenLine};

#[derive(Default, Debug, PartialEq)]
pub(super) struct ScreenState {
    pub line: ScreenLine,
    pub column: GridColumn,
    pub pending_wrap: bool,
    pub pen: Pen,
    /// The cell the last printed glyph's body landed on; `None` before
    /// the first print.
    pub last_landing: Option<(ScreenLine, GridColumn)>,
}
