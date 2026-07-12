//! Bevy UI Plugin and shared UI markers. Spawns the singleton `UiRoot` Node
//! (via `OrzmaUiRootPlugin`) that the UI subtree attaches under.

use crate::ui::root::OrzmaUiRootPlugin;
use bevy::prelude::*;

pub(crate) mod default_mode;
pub(crate) mod ime_overlay;
pub mod palette;
pub mod root;
pub mod vi_mode;
pub mod vi_mode_indicator;

/// Marker for the single root UI Node entity. Spawned once in Startup, never
/// despawned. Hosts the `DefaultModeUi` subtree as a child.
#[derive(Component)]
pub struct UiRoot;

/// Bevy Plugin spawning the singleton UI root Node tree.
pub struct OrzmaUiPlugin;

impl Plugin for OrzmaUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((OrzmaUiRootPlugin, default_mode::DefaultModeUiPlugin));
    }
}
