//! Mirrors an `ozmux_mux::Mux` into the Bevy ECS: the `MuxState` Resource,
//! `MuxId` forward-lookup components, the apply-handler that turns
//! `MuxEvent`s into ECS mutations, and a consistency checker. Plan 2b-1
//! builds this as library code; the source-of-truth flip is Plan 2b-2.

use bevy::prelude::*;
use ozmux_mux::{PaneId, SplitId, SurfaceId, WorkspaceId};
use slotmap::SecondaryMap;

/// Authoritative `Mux` plus the reverse maps (`MuxId` → `Entity`). Forward
/// lookup (`Entity` → `MuxId`) is the `Mux*Id` components below.
#[derive(Resource)]
pub struct MuxState {
    /// The Bevy-free multiplexer core (Plan 2b-1: shadow only; 2b-2: authoritative).
    pub mux: ozmux_mux::Mux,
    #[expect(dead_code, reason = "consumed by apply-handler in Plan 2b-1 Tasks 2-5")]
    pub(crate) workspaces: SecondaryMap<WorkspaceId, Entity>,
    #[expect(dead_code, reason = "consumed by apply-handler in Plan 2b-1 Tasks 2-5")]
    pub(crate) panes: SecondaryMap<PaneId, Entity>,
    #[expect(dead_code, reason = "consumed by apply-handler in Plan 2b-1 Tasks 2-5")]
    pub(crate) splits: SecondaryMap<SplitId, Entity>,
    #[expect(dead_code, reason = "consumed by apply-handler in Plan 2b-1 Tasks 2-5")]
    pub(crate) surfaces: SecondaryMap<SurfaceId, Entity>,
    /// The GUI layout-root container entity per workspace (not a `Mux` node).
    #[expect(dead_code, reason = "consumed by apply-handler in Plan 2b-1 Tasks 2-5")]
    pub(crate) layout_roots: SecondaryMap<WorkspaceId, Entity>,
}

/// Forward lookup `Entity` → `WorkspaceId`.
#[derive(Component, Clone, Copy)]
pub struct MuxWorkspaceId(pub WorkspaceId);

/// Forward lookup `Entity` → `PaneId`.
#[derive(Component, Clone, Copy)]
pub struct MuxPaneId(pub PaneId);

/// Forward lookup `Entity` → `SplitId`.
#[derive(Component, Clone, Copy)]
pub struct MuxSplitId(pub SplitId);

/// Forward lookup `Entity` → `SurfaceId`.
#[derive(Component, Clone, Copy)]
pub struct MuxSurfaceId(pub SurfaceId);

impl MuxState {
    /// Creates a `MuxState` wrapping `mux` with empty reverse maps. Callers
    /// then run `materialize_snapshot` (Task 2) to realize the tree.
    pub fn new(mux: ozmux_mux::Mux) -> Self {
        Self {
            mux,
            workspaces: SecondaryMap::new(),
            panes: SecondaryMap::new(),
            splits: SecondaryMap::new(),
            surfaces: SecondaryMap::new(),
            layout_roots: SecondaryMap::new(),
        }
    }
}
