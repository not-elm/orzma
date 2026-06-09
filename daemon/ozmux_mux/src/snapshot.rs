//! Full-state dump for cold-attach: a client builds its mirror from this,
//! then streams MuxEvent deltas.

use crate::id::{PaneId, SessionId, SurfaceId, WorkspaceId};
use crate::surface::SurfaceKind;
use crate::tree::LayoutNode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A snapshot of one session's full state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// The session.
    pub session: SessionId,
    /// The session's active workspace.
    pub active_workspace: WorkspaceId,
    /// Each workspace's full state.
    pub workspaces: Vec<WorkspaceSnapshot>,
}

/// One workspace's snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// Its name.
    pub name: String,
    /// Its layout tree.
    pub layout: LayoutNode,
    /// Its resolved viewport in cells (None before the first size).
    pub size: Option<(u16, u16)>,
    /// Its active pane.
    pub active_pane: PaneId,
    /// Per-pane surface state.
    pub panes: Vec<PaneSnapshot>,
}

/// One pane's surfaces (the layout tree only carries the active kind).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    /// The pane.
    pub pane: PaneId,
    /// Its surfaces (creation order).
    pub surfaces: Vec<SurfaceState>,
    /// Its active surface.
    pub active_surface: SurfaceId,
}

/// One surface's state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceState {
    /// The surface.
    pub surface: SurfaceId,
    /// Its kind.
    pub kind: SurfaceKind,
    /// Its working directory.
    pub cwd: PathBuf,
}
