//! The action layer: per-command `EntityEvent`s and their apply observers,
//! grouped by domain (tmux pane/window ops, shared vi-mode ops,
//! PTY-level terminal ops).

pub mod clipboard;
pub(crate) mod terminal;
pub(crate) mod tmux;
pub(crate) mod vi;

use crate::action::{
    clipboard::ClipboardActionsPlugin, terminal::TerminalActionPlugin, tmux::TmuxActionPlugin,
    vi::ViActionPlugin,
};
use bevy::prelude::*;

/// Aggregates the action-layer plugins.
pub(crate) struct ActionPlugin;

impl Plugin for ActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            TerminalActionPlugin,
            TmuxActionPlugin,
            ViActionPlugin,
            ClipboardActionsPlugin,
        ));
    }
}
