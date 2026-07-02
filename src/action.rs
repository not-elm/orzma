//! The action layer: per-command `EntityEvent`s and their apply observers,
//! grouped by domain (tmux pane/window ops, shared VI copy-mode ops).

pub(crate) mod tmux;

use bevy::prelude::*;

/// Aggregates the action-layer plugins.
pub(crate) struct ActionPlugin;

impl Plugin for ActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(tmux::TmuxActionPlugin);
    }
}
