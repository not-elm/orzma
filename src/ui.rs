//! Bevy UI Plugin and shared UI markers. Spawns the singleton `UiRoot` /
//! `WorkspaceUiRoot` Node tree (via `OzmuxUiRootPlugin`) that the tmux render
//! layer attaches its window container under.

use crate::ui::root::OzmuxUiRootPlugin;
use bevy::prelude::*;

pub(crate) mod confirm_prompt;
pub mod copy_mode;
pub mod copy_mode_indicator;
pub(crate) mod copy_search;
pub(crate) mod ime_overlay;
pub mod palette;
pub(crate) mod rename_prompt;
pub mod root;

/// Marker for the single root UI Node entity. Spawned once in Startup,
/// never despawned. Hosts `WorkspaceUiRoot` (the tmux window container's
/// attachment point) and the tmux window status bar (`WindowBarRoot`) as
/// direct children.
#[derive(Component)]
pub struct UiRoot;

/// Marker for the single attachment-point `Node` child of `UiRoot` under
/// which the tmux render layer parents its window container. Spawned once in
/// Startup; never despawned.
#[derive(Component)]
pub struct WorkspaceUiRoot;

/// Bevy Plugin spawning the singleton UI root Node tree.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(OzmuxUiRootPlugin);
    }
}
