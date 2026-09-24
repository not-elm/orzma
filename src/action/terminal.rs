//! Per-command PTY-level terminal action events: mode-neutral apply observers
//! that trigger `bevy_orzmux` requests against a terminal surface. This
//! root aggregates their per-file plugins.

mod open_uri;
mod selection;

use crate::action::terminal::{open_uri::OpenUriPlugin, selection::SelectionPlugin};
use bevy::prelude::*;

pub(crate) use open_uri::TerminalOpenUri;
#[cfg(test)]
pub(crate) use selection::TerminalSelectionCopy;
pub(crate) use selection::trigger_selection_copy;

/// Aggregates the per-command terminal action plugins.
pub(super) struct TerminalActionPlugin;

impl Plugin for TerminalActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((OpenUriPlugin, SelectionPlugin));
    }
}
