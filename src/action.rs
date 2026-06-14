//! Aggregates the ozmux shortcut-action plugins that dispatch through
//! `EntityEvent`s: workspace-lifecycle actions and pane/surface actions.

use bevy::prelude::*;
use close_pane::ClosePaneActionPlugin;
use focus_pane::FocusPaneActionPlugin;
use split_pane::SplitPaneActionPlugin;
use swap_pane::SwapPaneActionPlugin;
use workspace::OzmuxWorkspaceActionPlugin;

pub(crate) mod close_pane;
pub(crate) mod focus_pane;
pub(crate) mod split_pane;
pub(crate) mod swap_pane;
pub(crate) mod workspace;

/// Bevy Plugin that registers every action-dispatch sub-plugin under the
/// `action` module.
pub(crate) struct OzmuxActionPlugin;

impl Plugin for OzmuxActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            OzmuxWorkspaceActionPlugin,
            SplitPaneActionPlugin,
            FocusPaneActionPlugin,
            SwapPaneActionPlugin,
            ClosePaneActionPlugin,
        ));
    }
}
