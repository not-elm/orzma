//! Bevy UI Plugin and shared UI markers. Spawns the singleton `UiRoot` /
//! `WorkspaceUiRoot` Node tree (via `OzmuxUiRootPlugin`) that the tmux render
//! layer attaches its window container under. Shared markers (`Slotted`,
//! `TerminalSurfaceMarker`) and the `HomeDir` resource live here.

use crate::ui::root::OzmuxUiRootPlugin;
use bevy::prelude::*;
use std::path::PathBuf;

pub(crate) mod confirm_prompt;
pub mod copy_mode;
pub mod copy_mode_indicator;
pub(crate) mod copy_search;
pub(crate) mod ime_overlay;
pub mod palette;
pub mod root;
pub(crate) mod tmux_dialog;
pub(crate) mod tmux_divider_handle;
pub(crate) mod tmux_pane_focus;
pub(crate) mod tmux_window_bar;
pub(crate) mod tmux_window_bar_input;

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

/// Marks the Surface entity currently slotted into its pane's visible
/// `surface_slot` (i.e. the active surface). Inactive surfaces are parked
/// under a non-`Node` parent and keep this marker removed.
///
/// # Invariants
///
/// Geometric hit-tests (`resolve_pane_at_phys`) MUST filter on this marker:
/// a parked surface is excluded from layout, so its `ComputedNode` retains
/// stale, often window-sized geometry. Without this filter a click resolves
/// to a parked surface of an already-active pane and focus never moves.
#[derive(Component)]
pub struct Slotted;

/// Marks a terminal Surface entity. Queried with `With<TerminalSurfaceMarker>`
/// to find surfaces that need a `TerminalBundle` + `TerminalRenderBundle`
/// attached.
#[derive(Component)]
pub struct TerminalSurfaceMarker;

/// Resolved `$HOME` at startup (`None` if unset). Used to home-abbreviate
/// terminal paths; the value matches the terminal spawner's `$HOME` fallback
/// so the path agrees with where the shell started.
#[derive(Resource)]
pub(crate) struct HomeDir(pub(crate) Option<PathBuf>);

/// Bevy Plugin spawning the singleton UI root Node tree.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(HomeDir(std::env::var_os("HOME").map(PathBuf::from)))
            .add_plugins(OzmuxUiRootPlugin);
    }
}
