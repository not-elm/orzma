//! Per-command PTY-level terminal action events: mode-neutral apply observers
//! that write to a terminal surface's handle, backend, or the clipboard. This
//! root aggregates their per-file plugins.

mod paste;

use bevy::prelude::*;

pub(crate) use paste::PasteAction;

/// Aggregates the per-command terminal action plugins.
pub(super) struct TerminalActionPlugin;

impl Plugin for TerminalActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(paste::PastePlugin);
    }
}
