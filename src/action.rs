//! Aggregates the ozmux shortcut-action plugins that dispatch through
//! `EntityEvent`s: session-lifecycle actions and pane/activity actions.

use bevy::prelude::*;
use close_activity::CloseActivityActionPlugin;
use close_pane::ClosePaneActionPlugin;
use focus_activity::FocusActivityActionPlugin;
use focus_pane::FocusPaneActionPlugin;
use new_terminal_activity::NewTerminalActivityActionPlugin;
use session::OzmuxSessionActionPlugin;
use split_pane::SplitPaneActionPlugin;
use swap_pane::SwapPaneActionPlugin;

pub(crate) mod close_activity;
pub(crate) mod close_pane;
pub(crate) mod focus_activity;
pub(crate) mod focus_pane;
pub(crate) mod new_terminal_activity;
pub(crate) mod session;
pub(crate) mod split_pane;
pub(crate) mod swap_pane;

/// Bevy Plugin that registers every action-dispatch sub-plugin under the
/// `action` module.
pub(crate) struct OzmuxActionPlugin;

impl Plugin for OzmuxActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            OzmuxSessionActionPlugin,
            SplitPaneActionPlugin,
            NewTerminalActivityActionPlugin,
            FocusPaneActionPlugin,
            FocusActivityActionPlugin,
            SwapPaneActionPlugin,
            ClosePaneActionPlugin,
            CloseActivityActionPlugin,
        ));
    }
}
