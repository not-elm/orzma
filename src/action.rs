//! Aggregates the ozmux shortcut-action plugins that dispatch through
//! `EntityEvent`s: workspace-lifecycle actions and pane/surface actions.

use bevy::prelude::*;
use close_pane::ClosePaneActionPlugin;
use close_surface::CloseSurfaceActionPlugin;
use focus_pane::FocusPaneActionPlugin;
use focus_surface::FocusSurfaceActionPlugin;
use new_terminal_surface::NewTerminalSurfaceActionPlugin;
use workspace::OzmuxWorkspaceActionPlugin;
use split_pane::SplitPaneActionPlugin;
use swap_pane::SwapPaneActionPlugin;

pub(crate) mod close_pane;
pub(crate) mod close_surface;
pub(crate) mod focus_pane;
pub(crate) mod focus_surface;
pub(crate) mod new_terminal_surface;
pub(crate) mod workspace;
pub(crate) mod split_pane;
pub(crate) mod swap_pane;

/// Bevy Plugin that registers every action-dispatch sub-plugin under the
/// `action` module.
pub(crate) struct OzmuxActionPlugin;

impl Plugin for OzmuxActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            OzmuxWorkspaceActionPlugin,
            SplitPaneActionPlugin,
            NewTerminalSurfaceActionPlugin,
            FocusPaneActionPlugin,
            FocusSurfaceActionPlugin,
            SwapPaneActionPlugin,
            ClosePaneActionPlugin,
            CloseSurfaceActionPlugin,
        ));
    }
}
