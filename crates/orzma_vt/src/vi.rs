//! Vi mode: the cursor it adds and the switch that enters or leaves it.

use crate::screen::grid::coords::GridPoint;

/// Vi-mode cursor position in active-grid coordinates.
///
/// The line goes negative while the vi cursor sits in scrollback
/// history. The sign is not a visibility signal: a negative line can
/// still be visible, so project `point` with
/// [`crate::prelude::GridLine::to_viewport`] to decide whether there
/// is a cell to paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViCursor {
    /// Grid cell the vi cursor sits on.
    pub point: GridPoint,
}

/// The direction of a vi-mode switch.
///
/// This terminal does not implement vi mode, so nothing in it acts on a
/// switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViModeSwitch {
    /// Enter vi mode.
    Enter,
    /// Leave vi mode.
    Exit,
}
