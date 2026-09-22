//! The action layer: per-command events and their apply observers, grouped by
//! domain (vi mode: state, keymap, and ops; PTY-level terminal ops; clipboard;
//! font-size zoom). An event targets an entity unless it is window-wide.

pub(crate) mod clipboard;
pub(crate) mod font_zoom;
pub(crate) mod terminal;
pub(crate) mod vi;

use crate::action::{
    clipboard::ClipboardActionsPlugin, font_zoom::FontZoomPlugin, terminal::TerminalActionPlugin,
    vi::ViActionPlugin,
};
use bevy::prelude::*;

/// Aggregates the action-layer plugins.
pub(crate) struct ActionPlugin;

impl Plugin for ActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            TerminalActionPlugin,
            ViActionPlugin,
            ClipboardActionsPlugin,
            FontZoomPlugin,
        ));
    }
}
