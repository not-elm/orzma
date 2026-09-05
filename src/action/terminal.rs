//! Per-command PTY-level terminal action events: mode-neutral apply observers
//! that trigger `bevy_orzma_tty` requests against a terminal surface. This
//! root aggregates their per-file plugins.

mod open_uri;
mod selection;
mod viewport_scroll;

use crate::action::terminal::{
    open_uri::OpenUriPlugin, selection::SelectionPlugin, viewport_scroll::ViewportScrollPlugin,
};
use bevy::prelude::*;

pub(crate) use open_uri::TerminalOpenUri;
pub(crate) use selection::{
    TerminalSelectionClear, TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate,
    copy_selection_of, trigger_selection_copy,
};
pub(crate) use viewport_scroll::TerminalViewportScroll;

/// Aggregates the per-command terminal action plugins.
pub(super) struct TerminalActionPlugin;

impl Plugin for TerminalActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((OpenUriPlugin, SelectionPlugin, ViewportScrollPlugin));
    }
}
