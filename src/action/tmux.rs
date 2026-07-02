//! Per-command tmux action events: each ozmux shortcut that drives a tmux
//! command has one `EntityEvent` + apply observer module under
//! `src/mode/tmux/action/`. This root aggregates their per-file plugins.

mod enter_copy_mode;
mod kill_pane;
mod kill_window;
mod new_window;
mod next_window;
mod previous_window;
mod rename_session;
mod rename_window;
mod select_pane;
mod select_window;
mod split_pane;
mod zoom_pane;

use bevy::prelude::*;

pub(crate) use enter_copy_mode::EnterCopyModeRequest;
pub(crate) use kill_pane::KillPaneRequest;
pub(crate) use kill_window::KillWindowRequest;
pub(crate) use new_window::NewWindowRequest;
pub(crate) use next_window::NextWindowRequest;
pub(crate) use previous_window::PreviousWindowRequest;
pub(crate) use rename_session::RenameSessionRequest;
pub(crate) use rename_window::RenameWindowRequest;
pub(crate) use select_pane::SelectPaneRequest;
pub(crate) use select_window::SelectWindowRequest;
pub(crate) use split_pane::SplitPaneRequest;
pub(crate) use zoom_pane::ZoomPaneRequest;

/// Aggregates the per-command tmux action plugins.
pub(super) struct TmuxActionPlugin;

impl Plugin for TmuxActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            enter_copy_mode::EnterCopyModePlugin,
            kill_pane::KillPanePlugin,
            kill_window::KillWindowPlugin,
            new_window::NewWindowPlugin,
            next_window::NextWindowPlugin,
            previous_window::PreviousWindowPlugin,
            rename_session::RenameSessionPlugin,
            rename_window::RenameWindowPlugin,
            select_pane::SelectPanePlugin,
            select_window::SelectWindowPlugin,
            split_pane::SplitPanePlugin,
            zoom_pane::ZoomPanePlugin,
        ));
    }
}
