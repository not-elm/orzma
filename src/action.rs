//! The action layer: per-command `EntityEvent`s and their apply observers,
//! grouped by domain (tmux pane/window ops, shared VI copy-mode ops,
//! PTY-level terminal ops).

pub(crate) mod terminal;
pub(crate) mod tmux;
pub(crate) mod vi;

use bevy::prelude::*;

/// Aggregates the action-layer plugins.
pub(crate) struct ActionPlugin;

impl Plugin for ActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            terminal::TerminalActionPlugin,
            tmux::TmuxActionPlugin,
            vi::ViActionPlugin,
        ));
    }
}
