//! The write cursor's mutable state: position, pen, and deferred wrap.

use crate::schema::{GridColumn, ScreenLine};
use crate::screen::cell::Pen;

#[derive(Default)]
pub(super) struct ScreenState {
    pub line: ScreenLine,
    pub column: GridColumn,
    pub pending_wrap: bool,
    pub pen: Pen,
}
