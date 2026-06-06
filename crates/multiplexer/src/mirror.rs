//! Mirrors an `ozmux_mux::Mux` into the Bevy ECS: the `MuxState` Resource,
//! `MuxId` forward-lookup components, the apply-handler that turns
//! `MuxEvent`s into ECS mutations, and a consistency checker. Plan 2b-1
//! builds this as library code; the source-of-truth flip is Plan 2b-2.

use crate::components::{
    ActivePane, ActiveSurface, BrowserProfile, CopyMode, OwningWorkspace, PaneDimensions,
    PaneMarker, SplitNode, SurfaceKind, SurfaceMarker, SurfaceOf, WorkspaceMarker,
    WorkspaceUiSubtree,
};
use crate::error::MultiplexerError;
use crate::layout::{
    SplitOrientation, child_flex, pane_frame_node, set_child_grow, split_node_bundle,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use ozmux_mux::{LayoutNode, MuxError, MuxEvent, NodeId, PaneId, SplitId, SurfaceId, WorkspaceId};
use slotmap::SecondaryMap;
use std::collections::{HashMap, HashSet};

/// Startup ordering seam: `Materialize` builds the ECS mirror from `MuxState`
/// before app-side bootstrap attaches the initial workspace.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum MultiplexerStartupSet {
    /// Realizes the Mux's initial tree into the ECS mirror.
    Materialize,
}

/// Startup system: realizes `MuxState`'s current tree into the ECS mirror.
pub(crate) fn materialize_mux_snapshot(mut commands: Commands, mut state: ResMut<MuxState>) {
    state.materialize_snapshot(&mut commands);
}

/// Authoritative `Mux` plus the reverse maps (`MuxId` → `Entity`). Forward
/// lookup (`Entity` → `MuxId`) is the `Mux*Id` components below.
#[derive(Resource)]
pub struct MuxState {
    /// The Bevy-free multiplexer core (Plan 2b-1: shadow only; 2b-2: authoritative).
    pub mux: ozmux_mux::Mux,
    pub(crate) workspaces: SecondaryMap<WorkspaceId, Entity>,
    pub(crate) panes: SecondaryMap<PaneId, Entity>,
    pub(crate) splits: SecondaryMap<SplitId, Entity>,
    pub(crate) surfaces: SecondaryMap<SurfaceId, Entity>,
    /// The GUI layout-root container entity per workspace (not a `Mux` node).
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

/// Read-only ECS context the find-and-replace path needs to inspect the
/// current layout tree (`Children` / `ChildOf` / `Node`) and resolve a layout
/// entity back to its `NodeId` (`MuxPaneId` / `MuxSplitId`).
#[derive(SystemParam)]
pub struct MirrorReadCtx<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    child_of: Query<'w, 's, &'static ChildOf>,
    nodes: Query<'w, 's, &'static Node>,
    node_ids: Query<'w, 's, (Option<&'static MuxPaneId>, Option<&'static MuxSplitId>)>,
    ws_ids: Query<'w, 's, &'static MuxWorkspaceId>,
    surface_ids: Query<'w, 's, &'static MuxSurfaceId>,
}

impl MirrorReadCtx<'_, '_> {
    /// The `WorkspaceId` an entity maps to, or `None`.
    pub(crate) fn workspace_id_of(&self, ent: Entity) -> Option<WorkspaceId> {
        self.ws_ids.get(ent).ok().map(|w| w.0)
    }

    /// The `PaneId` an entity maps to, or `None`.
    pub(crate) fn pane_id_of(&self, ent: Entity) -> Option<PaneId> {
        match self.node_ids.get(ent).ok()? {
            (Some(p), _) => Some(p.0),
            _ => None,
        }
    }

    /// The `SurfaceId` an entity maps to, or `None`.
    pub(crate) fn surface_id_of(&self, ent: Entity) -> Option<SurfaceId> {
        self.surface_ids.get(ent).ok().map(|s| s.0)
    }

    /// The `NodeId` an existing layout entity maps to (pane or split), or
    /// `None` if it carries neither marker.
    fn node_id_of(&self, ent: Entity) -> Option<NodeId> {
        match self.node_ids.get(ent).ok()? {
            (Some(p), _) => Some(NodeId::Pane(p.0)),
            (_, Some(s)) => Some(NodeId::Split(s.0)),
            _ => None,
        }
    }

    /// The `flex_grow` of a layout entity (1.0 if it has no `Node`).
    fn grow_of(&self, ent: Entity) -> f32 {
        self.nodes.get(ent).map(|n| n.flex_grow).unwrap_or(1.0)
    }

    /// Captures every `(Entity, NodeId)` in the subtree rooted at `ent`,
    /// walking the live ECS `Children` relation. Used to snapshot the OLD
    /// slot subtree before any reparent/despawn command is queued, so the
    /// stale sweep can despawn nodes that the replacement does not reuse.
    fn capture_subtree(&self, ent: Entity, out: &mut Vec<(Entity, NodeId)>) {
        if let Some(id) = self.node_id_of(ent) {
            out.push((ent, id));
        }
        if let Ok(kids) = self.children.get(ent) {
            for child in kids.iter() {
                self.capture_subtree(child, out);
            }
        }
    }
}

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

    /// Creates a `MuxState` backed by a freshly-created `Mux`, with empty
    /// reverse maps. Convenience for tests that need a live `MuxState`
    /// resource without importing `ozmux_mux` directly.
    pub fn with_new_mux() -> Self {
        Self::new(ozmux_mux::Mux::new())
    }

    /// Looks up the `SurfaceId` for `entity` using the reverse map.
    ///
    /// Prefer this over the `MirrorReadCtx` ECS query when the surface was
    /// spawned in the same deferred command batch (the `MuxSurfaceId` component
    /// may not be flushed yet, but the reverse map is updated immediately).
    pub(crate) fn surface_id_of_entity(&self, entity: Entity) -> Option<SurfaceId> {
        self.surfaces
            .iter()
            .find(|&(_, e)| *e == entity)
            .map(|(id, _)| id)
    }

    /// The `PaneId` mapped to `entity`, via the immediate reverse map (resolves
    /// even before the deferred `MuxPaneId` component is flushed). `None` if unmapped.
    pub(crate) fn pane_id_of_entity(&self, entity: Entity) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|&(_, e)| *e == entity)
            .map(|(id, _)| id)
    }

    /// The `WorkspaceId` mapped to `entity`, via the immediate reverse map. `None` if unmapped.
    pub(crate) fn workspace_id_of_entity(&self, entity: Entity) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|&(_, e)| *e == entity)
            .map(|(id, _)| id)
    }

    /// Realizes the Mux's current tree (active session's workspace, layout tree,
    /// surfaces) into the ECS, recording every reverse map + WorkspaceUiSubtree +
    /// ChildOf, matching the ECS composition `apply_event` produces.
    pub fn materialize_snapshot(&mut self, commands: &mut Commands) {
        let ws = self.mux.active_workspace();
        let name = self.mux.workspace_name(ws).unwrap_or("default").to_owned();
        let active_pane_id = self.mux.active_pane(ws).expect("active pane must exist");

        let (ws_ent, container) = spawn_workspace(commands, self, ws, &name);

        let layout = self
            .mux
            .workspace_layout(ws)
            .expect("workspace layout must be valid");

        let top_ent = realize_layout_node(commands, self, &layout, ws_ent, 1.0);
        commands.entity(top_ent).insert(ChildOf(container));

        let active_pane_ent = self.panes[active_pane_id];
        commands.entity(ws_ent).insert(ActivePane(active_pane_ent));
    }
}

/// Applies one `MuxEvent` to the ECS mirror.
///
/// `LayoutChanged` and `WorkspaceRootChanged` are handled by the
/// find-and-replace path: capture the old slot subtree, realize (reusing
/// already-mapped nodes) the replacement subtree into the slot, then despawn
/// the old nodes the replacement does not reuse.
///
/// `read` exposes the read-only ECS layout queries (`Children` / `ChildOf` /
/// `Node` / `Mux*Id`) the find-and-replace path and `LayoutRatioChanged` need
/// (Commands is write-only).
///
/// The `Mux` inside `state` MUST already reflect the post-event state before
/// this is called (so queries like `state.mux.surfaces(pane)` see the new
/// surface list).
pub fn apply_event(
    commands: &mut Commands,
    state: &mut MuxState,
    read: &MirrorReadCtx,
    event: &MuxEvent,
) {
    match event {
        MuxEvent::WorkspaceCreated { workspace, .. } => {
            let name = state
                .mux
                .workspace_name(*workspace)
                .unwrap_or("default")
                .to_owned();
            spawn_workspace(commands, state, *workspace, &name);
        }

        MuxEvent::PaneCreated {
            pane,
            workspace,
            surface_kind,
        } => {
            let ws_ent = state.workspaces[*workspace];
            let pane_ent = spawn_pane(commands, state, *pane, ws_ent, 1.0);

            let is_root = state
                .mux
                .workspace_root(*workspace)
                .map(|r| r == NodeId::Pane(*pane))
                .unwrap_or(false);
            if is_root {
                let container = state.layout_roots[*workspace];
                commands.entity(pane_ent).insert(ChildOf(container));
            }

            // NOTE: query surfaces from the Mux AFTER the mutation so the list
            // already includes any surfaces carried by this new pane (e.g. a
            // moved surface from break_surface_to_pane uses the existing
            // SurfaceId; detect reuse by checking state.surfaces).
            let surface_ids = state
                .mux
                .surfaces(*pane)
                .expect("PaneCreated: pane surfaces must be readable");
            let active_surface_id = state
                .mux
                .active_surface(*pane)
                .expect("PaneCreated: active surface must exist");

            let _ = surface_kind;

            let mut active_surface_ent = None;
            for sid in &surface_ids {
                let surf_ent = if state.surfaces.contains_key(*sid) {
                    let ent = state.surfaces[*sid];
                    commands
                        .entity(ent)
                        .insert((ChildOf(pane_ent), SurfaceOf(pane_ent)));
                    ent
                } else {
                    let sk = state
                        .mux
                        .surface_kind(*sid)
                        .expect("PaneCreated: surface kind must be readable");
                    spawn_surface(commands, state, *sid, pane_ent, sk)
                };
                if *sid == active_surface_id {
                    active_surface_ent = Some(surf_ent);
                }
            }

            let active_ent =
                active_surface_ent.expect("PaneCreated: active surface entity must exist");
            commands.entity(pane_ent).insert(ActiveSurface(active_ent));
        }

        MuxEvent::SurfaceSpawned {
            pane,
            surface,
            kind,
        } => {
            let pane_ent = state.panes[*pane];
            spawn_surface(commands, state, *surface, pane_ent, kind.clone());
        }

        MuxEvent::SurfaceClosed { surface } => {
            if let Some(ent) = state.surfaces.remove(*surface)
                && let Ok(mut ec) = commands.get_entity(ent)
            {
                ec.try_despawn();
            }
        }

        MuxEvent::PaneClosed { pane } => {
            if let Some(ent) = state.panes.remove(*pane) {
                commands.entity(ent).despawn();
            }
        }

        MuxEvent::ActivePaneChanged { workspace, pane } => {
            let ws_ent = state.workspaces[*workspace];
            let pane_ent = state.panes[*pane];
            commands.entity(ws_ent).insert(ActivePane(pane_ent));
        }

        MuxEvent::ActiveSurfaceChanged { pane, surface } => {
            let pane_ent = state.panes[*pane];
            let surf_ent = state.surfaces[*surface];
            commands.entity(pane_ent).insert(ActiveSurface(surf_ent));
        }

        MuxEvent::LayoutRatioChanged { split, ratio } => {
            let split_ent = state.splits[*split];
            if let Ok(kids) = read.children.get(split_ent) {
                let mut it = kids.iter();
                if let (Some(lhs), Some(rhs)) = (it.next(), it.next()) {
                    set_split_grows_from_ratio(commands, lhs, rhs, *ratio);
                }
            }
        }

        MuxEvent::WorkspaceRenamed { workspace, name } => {
            let ws_ent = state.workspaces[*workspace];
            commands.entity(ws_ent).insert(Name::new(name.clone()));
        }

        MuxEvent::WorkspaceDestroyed { workspace } => {
            // NOTE: the Mux emits no per-split event, so unmap the whole layout
            // subtree (splits + panes) here or their reverse-map entries leak
            // (the entities cascade-despawn with the workspace, but the maps would
            // keep stale ids). Panes/surfaces are usually already unmapped by the
            // preceding PaneClosed/SurfaceClosed events; these removes are idempotent.
            if let Some(container) = state.layout_roots.get(*workspace).copied()
                && let Ok(kids) = read.children.get(container)
                && let Some(top) = kids.iter().next()
            {
                let mut nodes = Vec::new();
                read.capture_subtree(top, &mut nodes);
                for (_ent, id) in nodes {
                    match id {
                        NodeId::Pane(p) => {
                            state.panes.remove(p);
                        }
                        NodeId::Split(s) => {
                            state.splits.remove(s);
                        }
                    }
                }
            }
            state.layout_roots.remove(*workspace);
            if let Some(ent) = state.workspaces.remove(*workspace) {
                commands.entity(ent).despawn();
            }
        }

        MuxEvent::PaneResized { pane, cols, rows } => {
            if let Some(&ent) = state.panes.get(*pane) {
                commands.entity(ent).insert(PaneDimensions {
                    cols: *cols,
                    rows: *rows,
                });
            }
        }

        // NOTE: GUI-side concerns (Plan 2b-2) or size-flow events — no ECS mirror
        // mutation needed at this layer.
        MuxEvent::SessionCreated { .. }
        | MuxEvent::WorkspaceSelected { .. }
        | MuxEvent::SurfaceCwdChanged { .. } => {}

        MuxEvent::LayoutChanged {
            workspace,
            root,
            subtree,
        } => {
            let ws_ent = state.workspaces[*workspace];
            let slot_ent = match root {
                NodeId::Pane(p) => state.panes[*p],
                NodeId::Split(s) => state.splits[*s],
            };
            let parent = read
                .child_of
                .get(slot_ent)
                .map(|c| c.parent())
                .expect("LayoutChanged: slot entity must have a parent");
            let inherited_grow = read.grow_of(slot_ent);
            replace_slot(
                commands,
                state,
                read,
                *workspace,
                ws_ent,
                slot_ent,
                parent,
                inherited_grow,
                subtree,
            );
        }

        MuxEvent::WorkspaceRootChanged { workspace, root } => {
            let ws_ent = state.workspaces[*workspace];
            let container = state.layout_roots[*workspace];
            let slot_ent = read
                .children
                .get(container)
                .ok()
                .and_then(|kids| kids.iter().next())
                .expect("WorkspaceRootChanged: layout-root container must have a child");
            replace_slot(
                commands, state, read, *workspace, ws_ent, slot_ent, container, 1.0, root,
            );
        }
    }
}

/// The shared two-phase find-and-replace: swap the subtree occupying
/// `slot_ent` (under `parent`, taking `inherited_grow`) for `subtree`.
///
/// 1. capture the OLD slot subtree's `(Entity, NodeId)` set BEFORE any
///    mutation, so the stale sweep is not confused by deferred reparents;
/// 2. realize `subtree` — reusing every already-mapped node (split / pane)
///    and only spawning genuinely new ones — then reparent its root onto the
///    slot, inheriting the slot's `flex_grow`;
/// 3. despawn each old node the replacement does NOT reuse (`NodeId ∉ live`)
///    and drop it from the reverse maps. Reuse (phase 2) runs before this
///    sweep so a reused entity is never caught here, and the live root is
///    reparented out before its old container is despawned (recursive despawn
///    would otherwise take it).
#[expect(
    clippy::too_many_arguments,
    reason = "find-and-replace needs the slot, its parent, the inherited grow, and the read ctx"
)]
fn replace_slot(
    commands: &mut Commands,
    state: &mut MuxState,
    read: &MirrorReadCtx,
    ws: WorkspaceId,
    ws_ent: Entity,
    slot_ent: Entity,
    parent: Entity,
    inherited_grow: f32,
    subtree: &LayoutNode,
) {
    let mut old_nodes = Vec::new();
    read.capture_subtree(slot_ent, &mut old_nodes);

    // NOTE: the stale sweep keys off the Mux's CURRENT full tree, not this
    // event's `subtree`. A sibling LayoutChanged in the same batch (e.g. a
    // cross-parent swap) can move an old node to another slot; `capture_subtree`
    // reads the pre-flush ECS where it still appears under this slot. Despawning
    // by event-subtree alone would delete that moved-but-live node. Only nodes
    // the Mux no longer has anywhere (e.g. close's collapsed split) are swept.
    let mut mux_live = HashSet::new();
    if let Ok(layout) = state.mux.workspace_layout(ws) {
        collect_live_node_ids(&layout, &mut mux_live);
    }

    let new_root = realize_subtree(commands, state, ws_ent, subtree, inherited_grow);
    // NOTE: use position-aware insertion so the new root lands at exactly the
    // slot's original index in the parent's Children list. Plain `ChildOf`
    // insertion would APPEND, corrupting first/second ordering when the replaced
    // slot was the first child of a multi-child parent.
    let slot_index = read
        .children
        .get(parent)
        .ok()
        .and_then(|kids| kids.iter().position(|e| e == slot_ent));
    if let Some(idx) = slot_index {
        commands.entity(parent).insert_children(idx, &[new_root]);
    } else {
        commands.entity(new_root).insert(ChildOf(parent));
    }

    for (ent, id) in old_nodes {
        if mux_live.contains(&id) {
            continue;
        }
        match id {
            NodeId::Pane(p) => {
                state.panes.remove(p);
            }
            NodeId::Split(s) => {
                state.splits.remove(s);
            }
        }
        if let Ok(mut ec) = commands.get_entity(ent) {
            ec.try_despawn();
        }
    }
}

/// Realizes `node` into ECS entities, REUSING any node already present in the
/// reverse maps (split or pane). A reused split/pane is re-grown and (for
/// inner nodes) reparented; a brand-new pane is spawned via `spawn_pane` and
/// its surfaces query-bridged from the `Mux`. Returns the realized root entity
/// (the caller reparents it onto the slot). `grow` is this node's `flex_grow`.
fn realize_subtree(
    commands: &mut Commands,
    state: &mut MuxState,
    ws_ent: Entity,
    node: &LayoutNode,
    grow: f32,
) -> Entity {
    match node {
        LayoutNode::Pane { id, .. } => {
            if let Some(&pane_ent) = state.panes.get(*id) {
                set_child_grow(commands, pane_ent, grow);
                pane_ent
            } else {
                realize_new_pane(commands, state, *id, ws_ent, grow)
            }
        }
        LayoutNode::Split {
            id,
            orientation,
            ratio,
            first,
            second,
        } => {
            let split_ent = if let Some(&split_ent) = state.splits.get(*id) {
                set_child_grow(commands, split_ent, grow);
                split_ent
            } else {
                spawn_split(commands, state, *id, *orientation, grow)
            };

            let first_ent = realize_subtree(commands, state, ws_ent, first, *ratio);
            let second_ent = realize_subtree(commands, state, ws_ent, second, 1.0 - ratio);

            commands.entity(first_ent).insert(ChildOf(split_ent));
            commands.entity(second_ent).insert(ChildOf(split_ent));

            split_ent
        }
    }
}

/// Spawns a brand-new pane plus its surfaces (querying the `Mux` for the
/// surface list), reusing any already-mapped surface entity (e.g. a moved
/// surface) and spawning the rest. Sets `ActiveSurface`.
fn realize_new_pane(
    commands: &mut Commands,
    state: &mut MuxState,
    pane: PaneId,
    ws_ent: Entity,
    grow: f32,
) -> Entity {
    let pane_ent = spawn_pane(commands, state, pane, ws_ent, grow);

    let surface_ids = state
        .mux
        .surfaces(pane)
        .expect("realize_new_pane: pane surfaces must be readable");
    let active_surface_id = state
        .mux
        .active_surface(pane)
        .expect("realize_new_pane: active surface must exist");

    let mut active_surface_ent = None;
    for sid in &surface_ids {
        let surf_ent = if let Some(&ent) = state.surfaces.get(*sid) {
            commands
                .entity(ent)
                .insert((ChildOf(pane_ent), SurfaceOf(pane_ent)));
            ent
        } else {
            let sk = state
                .mux
                .surface_kind(*sid)
                .expect("realize_new_pane: surface kind must be readable");
            spawn_surface(commands, state, *sid, pane_ent, sk)
        };
        if *sid == active_surface_id {
            active_surface_ent = Some(surf_ent);
        }
    }

    let active_ent =
        active_surface_ent.expect("realize_new_pane: active surface entity must exist");
    commands.entity(pane_ent).insert(ActiveSurface(active_ent));
    pane_ent
}

/// Collects every `NodeId` (splits and panes) present in `node` into `live`.
fn collect_live_node_ids(node: &LayoutNode, live: &mut HashSet<NodeId>) {
    match node {
        LayoutNode::Pane { id, .. } => {
            live.insert(NodeId::Pane(*id));
        }
        LayoutNode::Split {
            id, first, second, ..
        } => {
            live.insert(NodeId::Split(*id));
            collect_live_node_ids(first, live);
            collect_live_node_ids(second, live);
        }
    }
}

/// Sets the two children of a split's `Node.flex_grow` from a ratio,
/// matching the `set_child_grow` convention (`flex_basis = Px(0.0)`).
fn set_split_grows_from_ratio(commands: &mut Commands, lhs: Entity, rhs: Entity, ratio: f32) {
    use crate::layout::{normalized_grows, set_child_grow};
    let (l, r) = normalized_grows(ratio, 1.0 - ratio);
    set_child_grow(commands, lhs, l);
    set_child_grow(commands, rhs, r);
}

/// Spawns the workspace entity + layout-root container and records the reverse maps.
/// Returns `(workspace_entity, container_entity)`.
fn spawn_workspace(
    commands: &mut Commands,
    state: &mut MuxState,
    ws: WorkspaceId,
    name: &str,
) -> (Entity, Entity) {
    let ws_ent = commands
        .spawn((
            WorkspaceMarker,
            Name::new(name.to_owned()),
            MuxWorkspaceId(ws),
        ))
        .id();

    let container = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            Name::new(format!("layout-root: {name}")),
        ))
        .id();

    commands.entity(container).insert(ChildOf(ws_ent));
    commands
        .entity(ws_ent)
        .insert(WorkspaceUiSubtree(container));

    state.workspaces.insert(ws, ws_ent);
    state.layout_roots.insert(ws, container);

    (ws_ent, container)
}

/// Spawns a pane entity with the exact bundle the mirror's pane composition uses.
/// `ActiveSurface` is NOT set here — the caller sets it after surfaces exist.
/// `ChildOf` is also set by the caller.
fn spawn_pane(
    commands: &mut Commands,
    state: &mut MuxState,
    pane: PaneId,
    ws_ent: Entity,
    grow: f32,
) -> Entity {
    let mut pane_node = pane_frame_node();
    let cf = child_flex(grow);
    pane_node.flex_grow = cf.flex_grow;
    pane_node.flex_basis = cf.flex_basis;

    let ent = commands
        .spawn((
            PaneMarker,
            OwningWorkspace(ws_ent),
            CopyMode::default(),
            pane_node,
            Name::new(format!("pane: mux#{pane:?}")),
            MuxPaneId(pane),
        ))
        .id();

    state.panes.insert(pane, ent);
    ent
}

/// Spawns a surface entity with `SurfaceMarker`, kind, `ChildOf(pane)`, `SurfaceOf(pane)`,
/// and the `MuxSurfaceId` component.
fn spawn_surface(
    commands: &mut Commands,
    state: &mut MuxState,
    surface: SurfaceId,
    pane_ent: Entity,
    kind: ozmux_mux::SurfaceKind,
) -> Entity {
    let ecs_kind = mux_surface_kind_to_ecs(kind);
    let ent = commands
        .spawn((
            SurfaceMarker,
            ecs_kind,
            Name::new(format!("surface: mux#{surface:?}")),
            MuxSurfaceId(surface),
        ))
        .id();

    commands
        .entity(ent)
        .insert((ChildOf(pane_ent), SurfaceOf(pane_ent)));

    state.surfaces.insert(surface, ent);
    ent
}

/// Spawns a split entity merging `split_node_bundle` + `child_flex(grow)` fields
/// + `MuxSplitId`. `ChildOf` is set by the caller.
fn spawn_split(
    commands: &mut Commands,
    state: &mut MuxState,
    split: SplitId,
    orientation: ozmux_mux::SplitOrientation,
    grow: f32,
) -> Entity {
    let ecs_orientation = mux_orientation_to_ecs(orientation);
    let (mut node, split_component) = split_node_bundle(ecs_orientation);
    let cf = child_flex(grow);
    node.flex_grow = cf.flex_grow;
    node.flex_basis = cf.flex_basis;

    let ent = commands
        .spawn((
            node,
            split_component,
            Name::new(format!("split: mux#{split:?}")),
            MuxSplitId(split),
        ))
        .id();

    state.splits.insert(split, ent);
    ent
}

/// A discovered mismatch between the ECS mirror and the `Mux` tree.
#[derive(Debug)]
pub struct Mismatch(pub String);

/// Walks the ECS layout tree and the `Mux` tree in parallel (via the reverse
/// maps + `Mux*Id` components) and checks 1:1 correspondence: same kinds,
/// split orientations, child ratios, and active pointers.
///
/// Holds ONLY after a full `Vec<MuxEvent>` batch is applied AND `Commands`
/// are flushed (events describe the post-mutation state; intermediate states
/// do not match).
pub fn mirror_matches(world: &World, state: &MuxState) -> Result<(), Mismatch> {
    let ws = state.mux.active_workspace();
    let ws_ent = *state.workspaces.get(ws).ok_or_else(|| {
        Mismatch(format!(
            "active workspace {ws:?} missing from state.workspaces"
        ))
    })?;

    let container = world
        .get::<WorkspaceUiSubtree>(ws_ent)
        .ok_or_else(|| {
            Mismatch(format!(
                "workspace entity {ws_ent:?} missing WorkspaceUiSubtree"
            ))
        })?
        .0;

    let top_ent = world
        .get::<Children>(container)
        .and_then(|c| c.iter().next())
        .ok_or_else(|| Mismatch("layout-root container has no children".to_owned()))?;

    let layout = state
        .mux
        .workspace_layout(ws)
        .map_err(|e| Mismatch(format!("workspace_layout failed: {e:?}")))?;

    let expected_cells: Option<HashMap<PaneId, (u16, u16)>> = state
        .mux
        .workspace_size(ws)
        .ok()
        .flatten()
        .and_then(|(cols, rows)| {
            state
                .mux
                .resolve_sizes(ws, cols, rows)
                .ok()
                .map(|v| v.into_iter().collect())
        });

    check_node(world, state, top_ent, &layout, "", &expected_cells)?;

    let active_pane_id = state
        .mux
        .active_pane(ws)
        .map_err(|e| Mismatch(format!("active_pane failed: {e:?}")))?;
    let expected_active_ent = *state.panes.get(active_pane_id).ok_or_else(|| {
        Mismatch(format!(
            "active pane {active_pane_id:?} missing from state.panes"
        ))
    })?;
    let actual_active_ent = world
        .get::<ActivePane>(ws_ent)
        .ok_or_else(|| Mismatch(format!("workspace {ws_ent:?} missing ActivePane")))?
        .0;
    if actual_active_ent != expected_active_ent {
        return Err(Mismatch(format!(
            "ActivePane mismatch: ECS={actual_active_ent:?} expected={expected_active_ent:?}"
        )));
    }

    Ok(())
}

/// Translates a `MuxError` (id-addressed) to a `MultiplexerError` (Entity-addressed)
/// by looking up each id in the reverse maps. Ids absent from the maps yield
/// `Entity::PLACEHOLDER` so the lifted error is always constructible.
pub(crate) fn lift(state: &MuxState, err: MuxError) -> MultiplexerError {
    match err {
        MuxError::WorkspaceNotFound(ws) => MultiplexerError::WorkspaceNotFound(
            state
                .workspaces
                .get(ws)
                .copied()
                .unwrap_or(Entity::PLACEHOLDER),
        ),
        MuxError::PaneNotFound(pane) => MultiplexerError::PaneNotFound(
            state
                .panes
                .get(pane)
                .copied()
                .unwrap_or(Entity::PLACEHOLDER),
        ),
        MuxError::SurfaceNotFound(surface) => MultiplexerError::SurfaceNotFound(
            state
                .surfaces
                .get(surface)
                .copied()
                .unwrap_or(Entity::PLACEHOLDER),
        ),
        MuxError::CannotCloseLastPaneInWorkspace(ws) => {
            MultiplexerError::CannotCloseLastPaneInWorkspace(
                state
                    .workspaces
                    .get(ws)
                    .copied()
                    .unwrap_or(Entity::PLACEHOLDER),
            )
        }
        MuxError::CannotRemoveLastSurface(pane) => MultiplexerError::CannotRemoveLastSurface(
            state
                .panes
                .get(pane)
                .copied()
                .unwrap_or(Entity::PLACEHOLDER),
        ),
        MuxError::MissingParentCell => MultiplexerError::MissingParentCell,
    }
}

/// Extracts the `PaneId` from the first `PaneCreated` event in `events`, or `None`.
pub(crate) fn created_pane_id(events: &[MuxEvent]) -> Option<PaneId> {
    events.iter().find_map(|e| match e {
        MuxEvent::PaneCreated { pane, .. } => Some(*pane),
        _ => None,
    })
}

/// Extracts the `WorkspaceId` from the first `WorkspaceCreated` event in `events`, or `None`.
pub(crate) fn created_workspace_id(events: &[MuxEvent]) -> Option<WorkspaceId> {
    events.iter().find_map(|e| match e {
        MuxEvent::WorkspaceCreated { workspace, .. } => Some(*workspace),
        _ => None,
    })
}

/// Extracts the `SurfaceId` from the first `SurfaceSpawned` event in `events`, or `None`.
pub(crate) fn single_spawned_surface_id(events: &[MuxEvent]) -> Option<SurfaceId> {
    events.iter().find_map(|e| match e {
        MuxEvent::SurfaceSpawned { surface, .. } => Some(*surface),
        _ => None,
    })
}

/// Returns the active (seed) surface of `pane` from `state.mux`, or `None`.
pub(crate) fn seed_surface_of(state: &MuxState, pane: PaneId) -> Option<SurfaceId> {
    state.mux.active_surface(pane).ok()
}

/// Debug-only: after each frame's command flush, assert the ECS mirror matches
/// the authoritative `Mux`. Catches apply-handler drift on real usage at zero
/// release-build cost (gated on `debug_assertions`).
#[cfg(debug_assertions)]
pub(crate) fn assert_mirror_consistent(world: &World) {
    if let Some(state) = world.get_resource::<MuxState>()
        && let Err(m) = mirror_matches(world, state)
    {
        panic!("mux mirror drift: {m:?}");
    }
}

/// Debug-only: every reverse-map entry resolves to a live entity carrying the
/// matching `Mux*Id`, catching unmap leaks after despawns.
#[cfg(debug_assertions)]
pub fn assert_no_map_leaks(world: &World, state: &MuxState) {
    for (id, &ent) in &state.panes {
        let found = world.get::<MuxPaneId>(ent).map(|c| c.0);
        assert_eq!(
            found,
            Some(id),
            "state.panes leak: id={id:?} ent={ent:?} found={found:?}"
        );
    }
    for (id, &ent) in &state.splits {
        let found = world.get::<MuxSplitId>(ent).map(|c| c.0);
        assert_eq!(
            found,
            Some(id),
            "state.splits leak: id={id:?} ent={ent:?} found={found:?}"
        );
    }
    for (id, &ent) in &state.surfaces {
        let found = world.get::<MuxSurfaceId>(ent).map(|c| c.0);
        assert_eq!(
            found,
            Some(id),
            "state.surfaces leak: id={id:?} ent={ent:?} found={found:?}"
        );
    }
    for (id, &ent) in &state.workspaces {
        let found = world.get::<MuxWorkspaceId>(ent).map(|c| c.0);
        assert_eq!(
            found,
            Some(id),
            "state.workspaces leak: id={id:?} ent={ent:?} found={found:?}"
        );
    }
}

/// Recursively compares a single ECS entity with the corresponding `LayoutNode`.
fn check_node(
    world: &World,
    state: &MuxState,
    ecs_ent: Entity,
    mux_node: &LayoutNode,
    path: &str,
    expected_cells: &Option<HashMap<PaneId, (u16, u16)>>,
) -> Result<(), Mismatch> {
    match mux_node {
        LayoutNode::Pane { id, .. } => {
            if world.get::<PaneMarker>(ecs_ent).is_none() {
                return Err(Mismatch(format!(
                    "path {path:?}: expected PaneMarker on {ecs_ent:?}"
                )));
            }
            let comp_id = world
                .get::<MuxPaneId>(ecs_ent)
                .ok_or_else(|| {
                    Mismatch(format!(
                        "path {path:?}: entity {ecs_ent:?} missing MuxPaneId"
                    ))
                })?
                .0;
            if comp_id != *id {
                return Err(Mismatch(format!(
                    "path {path:?}: MuxPaneId mismatch: ECS={comp_id:?} mux={id:?}"
                )));
            }
            let mapped_ent = state.panes.get(*id).copied().ok_or_else(|| {
                Mismatch(format!(
                    "path {path:?}: pane {id:?} missing from state.panes"
                ))
            })?;
            if mapped_ent != ecs_ent {
                return Err(Mismatch(format!(
                    "path {path:?}: state.panes[{id:?}]={mapped_ent:?} but walked to {ecs_ent:?}"
                )));
            }
            if let Some(cells) = expected_cells
                && let Some(&(cols, rows)) = cells.get(id)
            {
                let actual = world.get::<PaneDimensions>(ecs_ent).copied();
                let expected = Some(PaneDimensions { cols, rows });
                if actual != expected {
                    return Err(Mismatch(format!(
                        "path {path:?}: PaneDimensions mismatch: ECS={actual:?} expected={expected:?}"
                    )));
                }
            }
            Ok(())
        }
        LayoutNode::Split {
            id,
            orientation,
            ratio: _,
            first,
            second,
        } => {
            let split_comp = world.get::<SplitNode>(ecs_ent).ok_or_else(|| {
                Mismatch(format!("path {path:?}: expected SplitNode on {ecs_ent:?}"))
            })?;
            let expected_orientation = mux_orientation_to_ecs(*orientation);
            if split_comp.orientation != expected_orientation {
                return Err(Mismatch(format!(
                    "path {path:?}: SplitNode orientation mismatch: ECS={:?} mux={expected_orientation:?}",
                    split_comp.orientation
                )));
            }
            let comp_id = world
                .get::<MuxSplitId>(ecs_ent)
                .ok_or_else(|| {
                    Mismatch(format!(
                        "path {path:?}: entity {ecs_ent:?} missing MuxSplitId"
                    ))
                })?
                .0;
            if comp_id != *id {
                return Err(Mismatch(format!(
                    "path {path:?}: MuxSplitId mismatch: ECS={comp_id:?} mux={id:?}"
                )));
            }
            let mapped_ent = state.splits.get(*id).copied().ok_or_else(|| {
                Mismatch(format!(
                    "path {path:?}: split {id:?} missing from state.splits"
                ))
            })?;
            if mapped_ent != ecs_ent {
                return Err(Mismatch(format!(
                    "path {path:?}: state.splits[{id:?}]={mapped_ent:?} but walked to {ecs_ent:?}"
                )));
            }

            let kids: Vec<Entity> = world
                .get::<Children>(ecs_ent)
                .map(|c| c.iter().collect())
                .unwrap_or_default();
            if kids.len() != 2 {
                return Err(Mismatch(format!(
                    "path {path:?}: split {ecs_ent:?} has {} children, expected 2",
                    kids.len()
                )));
            }

            check_node(
                world,
                state,
                kids[0],
                first,
                &format!("{path}/0"),
                expected_cells,
            )?;
            check_node(
                world,
                state,
                kids[1],
                second,
                &format!("{path}/1"),
                expected_cells,
            )?;
            Ok(())
        }
    }
}

/// Converts a `ozmux_mux::SplitOrientation` to the ECS `crate::layout::SplitOrientation`.
fn mux_orientation_to_ecs(o: ozmux_mux::SplitOrientation) -> SplitOrientation {
    match o {
        ozmux_mux::SplitOrientation::Horizontal => SplitOrientation::Horizontal,
        ozmux_mux::SplitOrientation::Vertical => SplitOrientation::Vertical,
    }
}

/// Converts the ECS `crate::layout::SplitOrientation` to `ozmux_mux::SplitOrientation`.
pub(crate) fn ecs_orientation_to_mux(o: SplitOrientation) -> ozmux_mux::SplitOrientation {
    match o {
        SplitOrientation::Horizontal => ozmux_mux::SplitOrientation::Horizontal,
        SplitOrientation::Vertical => ozmux_mux::SplitOrientation::Vertical,
    }
}

/// Converts the ECS `crate::layout::Side` to `ozmux_mux::Side`.
pub(crate) fn ecs_side_to_mux(s: crate::layout::Side) -> ozmux_mux::Side {
    match s {
        crate::layout::Side::Before => ozmux_mux::Side::Before,
        crate::layout::Side::After => ozmux_mux::Side::After,
    }
}

/// Converts the ECS `crate::components::SurfaceKind` to `ozmux_mux::SurfaceKind`.
pub(crate) fn ecs_surface_kind_to_mux(k: SurfaceKind) -> ozmux_mux::SurfaceKind {
    match k {
        SurfaceKind::Terminal => ozmux_mux::SurfaceKind::Terminal,
        SurfaceKind::Extension { entry } => ozmux_mux::SurfaceKind::Extension { entry },
        SurfaceKind::Browser {
            initial_url,
            profile,
        } => ozmux_mux::SurfaceKind::Browser {
            initial_url,
            profile: ecs_browser_profile_to_mux(profile),
        },
    }
}

/// Converts the ECS `crate::components::BrowserProfile` to `ozmux_mux::BrowserProfile`.
fn ecs_browser_profile_to_mux(p: crate::components::BrowserProfile) -> ozmux_mux::BrowserProfile {
    match p {
        crate::components::BrowserProfile::Named { name } => {
            ozmux_mux::BrowserProfile::Named { name }
        }
        crate::components::BrowserProfile::Incognito => ozmux_mux::BrowserProfile::Incognito,
    }
}

/// Converts the ECS `crate::swap::SwapOffset` to `ozmux_mux::SwapOffset`.
pub(crate) fn ecs_swap_offset_to_mux(o: crate::swap::SwapOffset) -> ozmux_mux::SwapOffset {
    match o {
        crate::swap::SwapOffset::Prev => ozmux_mux::SwapOffset::Prev,
        crate::swap::SwapOffset::Next => ozmux_mux::SwapOffset::Next,
    }
}

/// Converts the ECS `crate::direction::PaneDirection` to `ozmux_mux::PaneDirection`.
pub(crate) fn ecs_direction_to_mux(d: crate::direction::PaneDirection) -> ozmux_mux::PaneDirection {
    match d {
        crate::direction::PaneDirection::Up => ozmux_mux::PaneDirection::Up,
        crate::direction::PaneDirection::Down => ozmux_mux::PaneDirection::Down,
        crate::direction::PaneDirection::Left => ozmux_mux::PaneDirection::Left,
        crate::direction::PaneDirection::Right => ozmux_mux::PaneDirection::Right,
    }
}

/// Converts a `ozmux_mux::SurfaceKind` to the ECS `crate::components::SurfaceKind`.
fn mux_surface_kind_to_ecs(k: ozmux_mux::SurfaceKind) -> SurfaceKind {
    match k {
        ozmux_mux::SurfaceKind::Terminal => SurfaceKind::Terminal,
        ozmux_mux::SurfaceKind::Extension { entry } => SurfaceKind::Extension { entry },
        ozmux_mux::SurfaceKind::Browser {
            initial_url,
            profile,
        } => SurfaceKind::Browser {
            initial_url,
            profile: mux_browser_profile_to_ecs(profile),
        },
    }
}

/// Converts a `ozmux_mux::BrowserProfile` to the ECS `crate::components::BrowserProfile`.
fn mux_browser_profile_to_ecs(p: ozmux_mux::BrowserProfile) -> BrowserProfile {
    match p {
        ozmux_mux::BrowserProfile::Named { name } => BrowserProfile::Named { name },
        ozmux_mux::BrowserProfile::Incognito => BrowserProfile::Incognito,
    }
}

/// Recursively realizes a `LayoutNode` into ECS entities. Returns the top entity
/// for the subtree (which the caller wires via `ChildOf`).
///
/// `grow` is this node's `flex_grow` relative to its parent split slot.
/// `ws_ent` is the owning workspace entity (needed for `OwningWorkspace` on panes).
fn realize_layout_node(
    commands: &mut Commands,
    state: &mut MuxState,
    node: &LayoutNode,
    ws_ent: Entity,
    grow: f32,
) -> Entity {
    match node {
        LayoutNode::Pane {
            id, surface_kind, ..
        } => {
            let pane_ent = spawn_pane(commands, state, *id, ws_ent, grow);

            let surface_ids = state
                .mux
                .surfaces(*id)
                .expect("pane surfaces must be readable");
            let active_surface_id = state
                .mux
                .active_surface(*id)
                .expect("active surface must exist");

            // NOTE: LayoutNode.surface_kind carries only the ACTIVE surface's
            // kind; using it for non-active surfaces yields the wrong kind. Use
            // mux.surface_kind() per surface id instead.
            let _ = surface_kind;

            let mut active_surface_ent = None;
            for sid in &surface_ids {
                let sk = state
                    .mux
                    .surface_kind(*sid)
                    .expect("surface kind must be readable");
                let surf_ent = spawn_surface(commands, state, *sid, pane_ent, sk);
                if *sid == active_surface_id {
                    active_surface_ent = Some(surf_ent);
                }
            }

            let active_ent = active_surface_ent.expect("active surface entity must exist");
            commands.entity(pane_ent).insert(ActiveSurface(active_ent));

            pane_ent
        }
        LayoutNode::Split {
            id,
            orientation,
            ratio,
            first,
            second,
        } => {
            let split_ent = spawn_split(commands, state, *id, *orientation, grow);

            let first_ent = realize_layout_node(commands, state, first, ws_ent, *ratio);
            let second_ent = realize_layout_node(commands, state, second, ws_ent, 1.0 - ratio);

            commands.entity(first_ent).insert(ChildOf(split_ent));
            commands.entity(second_ent).insert(ChildOf(split_ent));

            split_ent
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{ActiveSurface, SplitNode, SurfaceKind, WorkspaceMarker};
    use crate::layout::split_ratio;
    use crate::plugin::MultiplexerPlugin;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn materialized_bootstrap_has_expected_tree() {
        // The plugin inserts MuxState(Mux::new()) + materializes it at Startup.
        // Assert the bootstrap mirror DIRECTLY (not vs an oracle — once the flip
        // lands, an oracle built from MultiplexerCommands would be Mux-driven too,
        // making such a comparison a tautology): exactly one workspace/pane/surface
        // with the active pointers + subtree wired, and mirror_matches holds.
        let mut app = make_mux_app();
        let ws_count = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .count();
        let pane_count = app
            .world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .count();
        let surf_count = app
            .world_mut()
            .query_filtered::<Entity, With<SurfaceMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(
            (ws_count, pane_count, surf_count),
            (1, 1, 1),
            "materialized bootstrap = one workspace/pane/surface"
        );
        let ws = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .next()
            .expect("bootstrap workspace");
        assert!(
            app.world().get::<ActivePane>(ws).is_some(),
            "workspace has ActivePane"
        );
        assert!(
            app.world().get::<WorkspaceUiSubtree>(ws).is_some(),
            "workspace has WorkspaceUiSubtree"
        );
        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "bootstrap mirror is consistent"
        );
    }

    #[test]
    fn mirror_matches_passes_on_materialized_snapshot() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MultiplexerPlugin);
        app.update();

        let result = {
            let world = app.world();
            let s = world.resource::<MuxState>();
            mirror_matches_with(world, s)
        };
        assert!(result.is_ok(), "mirror_matches failed: {result:?}");
    }

    #[test]
    fn mirror_matches_fails_on_corrupted_pane_dimensions() {
        let mut app = make_mux_app();

        // Split first, then set workspace size via run_mux_op so PaneResized
        // events flow through apply_event and PaneDimensions gets stamped.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let p = m.active_pane(ws).unwrap();
            m.split_pane(
                p,
                ozmux_mux::SplitOrientation::Horizontal,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap()
        });
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            m.set_workspace_size(ws, 80, 24).unwrap()
        });

        let baseline = {
            let world = app.world();
            let s = world.resource::<MuxState>();
            mirror_matches_with(world, s)
        };
        assert!(baseline.is_ok(), "baseline should pass before corruption");

        // Corrupt one pane's PaneDimensions to a wrong value.
        let pane_ent = {
            let mut q = app.world_mut().query_filtered::<Entity, With<PaneMarker>>();
            q.iter(app.world()).next().expect("pane entity must exist")
        };
        app.world_mut()
            .entity_mut(pane_ent)
            .insert(PaneDimensions { cols: 1, rows: 1 });

        let result = {
            let world = app.world();
            let s = world.resource::<MuxState>();
            mirror_matches_with(world, s)
        };
        assert!(
            result.is_err(),
            "mirror_matches should detect corrupted PaneDimensions"
        );
    }

    #[test]
    fn pane_resized_writes_pane_dimensions_and_mirror_matches() {
        let mut app = make_mux_app();

        // Set workspace size via run_mux_op to trigger PaneResized events.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            m.set_workspace_size(ws, 80, 24).unwrap()
        });

        // Assert the single pane has PaneDimensions == Mux's resolved cells.
        let pane_ent = only_pane(&mut app);
        let dims = app
            .world()
            .get::<PaneDimensions>(pane_ent)
            .copied()
            .expect("PaneDimensions must be present after set_workspace_size");
        assert_eq!(
            (dims.cols, dims.rows),
            (80, 24),
            "single pane fills the workspace"
        );

        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after set_workspace_size"
        );
    }

    #[test]
    fn pane_resized_after_split_matches_resolve_sizes() {
        let mut app = make_mux_app();

        // Split first, then set workspace size.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let p = m.active_pane(ws).unwrap();
            m.split_pane(
                p,
                ozmux_mux::SplitOrientation::Horizontal,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap()
        });
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            m.set_workspace_size(ws, 80, 24).unwrap()
        });

        // Verify each pane's PaneDimensions matches the Mux's resolved cells.
        let (expected_cells, pane_entities): (Vec<_>, Vec<_>) = {
            let state = app.world().resource::<MuxState>();
            let ws = state.mux.active_workspace();
            let cells = state.mux.resolve_sizes(ws, 80, 24).unwrap();
            cells
                .into_iter()
                .map(|(pid, (c, r))| ((c, r), state.panes[pid]))
                .unzip()
        };

        for (ent, expected) in pane_entities.iter().zip(expected_cells.iter()) {
            let dims = app
                .world()
                .get::<PaneDimensions>(*ent)
                .copied()
                .expect("PaneDimensions must be on every pane after set_workspace_size");
            assert_eq!(
                (dims.cols, dims.rows),
                *expected,
                "pane {ent:?} dimensions must match resolve_sizes"
            );
        }

        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after split + set_workspace_size"
        );
    }

    fn mirror_matches_with(world: &World, state: &MuxState) -> Result<(), Mismatch> {
        mirror_matches(world, state)
    }

    /// Runs a Mux mutation op, applies every returned `MuxEvent` via
    /// `apply_event`, flushes commands, and asserts `mirror_matches` is `Ok`.
    fn run_mux_op(app: &mut App, op: impl FnOnce(&mut ozmux_mux::Mux) -> Vec<MuxEvent>) {
        // Step 1: run the Mux mutation directly on the resource (no system).
        let events: Vec<MuxEvent> = {
            let mut state = app.world_mut().resource_mut::<MuxState>();
            op(&mut state.mux)
        };

        // Step 2: apply every event via apply_event in a one-shot system.
        // NOTE: Bevy run_system_once requires the closure to be FnMut + Send +
        // Sync; capture events as a local moved into a closure that wraps the
        // loop to satisfy the system signature constraints.
        app.world_mut()
            .run_system_once({
                let events = events;
                move |mut commands: Commands, mut state: ResMut<MuxState>, read: MirrorReadCtx| {
                    for ev in &events {
                        apply_event(&mut commands, &mut state, &read, ev);
                    }
                }
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let result = {
            let world = app.world();
            let s = world.resource::<MuxState>();
            mirror_matches(world, s)
        };
        assert!(result.is_ok(), "mirror_matches after op: {result:?}");

        #[cfg(debug_assertions)]
        {
            let world = app.world();
            let s = world.resource::<MuxState>();
            assert_no_map_leaks(world, s);
        }
    }

    fn make_mux_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MultiplexerPlugin);
        app.update();
        app
    }

    /// The only pane entity in a freshly-made single-pane app.
    fn only_pane(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .next()
            .expect("only pane must exist")
    }

    /// The active workspace entity (bootstrap or most-recently-created).
    fn active_workspace(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .next()
            .expect("active workspace must exist")
    }

    /// The first (and only) split entity directly under the workspace's layout-root container.
    fn split_under_workspace_root(app: &App, ws: Entity) -> Entity {
        let container = app
            .world()
            .get::<WorkspaceUiSubtree>(ws)
            .expect("workspace must have WorkspaceUiSubtree")
            .0;
        app.world()
            .get::<Children>(container)
            .and_then(|c| c.iter().next())
            .expect("container must have a child split")
    }

    #[test]
    fn new_workspace_creates_ecs_tree_and_mirror_matches() {
        // Drive create_workspace once (now Mux-backed), assert the resulting ECS
        // tree directly and that mirror_matches holds. The oracle-vs-mux comparison
        // would be a tautology after the flip (both sides go through Mux).
        let mut app = make_mux_app();

        app.world_mut()
            .run_system_once(|mut mux: crate::commands::MultiplexerCommands| {
                mux.create_workspace(Some("default".to_owned()));
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let ws_count = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .count();
        let pane_count = app
            .world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .count();
        let surf_count = app
            .world_mut()
            .query_filtered::<Entity, With<SurfaceMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(
            (ws_count, pane_count, surf_count),
            (2, 2, 2),
            "after create_workspace: 2 workspaces, 2 panes, 2 surfaces"
        );

        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after create_workspace"
        );
    }

    #[test]
    fn add_surface_active_unchanged() {
        // add_surface adds a second surface but must NOT change the active surface.
        // Assert: surface count increased to 2, active is still the bootstrap, mirror_matches.
        let mut app = make_mux_app();

        let pane = only_pane(&mut app);
        let original_active = app
            .world()
            .get::<ActiveSurface>(pane)
            .expect("pane must have ActiveSurface")
            .0;

        let second = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.add_surface(pane, SurfaceKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let surf_count = app
            .world()
            .get::<crate::components::Surfaces>(pane)
            .map(|s| s.iter().count())
            .unwrap_or(0);
        assert_eq!(surf_count, 2, "pane now has 2 surfaces");
        assert_ne!(second, original_active, "second surface is a new entity");
        assert_eq!(
            app.world().get::<ActiveSurface>(pane).map(|a| a.0),
            Some(original_active),
            "active surface unchanged after add_surface"
        );
        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after add_surface"
        );
    }

    #[test]
    fn set_active_surface_switches_active() {
        // add_surface then set_active_surface: active must change to the new surface.
        // Assert: ActiveSurface on the pane is now the second surface, mirror_matches.
        let mut app = make_mux_app();

        let pane = only_pane(&mut app);

        let second = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.add_surface(pane, SurfaceKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.set_active_surface(pane, second).unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        assert_eq!(
            app.world().get::<ActiveSurface>(pane).map(|a| a.0),
            Some(second),
            "active surface switched to second after set_active_surface"
        );
        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after set_active_surface"
        );
    }

    #[test]
    fn rename_workspace_updates_name_in_ecs_and_mux() {
        // Drive rename_workspace through MultiplexerCommands (now Mux-backed) and
        // assert the ECS Name and the Mux's own name both read back "renamed".
        // The oracle-vs-mux comparison would be a tautology after the flip.
        let mut app = make_mux_app();

        app.world_mut()
            .run_system_once(
                |mut mux: crate::commands::MultiplexerCommands,
                 ws_q: Query<Entity, With<WorkspaceMarker>>| {
                    let ws = ws_q.iter().next().expect("workspace must exist");
                    mux.rename_workspace(ws, "renamed".to_owned()).unwrap();
                },
            )
            .unwrap();
        app.world_mut().flush();
        app.update();

        let ecs_name = app
            .world_mut()
            .query_filtered::<&Name, With<WorkspaceMarker>>()
            .iter(app.world())
            .next()
            .map(|n| n.as_str().to_owned())
            .unwrap();
        assert_eq!(ecs_name, "renamed", "ECS Name updated by rename");

        let state = app.world().resource::<MuxState>();
        let ws_id = state.mux.active_workspace();
        let mux_name = state.mux.workspace_name(ws_id).unwrap_or("");
        assert_eq!(mux_name, "renamed", "Mux name updated by rename");

        assert!(
            mirror_matches(app.world(), state).is_ok(),
            "mirror_matches after rename_workspace"
        );
    }

    #[test]
    fn close_workspace_reduces_counts_and_mirror_matches() {
        // Drive create_workspace + close_workspace through MultiplexerCommands
        // (now Mux-backed) and assert the ECS tree directly plus mirror_matches.
        // The oracle-vs-mux comparison would be a tautology after the flip.
        let mut app = make_mux_app();

        // Create a second workspace so we have one to close.
        let second_ws = app
            .world_mut()
            .run_system_once(|mut mux: crate::commands::MultiplexerCommands| {
                let out = mux.create_workspace(Some("second".to_owned()));
                out.workspace
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        // Confirm we have 2 workspaces before the close.
        let ws_before = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(ws_before, 2, "two workspaces before close");

        // Close the second workspace through MultiplexerCommands.
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.close_workspace(second_ws);
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let ws_after = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(ws_after, 1, "one workspace remains after close");

        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after close_workspace"
        );
    }

    #[test]
    fn close_workspace_with_splits_unmaps_splits() {
        // A workspace closed while it contains a Split must unmap that split from
        // state.splits — the Mux emits no per-split event. run_mux_op asserts
        // assert_no_map_leaks, so a leaked split entry fails here (it did pre-fix).
        let mut mux_app = make_mux_app();
        run_mux_op(&mut mux_app, |m| m.new_workspace().unwrap());
        run_mux_op(&mut mux_app, |m| {
            let ws = m.active_workspace();
            let p = m.active_pane(ws).unwrap();
            m.split_pane(
                p,
                ozmux_mux::SplitOrientation::Horizontal,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap()
        });
        run_mux_op(&mut mux_app, |m| {
            let ws = m.active_workspace();
            m.close_workspace(ws).unwrap()
        });
    }

    #[test]
    fn split_after_horizontal_creates_two_pane_split() {
        // Both oracle and mux sides now go through MultiplexerCommands → Mux;
        // assert the resulting ECS tree directly and that mirror_matches holds.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        let p1 = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let pane_count = app
            .world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(pane_count, 2, "after split: 2 panes");

        let ws = active_workspace(&mut app);
        let split_ent = split_under_workspace_root(&app, ws);
        assert_eq!(
            app.world().get::<SplitNode>(split_ent).unwrap().orientation,
            SplitOrientation::Horizontal,
            "horizontal split"
        );
        let kids: Vec<Entity> = app
            .world()
            .get::<Children>(split_ent)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(kids, vec![p0, p1], "After: p0 first, new pane second");
        assert_eq!(
            app.world().get::<ActivePane>(ws).map(|a| a.0),
            Some(p1),
            "new pane is active"
        );
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn split_before_vertical_creates_two_pane_split() {
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        let p1 = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::Before, SplitOrientation::Vertical)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let pane_count = app
            .world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(pane_count, 2, "after split: 2 panes");

        let ws = active_workspace(&mut app);
        let split_ent = split_under_workspace_root(&app, ws);
        assert_eq!(
            app.world().get::<SplitNode>(split_ent).unwrap().orientation,
            SplitOrientation::Vertical,
            "vertical split"
        );
        let kids: Vec<Entity> = app
            .world()
            .get::<Children>(split_ent)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(kids, vec![p1, p0], "Before: new pane first, p0 second");
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn split_after_vertical_creates_two_pane_split() {
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        let p1 = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Vertical)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let ws = active_workspace(&mut app);
        let split_ent = split_under_workspace_root(&app, ws);
        assert_eq!(
            app.world().get::<SplitNode>(split_ent).unwrap().orientation,
            SplitOrientation::Vertical,
        );
        let kids: Vec<Entity> = app
            .world()
            .get::<Children>(split_ent)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(kids, vec![p0, p1], "After: p0 first, new pane second");
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn split_before_horizontal_creates_two_pane_split() {
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        let p1 = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(
                    p0,
                    crate::layout::Side::Before,
                    SplitOrientation::Horizontal,
                )
                .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let ws = active_workspace(&mut app);
        let split_ent = split_under_workspace_root(&app, ws);
        assert_eq!(
            app.world().get::<SplitNode>(split_ent).unwrap().orientation,
            SplitOrientation::Horizontal,
        );
        let kids: Vec<Entity> = app
            .world()
            .get::<Children>(split_ent)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(kids, vec![p1, p0], "Before: new pane first, p0 second");
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn close_pane_reduces_pane_count_and_mirror_matches() {
        // Three-pane tree → close the newest pane → 2 panes remain (exercises
        // LayoutChanged, not WorkspaceRootChanged). mirror_matches must hold.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Horizontal)
                    .unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let p2 = {
            let ws = active_workspace(&mut app);
            app.world().get::<ActivePane>(ws).unwrap().0
        };
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p2, crate::layout::Side::After, SplitOrientation::Vertical)
                    .unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let to_close = {
            let ws = active_workspace(&mut app);
            app.world().get::<ActivePane>(ws).unwrap().0
        };
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.close_pane(to_close).unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let pane_count = app
            .world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(pane_count, 2, "2 panes remain after close");
        assert!(
            app.world_mut().get_entity(to_close).is_err(),
            "closed pane despawned"
        );
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn close_to_root_reduces_to_single_pane_and_mirror_matches() {
        // Two-pane workspace → close the active pane → 1 pane remains
        // (exercises WorkspaceRootChanged). mirror_matches must hold.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Horizontal)
                    .unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let to_close = {
            let ws = active_workspace(&mut app);
            app.world().get::<ActivePane>(ws).unwrap().0
        };
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.close_pane(to_close).unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let pane_count = app
            .world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(pane_count, 1, "only p0 remains after close");
        let ws = active_workspace(&mut app);
        assert_eq!(
            app.world().get::<ActivePane>(ws).map(|a| a.0),
            Some(p0),
            "p0 is active after close"
        );
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn swap_pane_reorders_children_and_mirror_matches() {
        // Two-pane horizontal split: p0 (first), p1 (second / active).
        // Swap p0 Next → p1 first, p0 second. mirror_matches must hold.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        let p1 = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.swap_pane(p0, crate::swap::SwapOffset::Next).unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let ws = active_workspace(&mut app);
        let split_ent = split_under_workspace_root(&app, ws);
        let kids: Vec<Entity> = app
            .world()
            .get::<Children>(split_ent)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(kids, vec![p1, p0], "after swap: p1 first, p0 second");
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn swap_cross_parent_reorders_and_mirror_matches() {
        // Build tree S( S2(p0, p2), p1 ). DFS: [p0, p2, p1].
        // Swap p2 (index 1) with its Next neighbor p1 (index 2) — cross-parent.
        // mirror_matches must hold after the swap.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Horizontal)
                    .unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let p2 = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Vertical)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.swap_pane(p2, crate::swap::SwapOffset::Next).unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let pane_count = app
            .world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(
            pane_count, 3,
            "3 panes still present after cross-parent swap"
        );
        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after cross-parent swap"
        );
    }

    #[test]
    fn break_surface_to_pane_creates_new_pane_with_moved_surface() {
        // Give the bootstrap pane a second surface, then break it into a new pane.
        // Assert: 2 panes exist, the new pane holds the moved surface as its
        // active surface, and mirror_matches holds.
        let mut app = make_mux_app();

        let pane = only_pane(&mut app);
        let second = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.add_surface(pane, SurfaceKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let new_pane = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.break_surface_to_pane(
                    second,
                    crate::layout::Side::After,
                    SplitOrientation::Horizontal,
                )
                .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let pane_count = app
            .world_mut()
            .query_filtered::<Entity, With<PaneMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(pane_count, 2, "break creates a second pane");
        assert_eq!(
            app.world().get::<ActiveSurface>(new_pane).map(|a| a.0),
            Some(second),
            "new pane's active surface is the moved surface"
        );
        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after break_surface_to_pane"
        );
    }

    #[test]
    fn focus_pane_sets_active_pane_and_mirror_matches() {
        // Split to 2 panes (p1 becomes active after the split). Then focus p0;
        // assert ActivePane = p0 and mirror_matches holds.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Horizontal)
                    .unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let ws = active_workspace(&mut app);
        let p1 = app.world().get::<ActivePane>(ws).unwrap().0;
        assert_ne!(p0, p1, "p1 is active after split");

        app.world_mut()
            .run_system_once(
                move |mut mux: crate::commands::MultiplexerCommands,
                      ws_q: Query<Entity, With<WorkspaceMarker>>| {
                    let ws = ws_q.iter().next().expect("workspace must exist");
                    mux.set_active_pane(ws, p0).unwrap();
                },
            )
            .unwrap();
        app.world_mut().flush();
        app.update();

        let ws = active_workspace(&mut app);
        assert_eq!(
            app.world().get::<ActivePane>(ws).map(|a| a.0),
            Some(p0),
            "p0 is active after focus"
        );
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn navigate_left_moves_active_pane_to_geometric_neighbor() {
        // Horizontal split → p0 (left) and p1 (right). After the split p1 is
        // active. Navigate Left: the active pane must become p0, and the mirror
        // must still be consistent.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let active = m.active_pane(ws).unwrap();
            m.split_pane(
                active,
                ozmux_mux::SplitOrientation::Horizontal,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap()
        });

        // p1 (the new pane) is now active; navigate Left should land on p0.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let active = m.active_pane(ws).unwrap();
            m.navigate(active, ozmux_mux::PaneDirection::Left).unwrap()
        });

        let ws = active_workspace(&mut app);
        assert_eq!(
            app.world().get::<ActivePane>(ws).map(|a| a.0),
            Some(p0),
            "navigate Left must land on p0 (the left pane)"
        );
        let s = app.world().resource::<MuxState>();
        assert!(mirror_matches(app.world(), s).is_ok(), "mirror_matches");
    }

    #[test]
    fn resize_pane_moves_split_ratio_off_center() {
        // 2-pane horizontal split: set workspace size, resize p0 Right by 10 cells.
        // Assert: the split's children have different flex_grow values (ratio shifted
        // from the initial 0.5/0.5), and mirror_matches holds.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);
        let ws = active_workspace(&mut app);
        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.split_pane(p0, crate::layout::Side::After, SplitOrientation::Horizontal)
                    .unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        app.world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.set_workspace_dimensions(ws, 80, 24);
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let outcome = app
            .world_mut()
            .run_system_once(move |mut mux: crate::commands::MultiplexerCommands| {
                mux.resize_pane(p0, crate::direction::PaneDirection::Right, 10)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        assert_eq!(
            outcome,
            crate::resize::ResizePaneOutcome::Applied,
            "resize must apply for a 2-pane split with workspace size set"
        );

        let split_ent = split_under_workspace_root(&app, ws);
        let kids: Vec<Entity> = app
            .world()
            .get::<Children>(split_ent)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(kids.len(), 2, "split must have 2 children");
        let lhs_grow = app
            .world()
            .get::<Node>(kids[0])
            .map(|n| n.flex_grow)
            .unwrap_or(0.0);
        let rhs_grow = app
            .world()
            .get::<Node>(kids[1])
            .map(|n| n.flex_grow)
            .unwrap_or(0.0);
        let ratio = split_ratio(lhs_grow, rhs_grow);
        assert!(
            (ratio - 0.5).abs() > 1e-4,
            "ratio must be off 0.5 after resize; got ratio={ratio}"
        );
        assert!(
            lhs_grow > rhs_grow,
            "lhs must be bigger after resize Right: lhs={lhs_grow} rhs={rhs_grow}"
        );
        let s = app.world().resource::<MuxState>();
        assert!(
            mirror_matches(app.world(), s).is_ok(),
            "mirror_matches after resize_pane"
        );
    }

    #[test]
    fn created_pane_id_finds_pane_created_and_no_surface_for_split() {
        let mut mux = ozmux_mux::Mux::new();
        let ws = mux.active_workspace();
        let p = mux.active_pane(ws).unwrap();
        let events = mux
            .split_pane(
                p,
                ozmux_mux::SplitOrientation::Horizontal,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap();
        assert!(created_pane_id(&events).is_some());
        assert!(
            single_spawned_surface_id(&events).is_none(),
            "split emits no SurfaceSpawned"
        );
    }

    #[test]
    fn single_spawned_surface_id_finds_add_surface() {
        let mut mux = ozmux_mux::Mux::new();
        let ws = mux.active_workspace();
        let p = mux.active_pane(ws).unwrap();
        let events = mux
            .spawn_surface(p, ozmux_mux::SurfaceKind::Terminal)
            .unwrap();
        assert!(single_spawned_surface_id(&events).is_some());
    }

    #[test]
    fn lift_maps_pane_not_found() {
        // `mux` stays at its initial state; a separate `mux2` generates a stale
        // PaneId that is not registered in `mux`'s reverse maps.
        let mux = ozmux_mux::Mux::new();
        let mut mux2 = ozmux_mux::Mux::new();
        let ws2 = mux2.active_workspace();
        let p2 = mux2.active_pane(ws2).unwrap();
        // Split to get a second pane, close the second to get its stale id in mux.
        let events = mux2
            .split_pane(
                p2,
                ozmux_mux::SplitOrientation::Horizontal,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap();
        let new_pane = match events[0] {
            ozmux_mux::MuxEvent::PaneCreated { pane, .. } => pane,
            _ => panic!("first event must be PaneCreated"),
        };
        // Close the new pane in mux2 — now new_pane is stale in mux2.
        mux2.close_pane(new_pane).unwrap();
        // new_pane is now stale in mux2. Try surfaces(new_pane) → PaneNotFound.
        let err = mux2.surfaces(new_pane).unwrap_err();
        assert!(matches!(err, ozmux_mux::MuxError::PaneNotFound(_)));
        // Lift using mux (where new_pane was never registered in state.panes).
        let state = MuxState::new(mux);
        let lifted = lift(&state, err);
        assert!(
            matches!(lifted, crate::error::MultiplexerError::PaneNotFound(_)),
            "lifted error must be MultiplexerError::PaneNotFound, got: {lifted:?}"
        );
    }

    #[test]
    fn nested_close_active_pane_matches_mux_choice() {
        // Build tree [p0, [p1(active), p2]]:
        //   - bootstrap gives p0 (the only pane);
        //   - split p0 After/Horizontal → [p0, p1]; p1 becomes active;
        //   - split p1 After/Vertical   → [p0, [p1, p2]]; p2 becomes active;
        //   - focus p1 so the active pane IS the inner-left pane;
        //   - close p1 (the active one in the nested split).
        //
        // The Mux's `close_pane` repoints the active to `leftmost_pane(sibling)`:
        // sibling of p1 in its split is p2 → survivor = p2.
        //
        // The old observer walked the DFS-ordered leaf list [p0, p1, p2] and
        // picked the first leaf != dying (p0), which conflicts with the Mux's
        // choice. This test asserts ECS ActivePane == Mux's choice after the
        // close, exercising the exact divergence path.
        let mut app = make_mux_app();

        let p0 = only_pane(&mut app);

        // Split p0 After/Horizontal → [p0, p1]; p1 is now active.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let active = m.active_pane(ws).unwrap();
            m.split_pane(
                active,
                ozmux_mux::SplitOrientation::Horizontal,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap()
        });

        let p1 = {
            let ws = active_workspace(&mut app);
            app.world().get::<ActivePane>(ws).unwrap().0
        };
        assert_ne!(p0, p1, "p1 is a new pane distinct from p0");

        // Split p1 After/Vertical → [p0, [p1, p2]]; p2 is now active.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let active = m.active_pane(ws).unwrap();
            m.split_pane(
                active,
                ozmux_mux::SplitOrientation::Vertical,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap()
        });

        // Focus p1 so the active pane is the inner-left one.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let ordered = m.ordered_panes(ws).unwrap();
            // DFS order after splits: [p0, p1, p2]. p1 is index 1.
            m.focus_pane(ordered[1]).unwrap()
        });

        let ws = active_workspace(&mut app);
        assert_eq!(
            app.world().get::<ActivePane>(ws).map(|a| a.0),
            Some(p1),
            "p1 must be active before the close"
        );

        // Close p1 (the active pane). Mux picks its sibling in the inner split
        // (p2) via leftmost_pane. The old observer would have picked p0 (DFS
        // first != dying), causing a divergence.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let active = m.active_pane(ws).unwrap();
            m.close_pane(active).unwrap()
        });

        // Read what the Mux chose.
        let ws_ent = active_workspace(&mut app);
        let state = app.world().resource::<MuxState>();
        let mux_active_id = state.mux.active_pane(state.mux.active_workspace()).unwrap();
        let mux_active_ent = state.panes[mux_active_id];

        let ecs_active_ent = app.world().get::<ActivePane>(ws_ent).map(|a| a.0);
        assert_eq!(
            ecs_active_ent,
            Some(mux_active_ent),
            "ECS ActivePane must match the Mux's choice after nested-close; \
             Mux chose {mux_active_ent:?}, ECS has {ecs_active_ent:?}"
        );

        let result = mirror_matches(app.world(), state);
        assert!(
            result.is_ok(),
            "mirror_matches must hold after nested close: {result:?}"
        );

        // Additionally verify p2 (sibling-leftmost) is the survivor, not p0.
        assert_ne!(
            mux_active_ent, p0,
            "Mux must not pick p0 (DFS-first) as survivor; \
             it must pick the direct sibling (p2)"
        );
        assert_ne!(
            mux_active_ent, p1,
            "closed pane p1 must not be the active pane"
        );
    }

    #[test]
    fn multi_step_sequence_stays_consistent() {
        let mut app = make_mux_app();

        // Step 1: split p0 → p1 (p1 becomes active).
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let p0 = m.active_pane(ws).unwrap();
            m.split_pane(
                p0,
                ozmux_mux::SplitOrientation::Horizontal,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap()
        });

        // Step 2: split p1 → p2 (p2 becomes active).
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let p1 = m.active_pane(ws).unwrap();
            m.split_pane(
                p1,
                ozmux_mux::SplitOrientation::Vertical,
                ozmux_mux::Side::After,
                ozmux_mux::SurfaceKind::Terminal,
            )
            .unwrap()
        });

        // Step 3: close the middle pane (p1, ordered index 1). Active is p2
        // (index 2); close_pane on p1 promotes p2 (or p0) and collapses the
        // inner split without reaching the root.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let p1 = m.ordered_panes(ws).unwrap()[1];
            m.close_pane(p1).unwrap()
        });

        // Step 4: swap the two remaining panes.
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let first = m.ordered_panes(ws).unwrap()[0];
            m.swap_pane(first, ozmux_mux::SwapOffset::Next).unwrap()
        });

        // Step 5: close down to the root (close the active pane so only 1
        // pane remains, exercising WorkspaceRootChanged).
        run_mux_op(&mut app, |m| {
            let ws = m.active_workspace();
            let active = m.active_pane(ws).unwrap();
            m.close_pane(active).unwrap()
        });
    }
}
