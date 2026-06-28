//! Bevy UI Plugin and shared UI markers. Spawns the singleton `UiRoot` Node
//! (via `OzmuxUiRootPlugin`) that each mode's UI subtree attaches under.

use crate::ui::root::OzmuxUiRootPlugin;
use bevy::prelude::*;

pub mod copy_mode;
pub mod copy_mode_indicator;
pub(crate) mod copy_search;
pub(crate) mod ime_overlay;
pub mod palette;
pub mod root;

/// Marker for the single root UI Node entity. Spawned once in Startup, never
/// despawned. Hosts each mode's UI subtree (`DefaultModeUi` / `TmuxModeUi`) as a
/// child while that mode is active.
#[derive(Component)]
pub struct UiRoot;

/// Bevy Plugin spawning the singleton UI root Node tree.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(OzmuxUiRootPlugin);
    }
}
