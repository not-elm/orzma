//! The `Mux` aggregate: owns every slotmap and the active pointers, and
//! exposes the mutation API (each op returns `Vec<MuxEvent>`) plus queries.

use crate::error::{MuxError, MuxResult};
use crate::id::{NodeId, PaneId, SessionId, SplitId, SurfaceId, WorkspaceId};
use crate::surface::{Surface, SurfaceKind};
use crate::tree::{Pane, Split};
use slotmap::SlotMap;

#[allow(dead_code)]
struct Session {
    workspaces: Vec<WorkspaceId>,
    active: WorkspaceId,
}

#[allow(dead_code)]
struct Workspace {
    root: NodeId,
    active_pane: PaneId,
    name: String,
    created_at: u32,
    size: Option<(u16, u16)>,
}

/// The multiplexer aggregate root.
pub struct Mux {
    sessions: SlotMap<SessionId, Session>,
    active_session: SessionId,
    workspaces: SlotMap<WorkspaceId, Workspace>,
    splits: SlotMap<SplitId, Split>,
    panes: SlotMap<PaneId, Pane>,
    surfaces: SlotMap<SurfaceId, Surface>,
    #[allow(dead_code)]
    name_counter: u32,
}

impl Default for Mux {
    fn default() -> Self {
        Self::new()
    }
}

impl Mux {
    /// Builds the initial state: one default session with one workspace
    /// holding a single terminal pane. Active pointers are valid
    /// immediately. (Initial state is conveyed as a snapshot, so no events.)
    pub fn new() -> Self {
        let mut mux = Mux {
            sessions: SlotMap::with_key(),
            active_session: SessionId::default(),
            workspaces: SlotMap::with_key(),
            splits: SlotMap::with_key(),
            panes: SlotMap::with_key(),
            surfaces: SlotMap::with_key(),
            name_counter: 0,
        };
        let surface = mux.surfaces.insert(Surface {
            kind: SurfaceKind::Terminal,
            cwd: None,
        });
        let pane = mux.panes.insert(Pane {
            surfaces: vec![surface],
            active_surface: surface,
            parent: None,
        });
        let created_at = mux.name_counter;
        mux.name_counter += 1;
        let workspace = mux.workspaces.insert(Workspace {
            root: NodeId::Pane(pane),
            active_pane: pane,
            name: format!("{created_at}"),
            created_at,
            size: None,
        });
        let session = mux.sessions.insert(Session {
            workspaces: vec![workspace],
            active: workspace,
        });
        mux.active_session = session;
        mux
    }

    /// The active session's active workspace.
    pub fn active_workspace(&self) -> WorkspaceId {
        self.sessions[self.active_session].active
    }

    /// The workspace's focused pane.
    pub fn active_pane(&self, workspace: WorkspaceId) -> MuxResult<PaneId> {
        Ok(self.workspace(workspace)?.active_pane)
    }

    /// The pane's focused surface.
    pub fn active_surface(&self, pane: PaneId) -> MuxResult<SurfaceId> {
        Ok(self.pane(pane)?.active_surface)
    }

    /// A pane's surfaces in creation order.
    pub fn surfaces(&self, pane: PaneId) -> MuxResult<Vec<SurfaceId>> {
        Ok(self.pane(pane)?.surfaces.clone())
    }

    /// A surface's kind.
    pub fn surface_kind(&self, surface: SurfaceId) -> MuxResult<SurfaceKind> {
        Ok(self.surface(surface)?.kind.clone())
    }

    /// Every pane in a workspace, DFS first-child-first (matches the old
    /// `ordered_panes`): leftmost leaf first.
    pub fn ordered_panes(&self, workspace: WorkspaceId) -> MuxResult<Vec<PaneId>> {
        let root = self.workspace(workspace)?.root;
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node {
                NodeId::Pane(p) => out.push(p),
                NodeId::Split(s) => {
                    let split = &self.splits[s];
                    // NOTE: push second THEN first so first pops (and is visited) first,
                    // preserving DFS leftmost-leaf-first order that layout.rs relies on.
                    stack.push(split.second);
                    stack.push(split.first);
                }
            }
        }
        Ok(out)
    }

    fn workspace(&self, id: WorkspaceId) -> MuxResult<&Workspace> {
        self.workspaces
            .get(id)
            .ok_or(MuxError::WorkspaceNotFound(id))
    }

    fn pane(&self, id: PaneId) -> MuxResult<&Pane> {
        self.panes.get(id).ok_or(MuxError::PaneNotFound(id))
    }

    fn surface(&self, id: SurfaceId) -> MuxResult<&Surface> {
        self.surfaces.get(id).ok_or(MuxError::SurfaceNotFound(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::PaneId;
    use crate::surface::SurfaceKind;

    #[test]
    fn new_seeds_one_session_workspace_pane_surface() {
        let mux = Mux::new();
        let ws = mux.active_workspace();
        let panes = mux.ordered_panes(ws).unwrap();
        assert_eq!(panes.len(), 1);
        let pane = panes[0];
        assert_eq!(mux.surfaces(pane).unwrap().len(), 1);
        assert_eq!(mux.active_pane(ws).unwrap(), pane);
        let surface = mux.active_surface(pane).unwrap();
        assert!(matches!(
            mux.surface_kind(surface).unwrap(),
            SurfaceKind::Terminal
        ));
    }

    #[test]
    fn stale_pane_id_is_pane_not_found() {
        let mux = Mux::new();
        assert_eq!(
            mux.surfaces(PaneId::default()),
            Err(MuxError::PaneNotFound(PaneId::default()))
        );
    }
}
