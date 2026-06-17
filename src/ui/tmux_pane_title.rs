//! Per-pane title bar: `PaneTitleBar` marker and the plugin that keeps it in sync.

use bevy::prelude::*;

/// Marker on the title-bar child entity that sits at the top of each `TmuxPane`
/// container.
#[derive(Component)]
pub(crate) struct PaneTitleBar;

/// Stub plugin. Systems are added in Task 8.
pub(crate) struct OzmuxTmuxPaneTitlePlugin;

impl Plugin for OzmuxTmuxPaneTitlePlugin {
    fn build(&self, _app: &mut App) {}
}
