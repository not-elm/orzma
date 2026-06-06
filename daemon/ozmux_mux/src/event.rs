//! Events returned by every `Mux` mutation — the single downstream channel
//! (daemon → UDS, Bevy mirror → ECS). Pure data; no VT `Frame`s (those are
//! `ozmux_vt`'s domain).

use crate::id::{NodeId, PaneId, SessionId, SplitId, SurfaceId, WorkspaceId};
use crate::surface::SurfaceKind;
use crate::tree::LayoutNode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One surface in a pane's creation manifest (self-sufficient for the wire).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceEntry {
    /// The surface.
    pub surface: SurfaceId,
    /// Its kind.
    pub kind: SurfaceKind,
    /// Its working directory.
    pub cwd: PathBuf,
}

/// A single state-change notification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MuxEvent {
    /// A session was created.
    SessionCreated {
        /// The new session.
        session: SessionId,
    },
    /// A workspace was created in a session.
    WorkspaceCreated {
        /// Owning session.
        session: SessionId,
        /// The new workspace.
        workspace: WorkspaceId,
        /// The workspace's name.
        name: String,
    },
    /// A workspace was destroyed.
    WorkspaceDestroyed {
        /// The removed workspace.
        workspace: WorkspaceId,
    },
    /// The active workspace changed.
    WorkspaceSelected {
        /// Owning session.
        session: SessionId,
        /// The now-active workspace.
        workspace: WorkspaceId,
    },
    /// A workspace was renamed.
    WorkspaceRenamed {
        /// The workspace.
        workspace: WorkspaceId,
        /// The new name.
        name: String,
    },
    /// A pane was created with its full surface manifest.
    PaneCreated {
        /// The new pane.
        pane: PaneId,
        /// Owning workspace.
        workspace: WorkspaceId,
        /// All surfaces the pane is created with (creation order).
        surfaces: Vec<SurfaceEntry>,
        /// The pane's initially-active surface.
        active_surface: SurfaceId,
    },
    /// A pane was closed.
    PaneClosed {
        /// The removed pane.
        pane: PaneId,
    },
    /// The focused pane changed.
    ActivePaneChanged {
        /// Owning workspace.
        workspace: WorkspaceId,
        /// The now-active pane.
        pane: PaneId,
    },
    /// A subtree was replaced: in `workspace`, replace the subtree at `root`
    /// with `subtree` (split/close partial diff).
    LayoutChanged {
        /// Owning workspace.
        workspace: WorkspaceId,
        /// The node to replace.
        root: NodeId,
        /// The replacement subtree.
        subtree: LayoutNode,
    },
    /// The whole workspace tree was replaced (collapse reached the root).
    WorkspaceRootChanged {
        /// Owning workspace.
        workspace: WorkspaceId,
        /// The new root subtree.
        root: LayoutNode,
    },
    /// A split's ratio changed (high-frequency drag/resize).
    LayoutRatioChanged {
        /// The split.
        split: SplitId,
        /// The new first-child fraction.
        ratio: f32,
    },
    /// A pane's resolved cell size changed.
    PaneResized {
        /// The pane.
        pane: PaneId,
        /// Resolved columns.
        cols: u16,
        /// Resolved rows.
        rows: u16,
    },
    /// A surface was added to a pane.
    SurfaceSpawned {
        /// Owning pane.
        pane: PaneId,
        /// The new surface.
        surface: SurfaceId,
        /// The surface's kind.
        kind: SurfaceKind,
    },
    /// A surface was removed.
    SurfaceClosed {
        /// The removed surface.
        surface: SurfaceId,
    },
    /// A pane's focused surface changed.
    ActiveSurfaceChanged {
        /// Owning pane.
        pane: PaneId,
        /// The now-active surface.
        surface: SurfaceId,
    },
    /// A surface's working directory changed.
    SurfaceCwdChanged {
        /// The surface.
        surface: SurfaceId,
        /// The new working directory.
        cwd: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::PaneId;

    #[test]
    fn muxevent_serde_round_trips() {
        let ev = MuxEvent::PaneClosed {
            pane: PaneId::default(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: MuxEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}
