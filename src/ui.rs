//! Bevy UI Plugin and shared UI markers. Spawns the singleton `UiRoot` Node
//! (via `OrzmaUiRootPlugin`) that the UI subtree attaches under.

use crate::ui::root::OrzmaUiRootPlugin;
use bevy::prelude::*;

pub mod root;

mod ime_overlay;
mod shell_surface;
mod vi_mode_indicator;

pub(crate) use shell_surface::ShellSurfaceUi;

/// Marker for the single root UI Node entity. Spawned once in Startup, never
/// despawned. Hosts the `ShellSurfaceUi` subtree as a child.
#[derive(Component)]
pub struct UiRoot;

/// Aggregates the UI plugins: the root Node tree, the shell-surface subtree,
/// the IME overlay, and the vi-mode indicator.
pub struct OrzmaUiPlugin;

impl Plugin for OrzmaUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            OrzmaUiRootPlugin,
            shell_surface::ShellSurfacePlugin,
            ime_overlay::ImeOverlayPlugin,
            vi_mode_indicator::ViModeIndicatorPlugin,
        ));
    }
}
