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
    pub active_workspace: Option<WorkspaceId>,
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
    pub active_pane: Option<PaneId>,
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
    pub active_surface: Option<SurfaceId>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::Mux;
    use crate::surface::SurfaceKind;
    use crate::tree::{Side, SplitOrientation, collect_node_ids};
    use std::collections::HashSet;

    #[test]
    fn session_snapshot_serde_round_trips() {
        let mut mux = Mux::new();
        let session = mux.sessions()[0];
        let ws = mux.active_workspace();
        mux.set_workspace_size(ws, 80, 24).unwrap();
        let pane = mux.active_pane(ws).unwrap();
        mux.split_pane(
            pane,
            SplitOrientation::Horizontal,
            Side::After,
            SurfaceKind::Terminal,
        )
        .unwrap();

        let snap = mux.snapshot(session).unwrap();
        let json = serde_json::to_string(&snap).unwrap();
        let back: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
    }

    #[test]
    fn snapshot_reflects_tree() {
        let mut mux = Mux::new();
        let session = mux.sessions()[0];
        mux.new_workspace().unwrap();
        let snap = mux.snapshot(session).unwrap();
        assert_eq!(snap.workspaces.len(), 2, "two workspaces in session");

        let ws_snap = &snap.workspaces[0];
        assert_eq!(ws_snap.panes.len(), 1, "first workspace has one pane");
        assert_eq!(ws_snap.panes[0].surfaces.len(), 1, "pane has one surface");
    }

    #[test]
    fn collect_node_ids_covers_split_and_panes() {
        let mut mux = Mux::new();
        let ws = mux.active_workspace();
        let pane = mux.active_pane(ws).unwrap();
        mux.set_workspace_size(ws, 80, 24).unwrap();
        mux.split_pane(
            pane,
            SplitOrientation::Horizontal,
            Side::After,
            SurfaceKind::Terminal,
        )
        .unwrap();

        let layout = mux.workspace_layout(ws).unwrap();
        let mut ids = HashSet::new();
        collect_node_ids(&layout, &mut ids);

        let (split_count, pane_count) =
            ids.iter()
                .fold((0usize, 0usize), |(s, p), node| match node {
                    crate::id::NodeId::Split(_) => (s + 1, p),
                    crate::id::NodeId::Pane(_) => (s, p + 1),
                });
        assert_eq!(split_count, 1, "one split node");
        assert_eq!(pane_count, 2, "two pane nodes");
    }
}
