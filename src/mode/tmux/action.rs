//! Per-command tmux action events: each ozmux shortcut that drives a tmux
//! command has one `EntityEvent` + apply observer module under
//! `src/mode/tmux/action/`. This root aggregates their per-file plugins.

mod select_pane;
mod split_pane;
mod zoom_pane;

use bevy::prelude::*;

#[allow(unused_imports)]
pub(crate) use select_pane::SelectPaneRequest;
#[allow(unused_imports)]
pub(crate) use split_pane::SplitPaneRequest;
#[allow(unused_imports)]
pub(crate) use zoom_pane::ZoomPaneRequest;

/// Aggregates the per-command tmux action plugins.
pub(crate) struct TmuxActionPlugin;

impl Plugin for TmuxActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            split_pane::SplitPanePlugin,
            select_pane::SelectPanePlugin,
            zoom_pane::ZoomPanePlugin,
        ));
    }
}
