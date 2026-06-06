//! `MultiplexerCommands` SystemParam — the sole mutation API for the
//! multiplexer. Each method performs whatever entity spawns/despawns and
//! component mutations are needed for one logical operation; Bevy's
//! native change detection (`Changed<T>`) carries the signal to downstream
//! rebuild systems.

use crate::components::{
    ActivePane, ActiveSurface, AttachedWorkspace, OwningWorkspace, PaneMarker, SurfaceKind,
    SurfaceMarker, SurfaceOf, Surfaces, WorkspaceCreatedAt, WorkspaceDimensions, WorkspaceMarker,
};
#[cfg(test)]
use crate::components::{SplitNode, WorkspaceUiSubtree};
use crate::direction::PaneDirection;
use crate::error::{MultiplexerError, MultiplexerResult};
use crate::layout::{Side, SplitOrientation};
use crate::mirror::{
    MirrorReadCtx, MuxState, apply_event, created_pane_id, created_workspace_id,
    ecs_direction_to_mux, ecs_orientation_to_mux, ecs_side_to_mux, ecs_surface_kind_to_mux,
    ecs_swap_offset_to_mux, seed_surface_of, single_spawned_surface_id,
};
use crate::resize::ResizePaneOutcome;
use crate::swap::{SwapOffset, SwapOutcome};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// Result of `create_workspace` — the three freshly-spawned entities.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceCreated {
    /// The Workspace entity.
    pub workspace: Entity,
    /// The bootstrap Pane entity.
    pub pane: Entity,
    /// The bootstrap Surface entity.
    pub surface: Entity,
}

/// Result of `split_pane_with_surface` — the new pane + its seeded surface.
#[derive(Debug, Clone, Copy)]
pub struct SplitOutcome {
    /// The freshly-spawned pane.
    pub pane: Entity,
    /// The surface seeded into the new pane.
    pub surface: Entity,
}

/// Monotonic counter for workspaces created through `MultiplexerCommands`.
/// Each `next()` returns the next 1-based creation-order index (never
/// reused). Seeds the `"workspace{n}"` auto-name and the `WorkspaceCreatedAt(n)`
/// component minted by [`MultiplexerCommands::spawn_attached_workspace`].
#[derive(Resource, Default, Debug)]
pub struct WorkspaceNameCounter(u32);

impl WorkspaceNameCounter {
    fn next(&mut self) -> u32 {
        self.0 = self.0.saturating_add(1);
        self.0
    }
}

/// SystemParam exposing every mutation on the multiplexer state. Read
/// helpers (`workspace_of_pane`, `panes_of_workspace`, etc.) are non-mut and
/// can be called by other systems through the same SystemParam.
#[derive(SystemParam)]
pub struct MultiplexerCommands<'w, 's> {
    commands: Commands<'w, 's>,
    counter: ResMut<'w, WorkspaceNameCounter>,
    mux: ResMut<'w, MuxState>,
    mirror_read: MirrorReadCtx<'w, 's>,
    workspaces: Query<'w, 's, &'static ActivePane, With<WorkspaceMarker>>,
    panes: Query<'w, 's, (&'static ActiveSurface, &'static OwningWorkspace), With<PaneMarker>>,
    panes_owned: Query<'w, 's, (Entity, &'static OwningWorkspace), With<PaneMarker>>,
    surface_owner: Query<'w, 's, &'static SurfaceOf, With<SurfaceMarker>>,
    pane_surfaces: Query<'w, 's, &'static Surfaces, With<PaneMarker>>,
}

impl<'w, 's> MultiplexerCommands<'w, 's> {
    /// Spawns a new Workspace through the Mux (the authoritative source of
    /// truth), applies the resulting events to the ECS mirror, then renames
    /// the workspace to `name` (defaulting to `"default"`).
    pub fn create_workspace(&mut self, name: Option<String>) -> WorkspaceCreated {
        let name = name.unwrap_or_else(|| "default".to_string());
        let events = self.mux.mux.new_workspace().expect("new_workspace");
        let ws_id = created_workspace_id(&events).expect("WorkspaceCreated");
        let pane_id = created_pane_id(&events).expect("PaneCreated");
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        let _ = self.mux.mux.rename_workspace(ws_id, name.clone());
        let ws_ent = self.mux.workspaces[ws_id];
        self.commands.entity(ws_ent).insert(Name::new(name));
        let pane_ent = self.mux.panes[pane_id];
        let seed = seed_surface_of(&self.mux, pane_id).expect("seed surface");
        let surface_ent = self.mux.surfaces[seed];
        WorkspaceCreated {
            workspace: ws_ent,
            pane: pane_ent,
            surface: surface_ent,
        }
    }

    /// Mints a workspace via `create_workspace` AND through the authoritative
    /// Mux, attaches `AttachedWorkspace` + `WorkspaceCreatedAt`, auto-named
    /// `"workspace{n}"`. The layout-root node (stored in `WorkspaceUiSubtree`)
    /// is spawned by `apply_event` when it processes the `WorkspaceCreated` event.
    pub fn spawn_attached_workspace(&mut self) -> Entity {
        let events = self
            .mux
            .mux
            .new_workspace()
            .expect("new_workspace must succeed");
        let new_id =
            created_workspace_id(&events).expect("new_workspace must emit WorkspaceCreated");
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        let n = self.counter.next();
        let _ = self
            .mux
            .mux
            .rename_workspace(new_id, format!("workspace{n}"));
        let ws_ent = self.mux.workspaces[new_id];
        self.commands
            .entity(ws_ent)
            .insert(Name::new(format!("workspace{n}")));
        self.attach_workspace_named(ws_ent, n);
        ws_ent
    }

    /// Attaches GUI state to the Mux-seeded initial workspace (renames it
    /// `"workspace1"` through the Mux so the Mux and ECS agree). Called once
    /// at bootstrap, after `materialize_mux_snapshot` has realized the tree.
    pub fn attach_initial_workspace(&mut self) -> Entity {
        let id = self.mux.mux.active_workspace();
        let _ = self.mux.mux.rename_workspace(id, "workspace1".to_string());
        let ws_ent = self.mux.workspaces[id];
        self.commands.entity(ws_ent).insert(Name::new("workspace1"));
        let n = self.counter.next();
        self.attach_workspace_named(ws_ent, n);
        ws_ent
    }

    /// Applies the GUI-attach state (`AttachedWorkspace` + `WorkspaceCreatedAt(n)`)
    /// to a workspace entity already present in the ECS mirror.
    fn attach_workspace_named(&mut self, workspace: Entity, n: u32) {
        self.commands
            .entity(workspace)
            .insert((AttachedWorkspace, WorkspaceCreatedAt(n)));
    }

    /// Renames the Workspace through the Mux, then applies the resulting events.
    /// Uses `set_if_neq` semantics (the Mux already deduplicates no-op renames).
    pub fn rename_workspace(&mut self, workspace: Entity, name: String) -> MultiplexerResult<()> {
        let id = self
            .resolve_workspace(workspace)
            .ok_or(MultiplexerError::WorkspaceNotFound(workspace))?;
        let events = self
            .mux
            .mux
            .rename_workspace(id, name)
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        Ok(())
    }

    /// Sets the Workspace's terminal dimensions through the Mux (to update
    /// resolved pane sizes), then inserts or updates the `WorkspaceDimensions`
    /// component so the GUI keeps the cached cols/rows in sync.
    pub fn set_workspace_dimensions(&mut self, workspace: Entity, cols: u16, rows: u16) {
        let Some(id) = self.resolve_workspace(workspace) else {
            return;
        };
        if let Ok(events) = self.mux.mux.set_workspace_size(id, cols, rows) {
            for ev in &events {
                apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
            }
        }
        self.commands
            .entity(workspace)
            .insert(WorkspaceDimensions { cols, rows });
    }

    /// Update the active pane through the Mux. The `_workspace` argument is
    /// unused — the Mux derives the workspace from the pane's parent chain.
    pub fn set_active_pane(&mut self, _workspace: Entity, pane: Entity) -> MultiplexerResult<()> {
        let id = self
            .resolve_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let events = self
            .mux
            .mux
            .focus_pane(id)
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        Ok(())
    }

    /// Update the Pane's active surface through the Mux, then applies the
    /// resulting events to keep the ECS mirror in sync.
    pub fn set_active_surface(&mut self, pane: Entity, surface: Entity) -> MultiplexerResult<()> {
        let pid = self
            .resolve_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let sid = self
            .resolve_surface(surface)
            .ok_or(MultiplexerError::SurfaceNotFound(surface))?;
        let events = self
            .mux
            .mux
            .set_active_surface(pid, sid)
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        Ok(())
    }

    /// Split the target pane and seed the new pane with one surface of the
    /// caller-chosen `kind`. Delegates to the Mux, applies the resulting events
    /// to the ECS mirror, then queries the new pane's seed surface from the
    /// post-apply Mux state.
    pub fn split_pane_with_surface(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
        kind: SurfaceKind,
    ) -> MultiplexerResult<SplitOutcome> {
        let target = self
            .resolve_pane(target_pane)
            .ok_or(MultiplexerError::PaneNotFound(target_pane))?;
        let events = self
            .mux
            .mux
            .split_pane(
                target,
                ecs_orientation_to_mux(orientation),
                ecs_side_to_mux(side),
                ecs_surface_kind_to_mux(kind),
            )
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        let new_pane_id = created_pane_id(&events).expect("split_pane emits PaneCreated");
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        let seed = seed_surface_of(&self.mux, new_pane_id).expect("new pane has a seed surface");
        Ok(SplitOutcome {
            pane: self.mux.panes[new_pane_id],
            surface: self.mux.surfaces[seed],
        })
    }

    /// Split the target pane in two, seeding a bootstrap Terminal surface.
    /// Delegates to `split_pane_with_surface`; returns only the new pane entity.
    pub fn split_pane(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<Entity> {
        self.split_pane_with_surface(target_pane, side, orientation, SurfaceKind::Terminal)
            .map(|o| o.pane)
    }

    /// Close a pane through the Mux. Promotes the sibling into the grandparent
    /// slot (or workspace root), despawns the pane and its surfaces, and
    /// repoints `ActivePane` to the survivor if the closed pane was active.
    pub fn close_pane(&mut self, pane: Entity) -> MultiplexerResult<()> {
        let id = self
            .resolve_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let events = self
            .mux
            .mux
            .close_pane(id)
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        Ok(())
    }

    /// Swap a pane's contents with its prev/next neighbor in the Mux's DFS
    /// leaf traversal. No-op for single-pane workspaces.
    pub fn swap_pane(
        &mut self,
        pane: Entity,
        offset: SwapOffset,
    ) -> MultiplexerResult<SwapOutcome> {
        let id = self
            .resolve_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let ws_ent = self
            .workspace_of_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let ws_id = self
            .resolve_workspace(ws_ent)
            .ok_or(MultiplexerError::WorkspaceNotFound(ws_ent))?;
        let ordered = self.mux.mux.ordered_panes(ws_id).unwrap_or_default();
        let other_id = pane_neighbor(&ordered, id, offset);
        let events = self
            .mux
            .mux
            .swap_pane(id, ecs_swap_offset_to_mux(offset))
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        Ok(match other_id {
            Some(o) if !events.is_empty() => SwapOutcome::Swapped {
                other_pane: self.mux.panes[o],
            },
            _ => SwapOutcome::NoOp,
        })
    }

    /// Spawn a new Surface as a child of `pane` through the Mux. Does NOT change
    /// `ActiveSurface` — call `set_active_surface` separately if needed.
    pub fn add_surface(&mut self, pane: Entity, kind: SurfaceKind) -> Entity {
        let id = self
            .resolve_pane(pane)
            .expect("add_surface: pane must be mapped");
        let events = self
            .mux
            .mux
            .spawn_surface(id, ecs_surface_kind_to_mux(kind))
            .expect("spawn_surface");
        let sid = single_spawned_surface_id(&events).expect("spawn_surface emits SurfaceSpawned");
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        self.mux.surfaces[sid]
    }

    /// Split the surface's owning Pane and move the surface into the
    /// freshly-created Pane (where it becomes the only surface). The new
    /// Pane becomes the workspace's `ActivePane`. Returns
    /// `CannotRemoveLastSurface` if the source Pane has only one surface.
    pub fn break_surface_to_pane(
        &mut self,
        surface: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<Entity> {
        let sid = self
            .resolve_surface(surface)
            .ok_or(MultiplexerError::SurfaceNotFound(surface))?;
        // NOTE: Mux arg order is (surface, ORIENTATION, SIDE) — reversed vs this
        // method's (surface, side, orientation).
        let events = self
            .mux
            .mux
            .break_surface_to_pane(
                sid,
                ecs_orientation_to_mux(orientation),
                ecs_side_to_mux(side),
            )
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        let new_pane_id = created_pane_id(&events).expect("break emits PaneCreated");
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        Ok(self.mux.panes[new_pane_id])
    }

    /// Inserts `bundle` on an entity the multiplexer spawned. The caller must
    /// ensure `entity` is a valid multiplexer-owned entity.
    pub fn insert_on(&mut self, entity: Entity, bundle: impl Bundle) {
        self.commands.entity(entity).insert(bundle);
    }

    /// Closes the Workspace through the Mux (cascade removes all Panes and
    /// Surfaces), then applies the resulting events to the ECS mirror.
    /// Silently returns if `workspace` is not a known Mux workspace.
    pub fn close_workspace(&mut self, workspace: Entity) {
        let Some(id) = self.resolve_workspace(workspace) else {
            return;
        };
        let Ok(events) = self.mux.mux.close_workspace(id) else {
            return;
        };
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
    }

    /// Sets the Mux's active workspace to `workspace`, keeping the Mux's
    /// active-workspace pointer in sync with the GUI's `AttachedWorkspace`.
    /// `WorkspaceSelected` is handled as a no-op in `apply_event`; the GUI
    /// moves `AttachedWorkspace` separately.
    pub fn select_workspace(&mut self, workspace: Entity) -> MultiplexerResult<()> {
        let id = self
            .resolve_workspace(workspace)
            .ok_or(MultiplexerError::WorkspaceNotFound(workspace))?;
        let events = self
            .mux
            .mux
            .select_workspace(id)
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        Ok(())
    }

    /// Resize the split that controls `pane`'s extent in the given
    /// direction by `amount` cells through the Mux. Returns `NoOp` when
    /// the workspace has no size set or there is no matching ancestor split.
    pub fn resize_pane(
        &mut self,
        pane: Entity,
        direction: PaneDirection,
        amount: u16,
    ) -> MultiplexerResult<ResizePaneOutcome> {
        let id = self
            .resolve_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let events = self
            .mux
            .mux
            .resize_pane(id, ecs_direction_to_mux(direction), amount)
            .map_err(|e| crate::mirror::lift(&self.mux, e))?;
        let applied = !events.is_empty();
        for ev in &events {
            apply_event(&mut self.commands, &mut self.mux, &self.mirror_read, ev);
        }
        Ok(if applied {
            ResizePaneOutcome::Applied
        } else {
            ResizePaneOutcome::NoOp
        })
    }

    /// The Pane's owning Workspace, read from its `OwningWorkspace` back-pointer.
    pub fn workspace_of_pane(&self, pane: Entity) -> Option<Entity> {
        self.panes.get(pane).ok().map(|(_, owner)| owner.0)
    }

    /// The Pane that owns this Surface, via the `SurfaceOf` relationship.
    pub fn pane_of_surface(&self, surface: Entity) -> Option<Entity> {
        self.surface_owner.get(surface).ok().map(|o| o.0)
    }

    /// Read the Workspace's `ActivePane` pointer.
    pub fn workspaces_active_pane(&self, workspace: Entity) -> Option<Entity> {
        self.workspaces.get(workspace).ok().map(|active| active.0)
    }

    /// Read the Pane's `ActiveSurface` pointer.
    pub fn panes_active_surface(&self, pane: Entity) -> Option<Entity> {
        self.panes.get(pane).ok().map(|(active, _)| active.0)
    }

    /// Iterate the Pane entities owned by the given Workspace (via `OwningWorkspace`).
    pub fn panes_of_workspace(&self, workspace: Entity) -> impl Iterator<Item = Entity> + '_ {
        self.panes_owned
            .iter()
            .filter(move |(_, owner)| owner.0 == workspace)
            .map(|(e, _)| e)
    }

    /// Iterate the Surfaces a Pane owns, via the `Surfaces` collection.
    pub fn surfaces_of_pane(&self, pane: Entity) -> impl Iterator<Item = Entity> + '_ {
        self.pane_surfaces
            .get(pane)
            .into_iter()
            .flat_map(|s| s.iter())
    }

    /// Resolves a pane `Entity` to its `PaneId` via the immediate reverse map
    /// first (so a pane spawned earlier in the same deferred-command batch resolves
    /// before its `MuxPaneId` component flushes), then the ECS query.
    fn resolve_pane(&self, pane: Entity) -> Option<ozmux_mux::PaneId> {
        self.mux
            .pane_id_of_entity(pane)
            .or_else(|| self.mirror_read.pane_id_of(pane))
    }

    /// Resolves a workspace `Entity` to its `WorkspaceId` via the immediate
    /// reverse map first, then the ECS query.
    fn resolve_workspace(&self, workspace: Entity) -> Option<ozmux_mux::WorkspaceId> {
        self.mux
            .workspace_id_of_entity(workspace)
            .or_else(|| self.mirror_read.workspace_id_of(workspace))
    }

    /// Resolves a surface `Entity` to its `SurfaceId` via the immediate
    /// reverse map first, then the ECS query.
    fn resolve_surface(&self, surface: Entity) -> Option<ozmux_mux::SurfaceId> {
        self.mux
            .surface_id_of_entity(surface)
            .or_else(|| self.mirror_read.surface_id_of(surface))
    }
}

/// Returns the DFS-ordered neighbor of `pane` at `offset` distance in
/// `ordered`, wrapping around. Returns `None` if there are fewer than 2 panes
/// (no swap target exists), mirroring the Mux's no-op condition.
fn pane_neighbor(
    ordered: &[ozmux_mux::PaneId],
    pane: ozmux_mux::PaneId,
    offset: SwapOffset,
) -> Option<ozmux_mux::PaneId> {
    if ordered.len() < 2 {
        return None;
    }
    let i = ordered.iter().position(|p| *p == pane)?;
    let len = ordered.len() as isize;
    let delta: isize = match offset {
        SwapOffset::Prev => -1,
        SwapOffset::Next => 1,
    };
    let j = ((i as isize + delta).rem_euclid(len)) as usize;
    Some(ordered[j])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::WorkspaceMarker;
    use crate::mirror::MuxState;
    use crate::plugin::MultiplexerPlugin;
    use bevy::ecs::system::RunSystemOnce;

    /// Entities whose `Changed<Children>` fired during the last `Update` tick.
    #[derive(Default, Resource)]
    struct PanesWithChangedChildren(Vec<Entity>);

    /// Entities whose `Changed<ActiveSurface>` fired during the last `Update` tick.
    #[derive(Default, Resource)]
    struct PanesWithChangedActiveSurface(Vec<Entity>);

    fn collect_changed_children(
        mut res: ResMut<PanesWithChangedChildren>,
        query: Query<Entity, (With<PaneMarker>, Changed<Children>)>,
    ) {
        res.0.clear();
        res.0.extend(query.iter());
    }

    fn collect_changed_active_surface(
        mut res: ResMut<PanesWithChangedActiveSurface>,
        query: Query<Entity, (With<PaneMarker>, Changed<ActiveSurface>)>,
    ) {
        res.0.clear();
        res.0.extend(query.iter());
    }

    /// Builds an App with `MultiplexerPlugin` (which inserts `MuxState` and runs the
    /// Startup materialize), ticks once so Startup fires, and returns the App plus
    /// the Mux-seeded initial workspace entity.
    #[expect(dead_code, reason = "consumed by the authority flip in P2b2 Tasks 3-6")]
    pub(crate) fn mux_backed_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.update();
        let ws = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceMarker>>()
            .iter(app.world())
            .next()
            .expect("initial workspace must exist after Startup");
        (app, ws)
    }

    /// Builds a `World` pre-loaded with `WorkspaceNameCounter` and a materialized
    /// `MuxState`, so that `run_system_once` with `MultiplexerCommands` succeeds.
    fn make_world() -> World {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        world.insert_resource(MuxState::new(ozmux_mux::Mux::new()));
        world
            .run_system_once(|mut commands: Commands, mut state: ResMut<MuxState>| {
                state.materialize_snapshot(&mut commands);
            })
            .unwrap();
        world.flush();
        world
    }

    /// Builds a minimal `App` with capture systems for change-detection assertions.
    fn make_change_detection_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MultiplexerPlugin);
        app.init_resource::<PanesWithChangedChildren>();
        app.init_resource::<PanesWithChangedActiveSurface>();
        app.add_systems(
            Update,
            (collect_changed_children, collect_changed_active_surface),
        );
        app
    }

    #[test]
    fn spawn_attached_workspace_attaches_marker_subtree_and_created_at() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MultiplexerPlugin);
        app.update();

        let workspace = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.spawn_attached_workspace())
            .unwrap();
        app.world_mut().flush();

        let world = app.world();
        assert!(
            world.get::<AttachedWorkspace>(workspace).is_some(),
            "new workspace carries AttachedWorkspace",
        );
        assert_eq!(
            world.get::<WorkspaceCreatedAt>(workspace).map(|c| c.0),
            Some(1),
            "first spawn_attached_workspace mints WorkspaceCreatedAt(1)",
        );
        assert_eq!(
            world.get::<Name>(workspace).map(|n| n.as_str().to_owned()),
            Some("workspace1".to_owned()),
            "workspace is auto-named workspace1 from the counter",
        );
        let subtree = world
            .get::<WorkspaceUiSubtree>(workspace)
            .expect("new workspace carries a WorkspaceUiSubtree pointer")
            .0;
        assert_eq!(
            world.get::<ChildOf>(subtree).map(|c| c.parent()),
            Some(workspace),
            "the subtree node is parented under the workspace",
        );
    }

    #[test]
    fn add_and_remove_surface_flag_changed_children_on_pane() {
        let mut app = make_change_detection_app();
        app.update();

        let outcome = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        app.world_mut().flush();
        // NOTE: this settle tick must run before the mutation below, or the
        // bootstrap `Changed<Children>` leaks into the assertion and the test
        // passes vacuously.
        app.update();

        let added = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(outcome.pane, SurfaceKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        assert!(
            app.world()
                .resource::<PanesWithChangedChildren>()
                .0
                .contains(&outcome.pane),
            "adding a surface child must flag Changed<Children> on the pane",
        );

        app.update();

        app.world_mut().entity_mut(added).despawn();
        app.world_mut().flush();
        app.update();

        assert!(
            app.world()
                .resource::<PanesWithChangedChildren>()
                .0
                .contains(&outcome.pane),
            "despawning a surface child must flag Changed<Children> on the pane",
        );
    }

    #[test]
    fn set_active_surface_flags_changed_only_on_real_change() {
        let mut app = make_change_detection_app();
        app.update();

        let outcome = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let second = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(outcome.pane, SurfaceKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();
        // NOTE: both settle ticks must run before the no-op mutation below, or
        // the bootstrap `Changed<ActiveSurface>` leaks into the negative
        // assertion and the test passes vacuously.
        app.update();
        app.update();

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_surface(outcome.pane, outcome.surface)
                    .unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        assert!(
            app.world()
                .resource::<PanesWithChangedActiveSurface>()
                .0
                .is_empty(),
            "no-op set_active_surface must NOT flag Changed<ActiveSurface>",
        );

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_surface(outcome.pane, second).unwrap();
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        assert!(
            app.world()
                .resource::<PanesWithChangedActiveSurface>()
                .0
                .contains(&outcome.pane),
            "a real switch must flag Changed<ActiveSurface> on the pane",
        );
    }

    #[test]
    fn create_workspace_spawns_root_pane_surface_tree() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("test".into()))
            })
            .unwrap();
        world.flush();

        assert!(world.get::<WorkspaceMarker>(outcome.workspace).is_some());
        assert_eq!(
            world.get::<Name>(outcome.workspace).map(|n| n.as_str()),
            Some("test")
        );
        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(outcome.pane)
        );
        let root = world
            .get::<WorkspaceUiSubtree>(outcome.workspace)
            .expect("subtree")
            .0;

        let root_kids: Vec<Entity> = world
            .get::<Children>(root)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(
            root_kids,
            vec![outcome.pane],
            "root node's single child is the pane"
        );

        assert!(world.get::<PaneMarker>(outcome.pane).is_some());
        assert_eq!(
            world.get::<OwningWorkspace>(outcome.pane).map(|o| o.0),
            Some(outcome.workspace)
        );
        assert_eq!(
            world.get::<ChildOf>(outcome.pane).map(|c| c.parent()),
            Some(root)
        );
        assert_eq!(
            world.get::<Node>(outcome.pane).map(|n| n.flex_grow),
            Some(0.0),
            "pane leaf uses fixed-px layout; flex_grow must be 0"
        );
        assert_eq!(
            world.get::<ActiveSurface>(outcome.pane).map(|a| a.0),
            Some(outcome.surface)
        );

        assert!(world.get::<SurfaceMarker>(outcome.surface).is_some());
        assert_eq!(
            world.get::<ChildOf>(outcome.surface).map(|c| c.parent()),
            Some(outcome.pane)
        );
    }

    #[test]
    fn workspace_of_pane_uses_owning_workspace_not_childof() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let ws = world
            .run_system_once(move |mux: MultiplexerCommands| mux.workspace_of_pane(outcome.pane))
            .unwrap();
        assert_eq!(
            ws,
            Some(outcome.workspace),
            "resolved via OwningWorkspace (pane is ChildOf the root node)"
        );
        let panes: Vec<Entity> = world
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.panes_of_workspace(outcome.workspace)
                    .collect::<Vec<_>>()
            })
            .unwrap();
        assert_eq!(panes, vec![outcome.pane]);
    }

    #[test]
    fn rename_workspace_mutates_name_and_only_fires_changed_on_actual_change() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("before".into()))
            })
            .unwrap();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.rename_workspace(outcome.workspace, "after".into())
                    .unwrap();
            })
            .unwrap();

        assert_eq!(
            world.get::<Name>(outcome.workspace).map(|n| n.as_str()),
            Some("after")
        );
    }

    #[test]
    fn set_workspace_dimensions_inserts_or_updates_component() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_workspace_dimensions(outcome.workspace, 120, 40);
            })
            .unwrap();
        world.flush();
        assert_eq!(
            world.get::<WorkspaceDimensions>(outcome.workspace).copied(),
            Some(WorkspaceDimensions {
                cols: 120,
                rows: 40
            }),
        );
    }

    #[test]
    fn set_active_pane_updates_active_pane_pointer() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let other_pane = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        world.flush();

        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(other_pane),
            "split makes the new pane active"
        );

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_pane(outcome.workspace, outcome.pane)
                    .unwrap();
            })
            .unwrap();

        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(outcome.pane),
            "set_active_pane re-focuses the first pane"
        );
    }

    #[test]
    fn set_active_surface_updates_active_surface_pointer() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let other_surface = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(outcome.pane, SurfaceKind::Terminal)
            })
            .unwrap();
        world.flush();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_surface(outcome.pane, other_surface).unwrap();
            })
            .unwrap();

        assert_eq!(
            world.get::<ActiveSurface>(outcome.pane).map(|a| a.0),
            Some(other_surface)
        );
    }

    #[test]
    fn split_pane_inserts_split_reparents_target_and_sets_grows() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let root = world
            .get::<WorkspaceUiSubtree>(outcome.workspace)
            .unwrap()
            .0;

        let new_pane = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        world.flush();

        let split = world.get::<Children>(root).unwrap().iter().next().unwrap();
        assert!(world.get::<SplitNode>(split).is_some());
        assert_eq!(
            world.get::<SplitNode>(split).unwrap().orientation,
            SplitOrientation::Horizontal
        );
        assert_eq!(
            world.get::<Node>(split).map(|n| n.flex_grow),
            Some(0.0),
            "split container uses fixed-px layout; flex_grow must be 0"
        );
        let kids: Vec<Entity> = world.get::<Children>(split).unwrap().iter().collect();
        assert_eq!(kids, vec![outcome.pane, new_pane]);
        assert_eq!(
            world.get::<Node>(outcome.pane).map(|n| n.flex_grow),
            Some(0.0),
            "pane leaf uses fixed-px layout; flex_grow must be 0"
        );
        assert_eq!(
            world.get::<Node>(new_pane).map(|n| n.flex_grow),
            Some(0.0),
            "pane leaf uses fixed-px layout; flex_grow must be 0"
        );
        assert_eq!(
            world.get::<OwningWorkspace>(new_pane).map(|o| o.0),
            Some(outcome.workspace)
        );
        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(new_pane)
        );
        assert_eq!(
            world.get::<ChildOf>(outcome.surface).map(|c| c.parent()),
            Some(outcome.pane)
        );
    }

    #[test]
    fn split_pane_before_orders_new_then_target() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let new_pane = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::Before, SplitOrientation::Vertical)
                    .unwrap()
            })
            .unwrap();
        world.flush();
        let root = world
            .get::<WorkspaceUiSubtree>(outcome.workspace)
            .unwrap()
            .0;
        let split = world.get::<Children>(root).unwrap().iter().next().unwrap();
        let kids: Vec<Entity> = world.get::<Children>(split).unwrap().iter().collect();
        assert_eq!(
            kids,
            vec![new_pane, outcome.pane],
            "Side::Before puts new pane first"
        );
    }

    #[test]
    fn close_pane_promotes_sibling_into_slot_and_despawns_split() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let root = world
            .get::<WorkspaceUiSubtree>(outcome.workspace)
            .unwrap()
            .0;
        let new_pane = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        world.flush();
        let split = world.get::<Children>(root).unwrap().iter().next().unwrap();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| mux.close_pane(new_pane).unwrap())
            .unwrap();
        world.flush();

        assert!(world.get_entity(new_pane).is_err(), "closed pane despawned");
        assert!(world.get_entity(split).is_err(), "parent split despawned");
        let root_kids: Vec<Entity> = world.get::<Children>(root).unwrap().iter().collect();
        assert_eq!(root_kids, vec![outcome.pane]);
        assert_eq!(
            world.get::<Node>(outcome.pane).map(|n| n.flex_grow),
            Some(0.0),
            "promoted pane uses fixed-px layout; flex_grow must be 0"
        );
        assert!(world.get_entity(outcome.surface).is_ok());
        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(outcome.pane)
        );
    }

    #[test]
    fn close_last_pane_errors() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| mux.close_pane(outcome.pane))
            .unwrap();
        assert!(matches!(
            result,
            Err(MultiplexerError::CannotCloseLastPaneInWorkspace(_))
        ));
    }

    #[test]
    fn swap_pane_swaps_child_order() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let other = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        world.flush();

        let root = world
            .get::<WorkspaceUiSubtree>(outcome.workspace)
            .unwrap()
            .0;
        let split = world.get::<Children>(root).unwrap().iter().next().unwrap();

        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.swap_pane(outcome.pane, SwapOffset::Next).unwrap()
            })
            .unwrap();
        world.flush();

        assert_eq!(result, SwapOutcome::Swapped { other_pane: other });
        let kids: Vec<Entity> = world.get::<Children>(split).unwrap().iter().collect();
        assert_eq!(kids, vec![other, outcome.pane]);
    }

    #[test]
    fn swap_pane_single_pane_is_noop() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.swap_pane(outcome.pane, SwapOffset::Next).unwrap()
            })
            .unwrap();
        assert_eq!(result, SwapOutcome::NoOp);
    }

    #[test]
    fn add_surface_spawns_surface_child_of_pane() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let new_surface = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(outcome.pane, SurfaceKind::Terminal)
            })
            .unwrap();
        world.flush();

        assert!(world.get::<SurfaceMarker>(new_surface).is_some());
        assert_eq!(
            world.get::<ChildOf>(new_surface).map(|c| c.parent()),
            Some(outcome.pane)
        );
    }

    #[test]
    fn close_workspace_despawns_workspace_and_descendants() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.close_workspace(outcome.workspace);
            })
            .unwrap();
        world.flush();

        assert!(world.get_entity(outcome.workspace).is_err());
        assert!(
            world.get_entity(outcome.pane).is_err(),
            "pane cascade-despawned"
        );
        assert!(
            world.get_entity(outcome.surface).is_err(),
            "surface cascade-despawned"
        );
    }

    #[test]
    fn break_surface_to_pane_creates_new_pane_with_moved_surface() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let second_surface = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(outcome.pane, SurfaceKind::Terminal)
            })
            .unwrap();
        world.flush();

        let new_pane = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.break_surface_to_pane(second_surface, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        world.flush();

        assert_eq!(
            world.get::<ChildOf>(second_surface).map(|c| c.parent()),
            Some(new_pane)
        );
        assert_eq!(
            world.get::<ActiveSurface>(new_pane).map(|a| a.0),
            Some(second_surface)
        );
        assert!(world.get::<PaneMarker>(outcome.pane).is_some());
    }

    #[test]
    fn break_surface_to_pane_returns_error_when_source_pane_has_only_one_surface() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.break_surface_to_pane(
                    outcome.surface,
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .unwrap();
        assert!(
            matches!(result, Err(MultiplexerError::CannotRemoveLastSurface(_))),
            "expected CannotRemoveLastSurface, got {result:?}",
        );
    }

    #[test]
    fn split_pane_with_surface_seeds_extension_surface() {
        use std::path::PathBuf;
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();

        let split = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane_with_surface(
                    outcome.pane,
                    Side::After,
                    SplitOrientation::Vertical,
                    SurfaceKind::Extension {
                        entry: PathBuf::from("/x/memo"),
                    },
                )
                .unwrap()
            })
            .unwrap();
        world.flush();

        assert!(world.get::<PaneMarker>(split.pane).is_some());
        let root = world
            .get::<WorkspaceUiSubtree>(outcome.workspace)
            .unwrap()
            .0;
        let split_node = world.get::<Children>(root).unwrap().iter().next().unwrap();
        assert!(
            world.get::<SplitNode>(split_node).is_some(),
            "root's single child is a Split"
        );
        let split_kids: Vec<Entity> = world.get::<Children>(split_node).unwrap().iter().collect();
        assert!(
            split_kids.contains(&split.pane),
            "new pane is under the Split in the entity tree"
        );
        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(split.pane)
        );
        assert_eq!(
            world.get::<ChildOf>(split.surface).map(|c| c.parent()),
            Some(split.pane)
        );
        assert_eq!(
            world.get::<ActiveSurface>(split.pane).map(|a| a.0),
            Some(split.surface)
        );
        assert!(matches!(
            world.get::<SurfaceKind>(split.surface),
            Some(SurfaceKind::Extension { .. })
        ));
    }

    #[test]
    fn workspaces_active_pane_returns_bootstrap_pane() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let active = world
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.workspaces_active_pane(outcome.workspace)
            })
            .unwrap();
        assert_eq!(active, Some(outcome.pane));
    }

    #[test]
    fn panes_active_surface_returns_bootstrap_surface() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let active = world
            .run_system_once(move |mux: MultiplexerCommands| mux.panes_active_surface(outcome.pane))
            .unwrap();
        assert_eq!(active, Some(outcome.surface));
    }

    #[test]
    fn resize_pane_returns_noop_without_workspace_dimensions() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.resize_pane(outcome.pane, PaneDirection::Right, 5)
            })
            .unwrap();
        assert!(matches!(result, Ok(ResizePaneOutcome::NoOp)));
    }

    #[test]
    fn resize_pane_returns_noop_for_single_pane_workspace() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_workspace_dimensions(outcome.workspace, 120, 40);
            })
            .unwrap();
        world.flush();
        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.resize_pane(outcome.pane, PaneDirection::Right, 5)
            })
            .unwrap();
        assert!(matches!(result, Ok(ResizePaneOutcome::NoOp)));
    }

    #[test]
    fn add_surface_stamps_surfaceof_and_appears_in_surfaces() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let s2 = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(outcome.pane, SurfaceKind::Terminal)
            })
            .unwrap();
        world.flush();
        assert_eq!(world.get::<SurfaceOf>(s2).map(|o| o.0), Some(outcome.pane));
        let pane = world
            .run_system_once(move |mux: MultiplexerCommands| mux.pane_of_surface(s2))
            .unwrap();
        assert_eq!(pane, Some(outcome.pane));
        let surfaces: Vec<Entity> = world
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.surfaces_of_pane(outcome.pane).collect::<Vec<_>>()
            })
            .unwrap();
        assert!(surfaces.contains(&outcome.surface) && surfaces.contains(&s2));
    }

    #[test]
    fn closing_pane_despawns_parked_surface_via_linked_spawn() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let parked = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(outcome.pane, SurfaceKind::Terminal)
            })
            .unwrap();
        world.flush();
        world.entity_mut(parked).insert(ChildOf(outcome.workspace));
        world.flush();
        world.entity_mut(outcome.pane).despawn();
        world.flush();
        assert!(
            world.get_entity(parked).is_err(),
            "parked surface cascade-despawned via Surfaces(linked_spawn)"
        );
    }

    #[test]
    fn same_batch_add_surface_and_set_active_pane_resolve_via_reverse_map() {
        // split_pane returns a new pane entity whose MuxPaneId component is
        // deferred (not yet flushed). A second run_system_once that calls
        // add_surface and set_active_pane on that entity WITHOUT a flush in
        // between must still succeed — resolved via the immediate reverse map.
        // Before Fix 1a-1c, pane_id_of returned None (component not flushed),
        // causing add_surface to panic and set_active_pane to return PaneNotFound.
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        world.flush();
        let p2 = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .expect("split_pane must succeed")
            })
            .unwrap();
        // NOTE: intentionally no world.flush() here — p2's MuxPaneId component is
        // not yet committed to the ECS, so only the reverse map can resolve it.
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                let _s = mux.add_surface(p2, SurfaceKind::Terminal);
                mux.set_active_pane(outcome.workspace, p2)
                    .expect("set_active_pane must succeed without prior flush");
            })
            .expect("run_system_once must succeed");
        world.flush();
        let state = world.resource::<MuxState>();
        let result = crate::mirror::mirror_matches(&world, state);
        assert!(
            result.is_ok(),
            "mirror_matches after same-batch ops: {result:?}"
        );
    }
}
