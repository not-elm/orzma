//! Mirrors an `ozmux_mux::Mux` into the Bevy ECS: the `MuxState` Resource,
//! `MuxId` forward-lookup components, the apply-handler that turns
//! `MuxEvent`s into ECS mutations, and a consistency checker. Plan 2b-1
//! builds this as library code; the source-of-truth flip is Plan 2b-2.

use crate::components::{
    ActivePane, ActiveSurface, BrowserProfile, CopyMode, OwningWorkspace, PaneMarker, SplitNode,
    SurfaceKind, SurfaceMarker, SurfaceOf, WorkspaceMarker, WorkspaceUiSubtree,
};
use crate::layout::{
    SplitOrientation, child_flex, pane_frame_node, split_node_bundle, split_ratio,
};
use bevy::prelude::*;
use ozmux_mux::{LayoutNode, PaneId, SplitId, SurfaceId, WorkspaceId};
use slotmap::SecondaryMap;

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

    /// Realizes the Mux's current tree (active session's workspace, layout tree,
    /// surfaces) into the ECS, recording every reverse map + WorkspaceUiSubtree +
    /// ChildOf exactly as create_workspace/split_in_tree would.
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

/// Spawns a pane entity with the exact bundle `create_workspace`/`split_pane_inner` uses.
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

    check_node(world, state, top_ent, &layout, "")?;

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
            Ok(())
        }
        LayoutNode::Split {
            id,
            orientation,
            ratio,
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
            let lhs_grow = world
                .get::<Node>(kids[0])
                .map(|n| n.flex_grow)
                .unwrap_or(0.0);
            let rhs_grow = world
                .get::<Node>(kids[1])
                .map(|n| n.flex_grow)
                .unwrap_or(0.0);
            let ecs_ratio = split_ratio(lhs_grow, rhs_grow);
            if (ecs_ratio - ratio).abs() > 1e-4 {
                return Err(Mismatch(format!(
                    "path {path:?}: split ratio mismatch: ECS={ecs_ratio} mux={ratio}"
                )));
            }

            check_node(world, state, kids[0], first, &format!("{path}/0"))?;
            check_node(world, state, kids[1], second, &format!("{path}/1"))?;
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

    /// Checks that two apps produce structurally equivalent layout trees for
    /// all workspaces. Panics with a clear message on any mismatch.
    ///
    /// Walks `WorkspaceMarker` entities in each app, matches them in creation
    /// order via `WorkspaceUiSubtree`, and compares the subtree recursively:
    /// - At a `SplitNode`: same orientation, and the ratio (normalized from
    ///   `flex_grow` pair) matches within 1e-4.
    /// - At a `PaneMarker` leaf: same active-surface `SurfaceKind` discriminant
    ///   and same total surface count.
    /// - The workspace's `ActivePane` entity maps to the same tree position.
    pub(crate) fn assert_layout_equiv(oracle: &mut App, mux_app: &mut App) {
        let oracle_workspaces = collect_workspaces(oracle.world_mut());
        let mux_workspaces = collect_workspaces(mux_app.world_mut());

        assert_eq!(
            oracle_workspaces.len(),
            mux_workspaces.len(),
            "workspace count mismatch: oracle={} mux={}",
            oracle_workspaces.len(),
            mux_workspaces.len(),
        );

        for (i, (o_ws, m_ws)) in oracle_workspaces
            .iter()
            .zip(mux_workspaces.iter())
            .enumerate()
        {
            let oracle_world = oracle.world();
            let mux_world = mux_app.world();

            let o_container = oracle_world
                .get::<WorkspaceUiSubtree>(*o_ws)
                .unwrap_or_else(|| panic!("oracle workspace[{i}] missing WorkspaceUiSubtree"))
                .0;
            let m_container = mux_world
                .get::<WorkspaceUiSubtree>(*m_ws)
                .unwrap_or_else(|| panic!("mux workspace[{i}] missing WorkspaceUiSubtree"))
                .0;

            let o_top = first_child(oracle_world, o_container)
                .unwrap_or_else(|| panic!("oracle workspace[{i}] container has no children"));
            let m_top = first_child(mux_world, m_container)
                .unwrap_or_else(|| panic!("mux workspace[{i}] container has no children"));

            let mut oracle_active_pos: Option<Vec<usize>> = None;
            let mut mux_active_pos: Option<Vec<usize>> = None;
            let o_active = oracle_world.get::<ActivePane>(*o_ws).map(|a| a.0);
            let m_active = mux_world.get::<ActivePane>(*m_ws).map(|a| a.0);

            compare_nodes(
                oracle_world,
                o_top,
                mux_world,
                m_top,
                &[],
                i,
                o_active,
                m_active,
                &mut oracle_active_pos,
                &mut mux_active_pos,
            );

            assert_eq!(
                oracle_active_pos, mux_active_pos,
                "workspace[{i}] ActivePane tree position mismatch: oracle={oracle_active_pos:?} mux={mux_active_pos:?}",
            );
        }
    }

    /// Collects all `WorkspaceMarker` entities sorted by entity index (creation-order
    /// proxy; adequate for single-workspace tests).
    fn collect_workspaces(world: &mut World) -> Vec<Entity> {
        let mut q = world.query_filtered::<Entity, With<WorkspaceMarker>>();
        let mut v: Vec<Entity> = q.iter(world).collect();
        v.sort_by_key(|e| e.index());
        v
    }

    fn first_child(world: &World, parent: Entity) -> Option<Entity> {
        world.get::<Children>(parent).and_then(|c| c.iter().next())
    }

    /// Recursive structural comparison of two layout subtree nodes across worlds.
    #[expect(
        clippy::too_many_arguments,
        reason = "recursive tree walker with full context"
    )]
    fn compare_nodes(
        o_world: &World,
        o_ent: Entity,
        m_world: &World,
        m_ent: Entity,
        path: &[usize],
        ws_idx: usize,
        o_active: Option<Entity>,
        m_active: Option<Entity>,
        oracle_active_pos: &mut Option<Vec<usize>>,
        mux_active_pos: &mut Option<Vec<usize>>,
    ) {
        let o_is_pane = o_world.get::<PaneMarker>(o_ent).is_some();
        let m_is_pane = m_world.get::<PaneMarker>(m_ent).is_some();
        let o_is_split = o_world.get::<SplitNode>(o_ent).is_some();
        let m_is_split = m_world.get::<SplitNode>(m_ent).is_some();

        assert_eq!(
            o_is_pane, m_is_pane,
            "ws[{ws_idx}] path {path:?}: pane marker mismatch (oracle={o_is_pane} mux={m_is_pane})",
        );
        assert_eq!(
            o_is_split, m_is_split,
            "ws[{ws_idx}] path {path:?}: split marker mismatch (oracle={o_is_split} mux={m_is_split})",
        );

        if o_is_pane {
            if o_active == Some(o_ent) {
                *oracle_active_pos = Some(path.to_vec());
            }
            if m_active == Some(m_ent) {
                *mux_active_pos = Some(path.to_vec());
            }

            let o_active_surf = o_world
                .get::<ActiveSurface>(o_ent)
                .unwrap_or_else(|| {
                    panic!("ws[{ws_idx}] path {path:?}: oracle pane missing ActiveSurface")
                })
                .0;
            let m_active_surf = m_world
                .get::<ActiveSurface>(m_ent)
                .unwrap_or_else(|| {
                    panic!("ws[{ws_idx}] path {path:?}: mux pane missing ActiveSurface")
                })
                .0;

            let o_kind = o_world
                .get::<SurfaceKind>(o_active_surf)
                .unwrap_or_else(|| {
                    panic!("ws[{ws_idx}] path {path:?}: oracle active surface missing SurfaceKind")
                });
            let m_kind = m_world
                .get::<SurfaceKind>(m_active_surf)
                .unwrap_or_else(|| {
                    panic!("ws[{ws_idx}] path {path:?}: mux active surface missing SurfaceKind")
                });

            assert!(
                surface_kind_discriminant_eq(o_kind, m_kind),
                "ws[{ws_idx}] path {path:?}: active surface kind mismatch: oracle={o_kind:?} mux={m_kind:?}",
            );

            let o_node = o_world
                .get::<Node>(o_ent)
                .unwrap_or_else(|| panic!("ws[{ws_idx}] path {path:?}: oracle pane missing Node"));
            let m_node = m_world
                .get::<Node>(m_ent)
                .unwrap_or_else(|| panic!("ws[{ws_idx}] path {path:?}: mux pane missing Node"));
            assert_eq!(
                o_node.flex_direction, m_node.flex_direction,
                "ws[{ws_idx}] path {path:?}: pane flex_direction mismatch",
            );
            assert_eq!(
                o_node.padding, m_node.padding,
                "ws[{ws_idx}] path {path:?}: pane padding mismatch",
            );

            let o_surf_count = o_world
                .get::<crate::components::Surfaces>(o_ent)
                .map(|s| s.iter().count())
                .unwrap_or(0);
            let m_surf_count = m_world
                .get::<crate::components::Surfaces>(m_ent)
                .map(|s| s.iter().count())
                .unwrap_or(0);

            assert_eq!(
                o_surf_count, m_surf_count,
                "ws[{ws_idx}] path {path:?}: surface count mismatch: oracle={o_surf_count} mux={m_surf_count}",
            );
        } else if o_is_split {
            let o_split = o_world.get::<SplitNode>(o_ent).expect("oracle split node");
            let m_split = m_world.get::<SplitNode>(m_ent).expect("mux split node");

            assert_eq!(
                o_split.orientation, m_split.orientation,
                "ws[{ws_idx}] path {path:?}: split orientation mismatch",
            );

            let o_kids: Vec<Entity> = o_world
                .get::<Children>(o_ent)
                .map(|c| c.iter().collect())
                .unwrap_or_default();
            let m_kids: Vec<Entity> = m_world
                .get::<Children>(m_ent)
                .map(|c| c.iter().collect())
                .unwrap_or_default();

            assert_eq!(
                o_kids.len(),
                2,
                "ws[{ws_idx}] path {path:?}: oracle split must have 2 children, got {}",
                o_kids.len(),
            );
            assert_eq!(
                m_kids.len(),
                2,
                "ws[{ws_idx}] path {path:?}: mux split must have 2 children, got {}",
                m_kids.len(),
            );

            let o_lhs_grow = o_world
                .get::<Node>(o_kids[0])
                .map(|n| n.flex_grow)
                .unwrap_or(0.0);
            let o_rhs_grow = o_world
                .get::<Node>(o_kids[1])
                .map(|n| n.flex_grow)
                .unwrap_or(0.0);
            let m_lhs_grow = m_world
                .get::<Node>(m_kids[0])
                .map(|n| n.flex_grow)
                .unwrap_or(0.0);
            let m_rhs_grow = m_world
                .get::<Node>(m_kids[1])
                .map(|n| n.flex_grow)
                .unwrap_or(0.0);

            let o_ratio = split_ratio(o_lhs_grow, o_rhs_grow);
            let m_ratio = split_ratio(m_lhs_grow, m_rhs_grow);

            assert!(
                (o_ratio - m_ratio).abs() < 1e-4,
                "ws[{ws_idx}] path {path:?}: split ratio mismatch: oracle={o_ratio} mux={m_ratio}",
            );

            let mut first_path = path.to_vec();
            first_path.push(0);
            let mut second_path = path.to_vec();
            second_path.push(1);

            compare_nodes(
                o_world,
                o_kids[0],
                m_world,
                m_kids[0],
                &first_path,
                ws_idx,
                o_active,
                m_active,
                oracle_active_pos,
                mux_active_pos,
            );
            compare_nodes(
                o_world,
                o_kids[1],
                m_world,
                m_kids[1],
                &second_path,
                ws_idx,
                o_active,
                m_active,
                oracle_active_pos,
                mux_active_pos,
            );
        } else {
            panic!(
                "ws[{ws_idx}] path {path:?}: entity is neither PaneMarker nor SplitNode (oracle={o_ent:?} mux={m_ent:?})",
            );
        }
    }

    /// True when two `SurfaceKind` values have the same discriminant.
    fn surface_kind_discriminant_eq(a: &SurfaceKind, b: &SurfaceKind) -> bool {
        matches!(
            (a, b),
            (SurfaceKind::Terminal, SurfaceKind::Terminal)
                | (SurfaceKind::Extension { .. }, SurfaceKind::Extension { .. })
                | (SurfaceKind::Browser { .. }, SurfaceKind::Browser { .. })
        )
    }

    #[test]
    fn materialize_matches_create_workspace_bootstrap() {
        // Oracle app: MultiplexerPlugin + create_workspace.
        // NOTE: Mux::new() names its initial workspace "0" (created_at=0); match that name.
        let mut oracle = App::new();
        oracle.add_plugins(MinimalPlugins);
        oracle.add_plugins(MultiplexerPlugin);

        oracle
            .world_mut()
            .run_system_once(|mut mux: crate::commands::MultiplexerCommands| {
                mux.create_workspace(Some("0".to_owned()));
            })
            .unwrap();
        oracle.world_mut().flush();
        oracle.update();

        // Mux app: insert MuxState and run materialize_snapshot.
        let mut mux_app = App::new();
        mux_app.add_plugins(MinimalPlugins);
        mux_app.add_plugins(MultiplexerPlugin);

        mux_app
            .world_mut()
            .insert_resource(MuxState::new(ozmux_mux::Mux::new()));

        mux_app
            .world_mut()
            .run_system_once(|mut commands: Commands, mut state: ResMut<MuxState>| {
                state.materialize_snapshot(&mut commands);
            })
            .unwrap();
        mux_app.world_mut().flush();
        mux_app.update();

        assert_layout_equiv(&mut oracle, &mut mux_app);
    }

    #[test]
    fn mirror_matches_passes_on_materialized_snapshot() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MultiplexerPlugin);

        app.world_mut()
            .insert_resource(MuxState::new(ozmux_mux::Mux::new()));

        app.world_mut()
            .run_system_once(|mut commands: Commands, mut state: ResMut<MuxState>| {
                state.materialize_snapshot(&mut commands);
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let state = app.world().resource::<MuxState>();
        // NOTE: MuxState must be cloned out first to avoid a shared borrow on
        // world while also passing world — we borrow state fields directly.
        let ws = state.mux.active_workspace();
        let ws_ent = state.workspaces[ws];
        let _ = (ws, ws_ent);

        let result = {
            let world = app.world();
            let s = world.resource::<MuxState>();
            // SAFETY: split borrow — we read state then immediately pass both
            // immutable refs; Rust allows two shared borrows of different fields.
            mirror_matches_with(world, s)
        };
        assert!(result.is_ok(), "mirror_matches failed: {result:?}");
    }

    #[test]
    fn mirror_matches_fails_on_corrupted_ratio() {
        let mut mux = ozmux_mux::Mux::new();
        let ws = mux.active_workspace();
        let p = mux.active_pane(ws).unwrap();
        mux.split_pane(
            p,
            ozmux_mux::SplitOrientation::Horizontal,
            ozmux_mux::Side::After,
            ozmux_mux::SurfaceKind::Terminal,
        )
        .unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MultiplexerPlugin);
        app.world_mut().insert_resource(MuxState::new(mux));

        app.world_mut()
            .run_system_once(|mut commands: Commands, mut state: ResMut<MuxState>| {
                state.materialize_snapshot(&mut commands);
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        // Find a SplitNode entity and corrupt its first child's flex_grow.
        let split_ent = {
            let mut q = app.world_mut().query_filtered::<Entity, With<SplitNode>>();
            q.iter(app.world()).next().expect("split entity must exist")
        };
        let first_child = app
            .world()
            .get::<Children>(split_ent)
            .and_then(|c| c.iter().next())
            .expect("split must have children");
        app.world_mut()
            .get_mut::<Node>(first_child)
            .expect("first child must have Node")
            .flex_grow = 999.0;
        app.update();

        let result = {
            let world = app.world();
            let s = world.resource::<MuxState>();
            mirror_matches_with(world, s)
        };
        assert!(
            result.is_err(),
            "mirror_matches should detect corrupted flex_grow"
        );
    }

    fn mirror_matches_with(world: &World, state: &MuxState) -> Result<(), Mismatch> {
        mirror_matches(world, state)
    }
}
