//! Per-command tmux action events: each orzma shortcut that drives a tmux
//! command has one `EntityEvent` + apply observer module under
//! `src/action/tmux/`. This root aggregates their per-file plugins.

mod detach_session;
mod kill_pane;
mod kill_window;
mod new_window;
mod next_window;
mod paste;
mod previous_window;
mod rename_session;
mod rename_window;
mod resize_pane;
mod select_pane;
mod select_window;
mod split_pane;
mod zoom_pane;

use bevy::prelude::*;

pub(crate) use detach_session::DetachSessionRequest;
pub(crate) use kill_pane::KillPaneRequest;
pub(crate) use kill_window::KillWindowRequest;
pub(crate) use new_window::NewWindowRequest;
pub(crate) use next_window::NextWindowRequest;
pub(crate) use previous_window::PreviousWindowRequest;
pub(crate) use rename_session::RenameSessionRequest;
pub(crate) use rename_window::RenameWindowRequest;
pub(crate) use resize_pane::ResizePaneRequest;
pub(crate) use select_pane::SelectPaneRequest;
pub(crate) use select_window::SelectWindowRequest;
pub(crate) use split_pane::SplitPaneRequest;
pub(crate) use zoom_pane::ZoomPaneRequest;

/// Aggregates the per-command tmux action plugins.
pub(super) struct TmuxActionPlugin;

impl Plugin for TmuxActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            detach_session::DetachSessionPlugin,
            kill_pane::KillPanePlugin,
            kill_window::KillWindowPlugin,
            new_window::NewWindowPlugin,
            next_window::NextWindowPlugin,
            paste::TmuxPastePlugin,
            previous_window::PreviousWindowPlugin,
            rename_session::RenameSessionPlugin,
            rename_window::RenameWindowPlugin,
            resize_pane::ResizePanePlugin,
            select_pane::SelectPanePlugin,
            select_window::SelectWindowPlugin,
            split_pane::SplitPanePlugin,
            zoom_pane::ZoomPanePlugin,
        ));
    }
}
