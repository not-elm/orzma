//! Passive client-side mirror: rebuilds one session's state from a
//! `SessionSnapshot` + a `MuxEvent` stream, with NO `Mux` reference.
//! DTO-backed — it updates `SessionSnapshot`-shaped state in place and
//! derives layout liveness from its own tree via `ozmux_mux::collect_node_ids`.

use ozmux_mux::{
    LayoutNode, MuxEvent, NodeId, PaneId, PaneSnapshot, SessionSnapshot, SplitId, SurfaceState,
    WorkspaceId, WorkspaceSnapshot, collect_node_ids,
};
use std::collections::HashSet;

/// Client-side reconstruction of one session's state from a cold-attach
/// snapshot followed by a `MuxEvent` delta stream.
pub struct ClientMirror {
    session: SessionSnapshot,
}

impl ClientMirror {
    /// Builds a mirror from a cold-attach snapshot.
    pub fn from_snapshot(snapshot: SessionSnapshot) -> Self {
        Self { session: snapshot }
    }

    /// Folds one event into the mirror, pruning stale panes after layout changes.
    pub fn apply_event(&mut self, event: &MuxEvent) {
        self.apply_event_no_prune(event);
        // NOTE: pruning must happen only after layout-mutating events; pruning after
        // PaneCreated (split case) would drop the new pane before LayoutChanged adds
        // it to the tree.
        if matches!(
            event,
            MuxEvent::LayoutChanged { .. } | MuxEvent::WorkspaceRootChanged { .. }
        ) {
            for ws in &mut self.session.workspaces {
                prune_panes(ws);
                reorder_panes_to_layout(ws);
            }
        }
    }

    /// Folds a whole event batch, deferring pane-pruning to the end.
    ///
    /// A cross-parent swap removes a pane from one parent (event i) and
    /// re-adds it under another (event j > i); per-event pruning would drop
    /// its `PaneSnapshot` at i and never restore it.  Deferring prune to
    /// batch-end keeps the manifest for panes that disappear-then-reappear
    /// within the batch.
    pub fn apply_events(&mut self, events: &[MuxEvent]) {
        for ev in events {
            self.apply_event_no_prune(ev);
        }
        // NOTE: prune + reorder are only meaningful after a layout-mutating event
        // (the only way a pane leaves the layout or changes leaf order); gating on
        // it matches `apply_event` and skips the O(n^2) reorder on non-layout
        // batches (e.g. PaneResized storms during a window-resize drag).
        if events.iter().any(|e| {
            matches!(
                e,
                MuxEvent::LayoutChanged { .. } | MuxEvent::WorkspaceRootChanged { .. }
            )
        }) {
            for ws in &mut self.session.workspaces {
                prune_panes(ws);
                reorder_panes_to_layout(ws);
            }
        }
    }

    /// The current layout tree for `ws`, if present (no `to_snapshot` rebuild).
    pub fn workspace_layout(&self, ws: WorkspaceId) -> Option<&LayoutNode> {
        self.session
            .workspaces
            .iter()
            .find(|w| w.workspace == ws)
            .map(|w| &w.layout)
    }

    /// The root `NodeId` of `ws`'s layout, if present.
    pub fn workspace_root(&self, ws: WorkspaceId) -> Option<NodeId> {
        self.workspace_layout(ws).map(|layout| match layout {
            LayoutNode::Split { id, .. } => NodeId::Split(*id),
            LayoutNode::Pane { id, .. } => NodeId::Pane(*id),
        })
    }

    /// Returns the current reconstructed snapshot for comparison.
    pub fn to_snapshot(&self) -> SessionSnapshot {
        self.session.clone()
    }

    fn apply_event_no_prune(&mut self, event: &MuxEvent) {
        match event {
            MuxEvent::SessionCreated { .. } => {}

            MuxEvent::WorkspaceCreated {
                workspace, name, ..
            } => {
                let placeholder_pane_id = PaneId::default();
                self.session.workspaces.push(WorkspaceSnapshot {
                    workspace: *workspace,
                    name: name.clone(),
                    layout: LayoutNode::Pane {
                        id: placeholder_pane_id,
                        surface_kind: ozmux_mux::SurfaceKind::Terminal,
                        cols: 0,
                        rows: 0,
                    },
                    size: None,
                    active_pane: placeholder_pane_id,
                    panes: Vec::new(),
                });
            }

            MuxEvent::WorkspaceDestroyed { workspace } => {
                self.session
                    .workspaces
                    .retain(|ws| ws.workspace != *workspace);
            }

            MuxEvent::WorkspaceSelected { workspace, .. } => {
                self.session.active_workspace = *workspace;
            }

            MuxEvent::WorkspaceRenamed { workspace, name } => {
                if let Some(ws) = find_workspace_mut(&mut self.session.workspaces, *workspace) {
                    ws.name = name.clone();
                }
            }

            MuxEvent::PaneCreated {
                pane,
                workspace,
                surfaces,
                active_surface,
            } => {
                let pane_snap = PaneSnapshot {
                    pane: *pane,
                    surfaces: surfaces
                        .iter()
                        .map(|e| SurfaceState {
                            surface: e.surface,
                            kind: e.kind.clone(),
                            cwd: e.cwd.clone(),
                        })
                        .collect(),
                    active_surface: *active_surface,
                };
                if let Some(ws) = find_workspace_mut(&mut self.session.workspaces, *workspace) {
                    // NOTE: when the workspace's pane list is empty, this is the
                    // workspace's root pane (created by new_workspace). new_workspace
                    // emits no LayoutChanged, so we must establish the root layout here.
                    // A split's PaneCreated arrives on a non-empty workspace and is
                    // positioned by the subsequent LayoutChanged — do not overwrite there.
                    if ws.panes.is_empty() {
                        let surface_kind = surfaces
                            .iter()
                            .find(|e| e.surface == *active_surface)
                            .or_else(|| surfaces.first())
                            .map(|e| e.kind.clone())
                            .unwrap_or(ozmux_mux::SurfaceKind::Terminal);
                        ws.layout = LayoutNode::Pane {
                            id: *pane,
                            surface_kind,
                            cols: 0,
                            rows: 0,
                        };
                    }
                    ws.panes.push(pane_snap);
                }
            }

            MuxEvent::PaneClosed { pane } => {
                for ws in &mut self.session.workspaces {
                    ws.panes.retain(|p| p.pane != *pane);
                }
            }

            MuxEvent::ActivePaneChanged { workspace, pane } => {
                if let Some(ws) = find_workspace_mut(&mut self.session.workspaces, *workspace) {
                    ws.active_pane = *pane;
                }
            }

            MuxEvent::LayoutChanged {
                workspace,
                root,
                subtree,
            } => {
                if let Some(ws) = find_workspace_mut(&mut self.session.workspaces, *workspace) {
                    apply_layout_node(ws, *root, subtree.clone());
                }
            }

            MuxEvent::WorkspaceRootChanged { workspace, root } => {
                if let Some(ws) = find_workspace_mut(&mut self.session.workspaces, *workspace) {
                    ws.layout = root.clone();
                }
            }

            MuxEvent::LayoutRatioChanged { split, ratio } => {
                for ws in &mut self.session.workspaces {
                    set_split_ratio(&mut ws.layout, *split, *ratio);
                }
            }

            MuxEvent::WorkspaceResized {
                workspace,
                cols,
                rows,
            } => {
                if let Some(ws) = find_workspace_mut(&mut self.session.workspaces, *workspace) {
                    ws.size = Some((*cols, *rows));
                }
            }

            MuxEvent::PaneResized { pane, cols, rows } => {
                for ws in &mut self.session.workspaces {
                    set_pane_size(&mut ws.layout, *pane, *cols, *rows);
                }
            }

            MuxEvent::SurfaceSpawned {
                pane,
                surface,
                kind,
                cwd,
            } => {
                if let Some(pane_snap) = find_pane_mut(&mut self.session.workspaces, *pane) {
                    pane_snap.surfaces.push(SurfaceState {
                        surface: *surface,
                        kind: kind.clone(),
                        cwd: cwd.clone(),
                    });
                }
            }

            MuxEvent::SurfaceClosed { surface } => {
                for ws in &mut self.session.workspaces {
                    for pane_snap in &mut ws.panes {
                        pane_snap.surfaces.retain(|s| s.surface != *surface);
                    }
                }
            }

            MuxEvent::ActiveSurfaceChanged { pane, surface } => {
                if let Some(pane_snap) = find_pane_mut(&mut self.session.workspaces, *pane) {
                    pane_snap.active_surface = *surface;
                }
            }

            MuxEvent::SurfaceCwdChanged { surface, cwd } => {
                for ws in &mut self.session.workspaces {
                    for pane_snap in &mut ws.panes {
                        for surf in &mut pane_snap.surfaces {
                            if surf.surface == *surface {
                                surf.cwd = cwd.clone();
                            }
                        }
                    }
                }
            }

            // NOTE: `PaneCreated` (emitted first) already adds `surface` to `to_pane`'s
            // manifest; `SurfaceMoved` removes it from `from_pane`. Apply in emission order.
            MuxEvent::SurfaceMoved {
                surface,
                from_pane,
                to_pane: _,
            } => {
                if let Some(pane_snap) = find_pane_mut(&mut self.session.workspaces, *from_pane) {
                    pane_snap.surfaces.retain(|s| s.surface != *surface);
                }
            }
        }
    }
}

fn find_workspace_mut(
    workspaces: &mut [WorkspaceSnapshot],
    id: WorkspaceId,
) -> Option<&mut WorkspaceSnapshot> {
    workspaces.iter_mut().find(|ws| ws.workspace == id)
}

fn find_pane_mut(workspaces: &mut [WorkspaceSnapshot], id: PaneId) -> Option<&mut PaneSnapshot> {
    for ws in workspaces {
        if let Some(p) = ws.panes.iter_mut().find(|p| p.pane == id) {
            return Some(p);
        }
    }
    None
}

/// Recursively find the node matching `target` in `tree`'s children and
/// replace it with `sub`. Returns `true` if a replacement was made.
/// Does NOT replace the root itself — callers handle the root case separately.
fn replace_node(tree: &mut LayoutNode, target: NodeId, sub: LayoutNode) -> bool {
    match tree {
        LayoutNode::Split { first, second, .. } => {
            if try_replace_child(first, target, sub.clone()) {
                return true;
            }
            if try_replace_child(second, target, sub) {
                return true;
            }
            false
        }
        LayoutNode::Pane { .. } => false,
    }
}

/// Replace `child` (a `Box<LayoutNode>`) if its root matches `target`.
fn try_replace_child(child: &mut Box<LayoutNode>, target: NodeId, sub: LayoutNode) -> bool {
    let matches = match (child.as_ref(), target) {
        (LayoutNode::Split { id, .. }, NodeId::Split(tid)) => *id == tid,
        (LayoutNode::Pane { id, .. }, NodeId::Pane(tid)) => *id == tid,
        _ => false,
    };
    if matches {
        **child = sub;
        return true;
    }
    // Recurse.
    match child.as_mut() {
        LayoutNode::Split { first, second, .. } => {
            if try_replace_child(first, target, sub.clone()) {
                return true;
            }
            if try_replace_child(second, target, sub) {
                return true;
            }
            false
        }
        LayoutNode::Pane { .. } => false,
    }
}

/// Replace a split's ratio in the tree (recurse until the `SplitId` matches).
///
/// Clamps the ratio to `[0, 1]` and rescues `NaN` to `0.5`, matching
/// `Mux`'s `Split::set_ratio` behaviour.
fn set_split_ratio(tree: &mut LayoutNode, target: SplitId, ratio: f32) {
    if let LayoutNode::Split {
        id,
        ratio: r,
        first,
        second,
        ..
    } = tree
    {
        if *id == target {
            *r = if ratio.is_finite() {
                ratio.clamp(0.0, 1.0)
            } else {
                0.5
            };
        } else {
            set_split_ratio(first, target, ratio);
            set_split_ratio(second, target, ratio);
        }
    }
}

/// Update a leaf pane's resolved size in the tree.
fn set_pane_size(tree: &mut LayoutNode, target: PaneId, cols: u16, rows: u16) {
    match tree {
        LayoutNode::Pane {
            id,
            cols: c,
            rows: r,
            ..
        } => {
            if *id == target {
                *c = cols;
                *r = rows;
            }
        }
        LayoutNode::Split { first, second, .. } => {
            set_pane_size(first, target, cols, rows);
            set_pane_size(second, target, cols, rows);
        }
    }
}

/// Remove panes from a workspace's pane list if they are no longer in the
/// layout tree (checked via `collect_node_ids`).
fn prune_panes(ws: &mut WorkspaceSnapshot) {
    let mut live_ids = HashSet::new();
    collect_node_ids(&ws.layout, &mut live_ids);
    ws.panes
        .retain(|p| live_ids.contains(&NodeId::Pane(p.pane)));
}

/// Appends the DFS leaf-pane ids of `node` to `out` (left/top before
/// right/bottom), matching `Mux::ordered_panes`.
fn dfs_leaf_panes(node: &LayoutNode, out: &mut Vec<PaneId>) {
    match node {
        LayoutNode::Split { first, second, .. } => {
            dfs_leaf_panes(first, out);
            dfs_leaf_panes(second, out);
        }
        LayoutNode::Pane { id, .. } => out.push(*id),
    }
}

/// Reorders `ws.panes` into the layout's DFS leaf order so the mirror's pane
/// list matches the server's `ordered_panes`-built snapshot. A swap reorders
/// existing leaves without adding or removing any, so without this the mirror's
/// pane Vec would diverge from the server purely in order.
fn reorder_panes_to_layout(ws: &mut WorkspaceSnapshot) {
    let mut order = Vec::with_capacity(ws.panes.len());
    dfs_leaf_panes(&ws.layout, &mut order);
    ws.panes.sort_by_key(|p| {
        order
            .iter()
            .position(|id| *id == p.pane)
            .unwrap_or(usize::MAX)
    });
}

/// Applies `replace_node` at the workspace root level, falling back to
/// replacing the whole layout when the root itself matches. Does NOT prune —
/// callers decide when to call `prune_panes`.
fn apply_layout_node(ws: &mut WorkspaceSnapshot, target: NodeId, subtree: LayoutNode) {
    let root_matches = match (&ws.layout, target) {
        (LayoutNode::Split { id, .. }, NodeId::Split(tid)) => *id == tid,
        (LayoutNode::Pane { id, .. }, NodeId::Pane(tid)) => *id == tid,
        _ => false,
    };
    if root_matches {
        ws.layout = subtree;
    } else {
        replace_node(&mut ws.layout, target, subtree);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{read_message, write_message};
    use crate::message::ServerMessage;
    use ozmux_mux::{MultiPlexer, PaneDirection, Side, SplitOrientation, SurfaceKind, SwapOffset};
    use std::io::Cursor;

    fn id_key<T: serde::Serialize>(id: &T) -> String {
        serde_json::to_string(id).unwrap_or_default()
    }

    fn normalize_snapshot(mut snap: SessionSnapshot) -> SessionSnapshot {
        snap.workspaces
            .sort_by(|a, b| id_key(&a.workspace).cmp(&id_key(&b.workspace)));
        for ws in &mut snap.workspaces {
            ws.panes
                .sort_by(|a, b| id_key(&a.pane).cmp(&id_key(&b.pane)));
            for pane in &mut ws.panes {
                pane.surfaces
                    .sort_by(|a, b| id_key(&a.surface).cmp(&id_key(&b.surface)));
            }
        }
        snap
    }

    #[test]
    fn snapshot_plus_events_reconstructs_mux() {
        let mut mux = MultiPlexer::new();
        let session = mux.sessions()[0];

        let ws0 = mux.active_workspace();
        let pane0 = mux.active_pane(ws0).unwrap();

        let mut events: Vec<MuxEvent> = Vec::new();

        // Set workspace size so PaneResized events fire.
        events.extend(mux.set_workspace_size(ws0, 120, 40).unwrap());

        // Split the initial pane horizontally → [pane0 | pane1].
        let split1_events = mux
            .split_pane(
                pane0,
                SplitOrientation::Horizontal,
                Side::After,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();
        let pane1 = match &split1_events[0] {
            MuxEvent::PaneCreated { pane, .. } => *pane,
            _ => panic!("expected PaneCreated"),
        };
        events.extend(split1_events);

        // Spawn an extra surface on pane0 so break_surface_to_pane has a
        // multi-surface source.
        events.extend(
            mux.spawn_surface(pane0, SurfaceKind::Terminal, None)
                .unwrap(),
        );
        let extra_surface = {
            let last_ev = events.last().unwrap();
            match last_ev {
                MuxEvent::SurfaceSpawned { surface, .. } => *surface,
                _ => panic!("expected SurfaceSpawned"),
            }
        };

        // Take a snapshot AFTER the initial setup is complete — the mirror
        // starts from here and replays only the subsequent delta events.
        let snap0 = mux.snapshot(session).unwrap();
        let mut delta_events: Vec<MuxEvent> = Vec::new();

        // new_workspace → creates ws1.
        let nw_evs = mux.new_workspace(None).unwrap();
        let ws1 = match &nw_evs[0] {
            MuxEvent::WorkspaceCreated { workspace, .. } => *workspace,
            _ => panic!("expected WorkspaceCreated"),
        };
        delta_events.extend(nw_evs);

        // rename ws1.
        delta_events.extend(mux.rename_workspace(ws1, "renamed".to_string()).unwrap());

        // select back to ws0.
        delta_events.extend(mux.select_workspace(ws0).unwrap());

        // break_surface_to_pane: move extra_surface out of pane0 into a new pane.
        let break_evs = mux
            .break_surface_to_pane(extra_surface, SplitOrientation::Vertical, Side::After)
            .unwrap();
        delta_events.extend(break_evs);

        // swap pane0 with its neighbor.
        let swap_evs = mux.swap_pane(pane0, SwapOffset::Next).unwrap();
        delta_events.extend(swap_evs);

        // resize pane0.
        let resize_evs = mux.resize_pane(pane0, PaneDirection::Right, 5).unwrap();
        delta_events.extend(resize_evs);

        // Spawn an extra surface on pane1 so we can exercise close_surface.
        let spawn_evs = mux
            .spawn_surface(pane1, SurfaceKind::Terminal, None)
            .unwrap();
        let extra_pane1_surface = match &spawn_evs[0] {
            MuxEvent::SurfaceSpawned { surface, .. } => *surface,
            _ => panic!("expected SurfaceSpawned"),
        };
        delta_events.extend(spawn_evs);

        // Close the extra surface.
        delta_events.extend(mux.close_surface(extra_pane1_surface).unwrap());

        // Set active surface on pane1 to its remaining surface (no-op if already active).
        let pane1_first_surface = mux.surfaces(pane1).unwrap()[0];
        delta_events.extend(mux.set_active_surface(pane1, pane1_first_surface).unwrap());

        // Close ws1 (destroying it + its panes).
        delta_events.extend(mux.close_workspace(ws1).unwrap());

        // Apply all delta events to the mirror.
        let mut mirror = ClientMirror::from_snapshot(snap0);
        for ev in &delta_events {
            mirror.apply_event(ev);
        }

        let mirror_snap = normalize_snapshot(mirror.to_snapshot());
        let mux_snap = normalize_snapshot(mux.snapshot(session).unwrap());
        assert_eq!(
            mirror_snap, mux_snap,
            "mirror reconstruction diverged from Mux"
        );
    }

    #[test]
    fn nested_and_root_collapse_reconstructs_mux() {
        // Exercises the two reconstruction paths the main round-trip test does
        // NOT: a nested-split `LayoutChanged` collapse (replace_node recursion +
        // prune_panes dropping the closed pane) and a collapse-to-root
        // `WorkspaceRootChanged`.
        let mut mux = MultiPlexer::new();
        let session = mux.sessions()[0];
        let ws0 = mux.active_workspace();
        mux.set_workspace_size(ws0, 120, 40).unwrap();
        let pane0 = mux.active_pane(ws0).unwrap();

        // Build a nested tree: split p0 → p1, then split p1 → p2.
        // Tree: Split( p0, Split( p1, p2 ) ).
        let split1 = mux
            .split_pane(
                pane0,
                SplitOrientation::Horizontal,
                Side::After,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();
        let pane1 = match &split1[0] {
            MuxEvent::PaneCreated { pane, .. } => *pane,
            _ => panic!("expected PaneCreated"),
        };
        mux.split_pane(
            pane1,
            SplitOrientation::Vertical,
            Side::After,
            SurfaceKind::Terminal,
            None,
        )
        .unwrap();

        // Cold-attach snapshot taken at the 3-pane state; replay collapses.
        let snap0 = mux.snapshot(session).unwrap();
        let mut delta_events: Vec<MuxEvent> = Vec::new();

        // Close p1 → collapses the INNER split (nested LayoutChanged).
        delta_events.extend(mux.close_pane(pane1).unwrap());
        // Close p0 → collapses to the workspace root (WorkspaceRootChanged).
        delta_events.extend(mux.close_pane(pane0).unwrap());

        let mut mirror = ClientMirror::from_snapshot(snap0);
        for ev in &delta_events {
            mirror.apply_event(ev);
        }

        let mirror_snap = normalize_snapshot(mirror.to_snapshot());
        let mux_snap = normalize_snapshot(mux.snapshot(session).unwrap());
        assert_eq!(
            mirror_snap, mux_snap,
            "nested + root collapse reconstruction diverged from Mux"
        );
    }

    #[test]
    fn post_attach_workspace_and_resize_reconstructs() {
        // Exercises the three wire self-sufficiency bugs.
        // Fix 1: new_workspace root-pane layout not established.
        // Fix 2: workspace size not on the wire.
        // Fix 3: break_surface_to_pane LayoutChanged subtree has stale surface_kind.
        let mut mux = MultiPlexer::new();
        let session = mux.sessions()[0];

        let ws0 = mux.active_workspace();
        mux.set_workspace_size(ws0, 120, 40).unwrap();
        let pane0 = mux.active_pane(ws0).unwrap();

        // Build pane0 with two surfaces so break_surface_to_pane has a source.
        mux.spawn_surface(pane0, SurfaceKind::Terminal, None)
            .unwrap();
        let extra_surface = {
            let s = mux.surfaces(pane0).unwrap();
            *s.last().unwrap()
        };

        // Cold-attach snapshot — ws0 is already stable.
        let snap0 = mux.snapshot(session).unwrap();
        let mut delta_events: Vec<MuxEvent> = Vec::new();

        // Fix 1: new_workspace creates ws1 post-snapshot.
        let nw_evs = mux.new_workspace(None).unwrap();
        let ws1 = match &nw_evs[0] {
            MuxEvent::WorkspaceCreated { workspace, .. } => *workspace,
            _ => panic!("expected WorkspaceCreated"),
        };
        delta_events.extend(nw_evs);

        // Fix 2: set size on the post-snapshot workspace.
        delta_events.extend(mux.set_workspace_size(ws1, 80, 24).unwrap());

        // Fix 1 consequence: split the post-snapshot workspace so LayoutChanged
        // must match the real pane id (not a PaneId::default() placeholder).
        let pane_ws1 = mux.active_pane(ws1).unwrap();
        let split_evs = mux
            .split_pane(
                pane_ws1,
                SplitOrientation::Horizontal,
                Side::After,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();
        delta_events.extend(split_evs);

        // Fix 3: break a Terminal surface out of pane0; both PaneCreated and
        // LayoutChanged's subtree must carry Terminal kind.
        // TODO: Fix 3 coverage for non-Terminal surfaces requires constructing an
        // Extension/Browser surface, which needs infra not available in this unit
        // test. The patch in mux.rs ensures the LayoutChanged subtree's surface_kind
        // matches PaneCreated's for all kinds; Terminal is covered here.
        delta_events.extend(mux.select_workspace(ws0).unwrap());
        let break_evs = mux
            .break_surface_to_pane(extra_surface, SplitOrientation::Vertical, Side::After)
            .unwrap();
        // Assert that the LayoutChanged subtree's new_pane leaf carries the same
        // surface_kind as the PaneCreated event (Fix 3 invariant).
        let pane_created_kind = match &break_evs[0] {
            MuxEvent::PaneCreated { surfaces, .. } => surfaces[0].kind.clone(),
            _ => panic!("break_evs[0] must be PaneCreated"),
        };
        let layout_changed_new_pane_kind = {
            let new_pane_id = match &break_evs[0] {
                MuxEvent::PaneCreated { pane, .. } => *pane,
                _ => panic!("break_evs[0] must be PaneCreated"),
            };
            match &break_evs[1] {
                MuxEvent::LayoutChanged { subtree, .. } => {
                    find_pane_kind_in_layout(subtree, new_pane_id)
                        .expect("new_pane must appear in LayoutChanged subtree")
                }
                _ => panic!("break_evs[1] must be LayoutChanged"),
            }
        };
        assert_eq!(
            pane_created_kind, layout_changed_new_pane_kind,
            "Fix 3: LayoutChanged subtree surface_kind must match PaneCreated"
        );
        delta_events.extend(break_evs);

        // Apply all delta events to the mirror.
        let mut mirror = ClientMirror::from_snapshot(snap0);
        for ev in &delta_events {
            mirror.apply_event(ev);
        }

        let mirror_snap = normalize_snapshot(mirror.to_snapshot());
        let mux_snap = normalize_snapshot(mux.snapshot(session).unwrap());
        assert_eq!(
            mirror_snap, mux_snap,
            "post-attach workspace + resize reconstruction diverged from Mux"
        );
    }

    fn find_pane_kind_in_layout(
        node: &ozmux_mux::LayoutNode,
        target: ozmux_mux::PaneId,
    ) -> Option<ozmux_mux::SurfaceKind> {
        match node {
            ozmux_mux::LayoutNode::Pane {
                id, surface_kind, ..
            } if *id == target => Some(surface_kind.clone()),
            ozmux_mux::LayoutNode::Pane { .. } => None,
            ozmux_mux::LayoutNode::Split { first, second, .. } => {
                find_pane_kind_in_layout(first, target)
                    .or_else(|| find_pane_kind_in_layout(second, target))
            }
        }
    }

    #[test]
    fn welcome_codec_round_trip() {
        let mut mux = MultiPlexer::new();
        let session = mux.sessions()[0];
        let ws = mux.active_workspace();
        mux.set_workspace_size(ws, 80, 24).unwrap();
        let pane = mux.active_pane(ws).unwrap();
        mux.split_pane(
            pane,
            SplitOrientation::Horizontal,
            Side::After,
            SurfaceKind::Terminal,
            None,
        )
        .unwrap();
        let snap = mux.snapshot(session).unwrap();

        let msg = ServerMessage::Welcome { snapshot: snap };

        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cursor = Cursor::new(buf);
        let decoded: Option<ServerMessage> = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, Some(msg), "Welcome round-trip failed");
    }

    #[test]
    fn apply_events_defers_prune_across_cross_parent_swap() {
        // Build tree S( S2(p0, p2), p1 ) — DFS order: [p0, p2, p1].
        // Swap p2 with its Next neighbor p1 — a cross-parent swap.
        // apply_events must keep all three PaneSnapshots and match the
        // authoritative post-swap Mux snapshot.
        let mut mux = MultiPlexer::new();
        let session = mux.sessions()[0];
        let ws0 = mux.active_workspace();
        mux.set_workspace_size(ws0, 120, 40).unwrap();
        let p0 = mux.active_pane(ws0).unwrap();

        // Split p0 horizontally → [p0 | p1].
        let split1_events = mux
            .split_pane(
                p0,
                SplitOrientation::Horizontal,
                Side::After,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();
        let _p1 = match &split1_events[0] {
            MuxEvent::PaneCreated { pane, .. } => *pane,
            _ => panic!("expected PaneCreated"),
        };

        // Split p0 vertically → S( S2(p0, p2), p1 ).
        let split2_events = mux
            .split_pane(
                p0,
                SplitOrientation::Vertical,
                Side::After,
                SurfaceKind::Terminal,
                None,
            )
            .unwrap();
        let p2 = match &split2_events[0] {
            MuxEvent::PaneCreated { pane, .. } => *pane,
            _ => panic!("expected PaneCreated"),
        };

        // Cold-attach snapshot: 3-pane state.
        let mirror_snap = mux.snapshot(session).unwrap();
        let mut mirror = ClientMirror::from_snapshot(mirror_snap);

        // Swap p2 (DFS index 1) with its Next neighbor p1 (DFS index 2) — cross-parent.
        let swap_events = mux.swap_pane(p2, SwapOffset::Next).unwrap();
        assert!(
            !swap_events.is_empty(),
            "cross-parent swap must emit events"
        );

        mirror.apply_events(&swap_events);
        let out = normalize_snapshot(mirror.to_snapshot());
        let authoritative = normalize_snapshot(mux.snapshot(session).unwrap());

        assert_eq!(
            out, authoritative,
            "apply_events batch fold must equal the Mux post-swap snapshot"
        );
    }

    #[test]
    fn workspace_layout_and_root_getters_work() {
        let mux = MultiPlexer::new();
        let session = mux.sessions()[0];
        let snap = mux.snapshot(session).unwrap();
        let ws = snap.workspaces[0].workspace;
        let mirror = ClientMirror::from_snapshot(snap);
        assert!(
            mirror.workspace_layout(ws).is_some(),
            "layout getter must return Some"
        );
        assert!(
            mirror.workspace_root(ws).is_some(),
            "root getter must return Some"
        );

        let fake_ws: ozmux_mux::WorkspaceId = ozmux_mux::WorkspaceId::default();
        assert!(
            mirror.workspace_layout(fake_ws).is_none(),
            "unknown ws returns None"
        );
        assert!(
            mirror.workspace_root(fake_ws).is_none(),
            "unknown ws returns None"
        );
    }
}
