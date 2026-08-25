//! Vi mode: the cursor it adds and the switch that enters or leaves it.

use crate::screen::grid::coords::GridPoint;

/// Vi-mode cursor position in active-grid coordinates.
///
/// The line goes negative while the vi cursor sits in scrollback
/// history. The sign is not a visibility signal — scrolling clamps
/// the vi cursor into the viewport, so a negative line can still be
/// visible; project `point` with
/// [`crate::prelude::GridLine::to_viewport`] to decide whether there
/// is a cell to paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViCursor {
    /// Grid cell the vi cursor sits on.
    pub point: GridPoint,
}

/// The direction of a vi-mode switch.
///
/// Named variants rather than a `bool` so the intent is readable at the
/// trigger site, where a bare `true` says nothing about which state it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViModeSwitch {
    /// Enter vi mode: the vi cursor starts tracking and keyboard input is
    /// interpreted as motions rather than forwarded to the PTY.
    Enter,
    /// Leave vi mode and snap the viewport back to the live tail.
    Exit,
}
