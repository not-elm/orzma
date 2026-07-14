//! Bevy UI Plugin and shared UI markers. Spawns the singleton `UiRoot` Node
//! (via `OrzmaUiRootPlugin`) that the UI subtree attaches under.

use crate::ui::root::OrzmaUiRootPlugin;
use bevy::prelude::*;

mod ime_overlay;
pub(crate) mod multiplexer;
pub mod root;
mod vi_mode_indicator;

/// Marker for the single root UI Node entity. Spawned once in Startup, never
/// despawned. Hosts the multiplexer UI subtree (window bar + workspace) as a
/// child.
#[derive(Component)]
pub struct UiRoot;

/// Bevy Plugin spawning the singleton UI root Node tree.
pub struct OrzmaUiPlugin;

impl Plugin for OrzmaUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            OrzmaUiRootPlugin,
            multiplexer::MultiplexerUiPlugin,
            ime_overlay::ImeOverlayPlugin,
            vi_mode_indicator::ViModeIndicatorPlugin,
        ));
    }
}
