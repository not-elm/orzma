//! Per-command tmux action events: each ozmux shortcut that drives a tmux
//! command has one `EntityEvent` + apply observer module under
//! `src/mode/tmux/action/`. This root aggregates their per-file plugins.

mod kill_pane;
mod kill_window;
mod new_window;
mod next_window;
mod previous_window;
mod select_pane;
mod select_window;
mod split_pane;
mod zoom_pane;

use bevy::prelude::*;

#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use kill_pane::KillPaneRequest;
#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use kill_window::KillWindowRequest;
#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use new_window::NewWindowRequest;
#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use next_window::NextWindowRequest;
#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use previous_window::PreviousWindowRequest;
#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use select_pane::SelectPaneRequest;
#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use select_window::SelectWindowRequest;
#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use split_pane::SplitPaneRequest;
#[expect(
    unused_imports,
    reason = "consumed by Task 7's shortcut dispatch wiring"
)]
pub(crate) use zoom_pane::ZoomPaneRequest;

/// Aggregates the per-command tmux action plugins.
pub(crate) struct TmuxActionPlugin;

impl Plugin for TmuxActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            kill_pane::KillPanePlugin,
            kill_window::KillWindowPlugin,
            new_window::NewWindowPlugin,
            next_window::NextWindowPlugin,
            previous_window::PreviousWindowPlugin,
            select_pane::SelectPanePlugin,
            select_window::SelectWindowPlugin,
            split_pane::SplitPanePlugin,
            zoom_pane::ZoomPanePlugin,
        ));
    }
}
