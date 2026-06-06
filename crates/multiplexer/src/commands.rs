//! `MultiplexerCommands` SystemParam — the sole mutation API for the
//! multiplexer. Each method performs whatever entity spawns/despawns and
//! component mutations are needed for one logical operation; Bevy's
//! native change detection (`Changed<T>`) carries the signal to downstream
//! rebuild systems.

use crate::components::{
    ActivePane, ActiveSurface, AttachedWorkspace, CopyMode, OwningWorkspace, PaneMarker, SplitNode,
    SurfaceKind, SurfaceMarker, SurfaceOf, Surfaces, WorkspaceCreatedAt, WorkspaceDimensions,
    WorkspaceMarker, WorkspaceUiSubtree,
};
use crate::direction::PaneDirection;
use crate::error::{MultiplexerError, MultiplexerResult};
use crate::layout::{LayoutTree, Side, SplitOrientation};
use crate::mirror::{MirrorReadCtx, MuxState, apply_event, created_workspace_id};
use crate::resize::{ResizePaneOutcome, resize_split_for_pane};
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
    workspaces: Query<
        'w,
        's,
        (
            &'static mut ActivePane,
            &'static mut Name,
            Option<&'static mut WorkspaceDimensions>,
        ),
        With<WorkspaceMarker>,
    >,
    panes: Query<
        'w,
        's,
        (
            &'static mut ActiveSurface,
            &'static mut CopyMode,
            &'static ChildOf,
            &'static OwningWorkspace,
        ),
        With<PaneMarker>,
    >,
    panes_owned: Query<'w, 's, (Entity, &'static OwningWorkspace), With<PaneMarker>>,
    surface_owner: Query<'w, 's, &'static SurfaceOf, With<SurfaceMarker>>,
    pane_surfaces: Query<'w, 's, &'static Surfaces, With<PaneMarker>>,
    children: Query<'w, 's, &'static Children>,
    child_of: Query<'w, 's, &'static ChildOf>,
    layout_nodes: Query<'w, 's, &'static Node>,
    splits: Query<'w, 's, &'static SplitNode>,
    panes_only: Query<'w, 's, (), With<PaneMarker>>,
    subtrees: Query<'w, 's, &'static WorkspaceUiSubtree>,
    tree: LayoutTree<'w, 's>,
}

impl<'w, 's> MultiplexerCommands<'w, 's> {
    /// Spawn a Workspace with a layout-root node holding one bootstrap Pane
    /// (one bootstrap Terminal Surface as its child).
    pub fn create_workspace(&mut self, name: Option<String>) -> WorkspaceCreated {
        let name = name.unwrap_or_else(|| "default".to_string());

        let workspace = self
            .commands
            .spawn((WorkspaceMarker, Name::new(name.clone())))
            .id();

        let root = self
            .commands
            .spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                Name::new(format!("layout-root: {name}")),
            ))
            .id();

        let surface = self
            .commands
            .spawn((
                SurfaceMarker,
                SurfaceKind::Terminal,
                Name::new(format!("surface: {name}#0")),
            ))
            .id();

        let mut pane_node = crate::layout::pane_frame_node();
        let cf = crate::layout::child_flex(1.0);
        pane_node.flex_grow = cf.flex_grow;
        pane_node.flex_basis = cf.flex_basis;
        let pane = self
            .commands
            .spawn((
                PaneMarker,
                OwningWorkspace(workspace),
                ActiveSurface(surface),
                CopyMode::default(),
                pane_node,
                Name::new(format!("pane: {name}#0")),
            ))
            .id();

        self.commands
            .entity(workspace)
            .insert((ActivePane(pane), WorkspaceUiSubtree(root)));
        self.commands.entity(root).insert(ChildOf(workspace));
        self.commands.entity(pane).insert(ChildOf(root));
        self.commands
            .entity(surface)
            .insert((ChildOf(pane), SurfaceOf(pane)));

        WorkspaceCreated {
            workspace,
            pane,
            surface,
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

    /// Mutate the Workspace's `Name` component. Uses `set_if_neq` so a
    /// no-op rename does not flag `Changed<Name>`.
    pub fn rename_workspace(&mut self, workspace: Entity, name: String) -> MultiplexerResult<()> {
        let (_, mut current_name, _) = self
            .workspaces
            .get_mut(workspace)
            .map_err(|_| MultiplexerError::WorkspaceNotFound(workspace))?;
        current_name.set_if_neq(Name::new(name));
        Ok(())
    }

    /// Set the Workspace's cached dimensions. Inserts the component on
    /// first call; subsequent calls update in place via `set_if_neq`.
    pub fn set_workspace_dimensions(&mut self, workspace: Entity, cols: u16, rows: u16) {
        let new = WorkspaceDimensions { cols, rows };
        if let Ok((_, _, dims)) = self.workspaces.get_mut(workspace)
            && let Some(mut dims) = dims
        {
            dims.set_if_neq(new);
            return;
        }
        self.commands.entity(workspace).insert(new);
    }

    /// Update the Workspace's `ActivePane` pointer to `pane`. The pane MUST
    /// belong to the workspace (caller's invariant; not validated here).
    pub fn set_active_pane(&mut self, workspace: Entity, pane: Entity) -> MultiplexerResult<()> {
        let (mut active_pane, _, _) = self
            .workspaces
            .get_mut(workspace)
            .map_err(|_| MultiplexerError::WorkspaceNotFound(workspace))?;
        active_pane.set_if_neq(ActivePane(pane));
        Ok(())
    }

    /// Update the Pane's `ActiveSurface` pointer.
    pub fn set_active_surface(&mut self, pane: Entity, surface: Entity) -> MultiplexerResult<()> {
        let (mut active_surface, _, _, _) = self
            .panes
            .get_mut(pane)
            .map_err(|_| MultiplexerError::PaneNotFound(pane))?;
        active_surface.set_if_neq(ActiveSurface(surface));
        Ok(())
    }

    /// Split the target pane and seed the new pane with one surface of the
    /// caller-chosen `kind`. Delegates to `split_pane_inner` (which inserts a
    /// `Split` node into the target's layout slot, reparents the target and the
    /// new pane under it, and promotes the new pane to `ActivePane`) and
    /// attaches the surface; on error the freshly-spawned surface is despawned
    /// to leave no orphan.
    pub fn split_pane_with_surface(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
        kind: SurfaceKind,
    ) -> MultiplexerResult<SplitOutcome> {
        let surface = self
            .commands
            .spawn((SurfaceMarker, kind, Name::new("surface: split")))
            .id();
        match self.split_pane_inner(target_pane, side, orientation) {
            Ok((new_pane, _)) => {
                self.commands
                    .entity(new_pane)
                    .insert(ActiveSurface(surface));
                self.commands
                    .entity(surface)
                    .insert((ChildOf(new_pane), SurfaceOf(new_pane)));
                Ok(SplitOutcome {
                    pane: new_pane,
                    surface,
                })
            }
            Err(e) => {
                self.commands.entity(surface).despawn();
                Err(e)
            }
        }
    }

    /// Split the target pane in two, seeding a bootstrap Terminal surface.
    /// Inserts a `Split` node into the target's layout slot, reparents the
    /// target and the new pane under it, and promotes the new pane to
    /// `ActivePane`. On error, freshly-spawned entities are despawned to leave
    /// no orphans.
    pub fn split_pane(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<Entity> {
        self.split_pane_with_surface(target_pane, side, orientation, SurfaceKind::Terminal)
            .map(|o| o.pane)
    }

    /// Close a pane. Promotes the closed pane's sibling into the grandparent
    /// slot, despawns the pane and its parent `Split`, and repoints `ActivePane`
    /// to the survivor if the closed pane was active. Surface children cascade-
    /// despawn via `ChildOf`.
    pub fn close_pane(&mut self, pane: Entity) -> MultiplexerResult<()> {
        let workspace = self
            .workspace_of_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let result = crate::layout::close_in_tree(
            &mut self.commands,
            workspace,
            pane,
            &self.child_of,
            &self.children,
            &self.layout_nodes,
            &self.splits,
            &self.panes_only,
        )?;
        if let Ok((mut active_pane, _, _)) = self.workspaces.get_mut(workspace)
            && active_pane.0 == pane
        {
            active_pane.0 = result.survivor_pane;
        }
        Ok(())
    }

    /// Swap a pane's contents with its prev/next neighbor in the layout's
    /// DFS leaf traversal. No-op for single-pane workspaces.
    pub fn swap_pane(
        &mut self,
        pane: Entity,
        offset: SwapOffset,
    ) -> MultiplexerResult<SwapOutcome> {
        let workspace = self
            .workspace_of_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let root = self
            .subtrees
            .get(workspace)
            .map(|s| s.0)
            .map_err(|_| MultiplexerError::WorkspaceNotFound(workspace))?;
        let ordered = crate::layout::ordered_panes(root, &self.children, &self.panes_only);
        if ordered.len() < 2 {
            return Ok(SwapOutcome::NoOp);
        }
        let i = ordered
            .iter()
            .position(|p| *p == pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let len = ordered.len() as isize;
        let delta = match offset {
            SwapOffset::Prev => -1,
            SwapOffset::Next => 1,
        };
        let j = ((i as isize + delta).rem_euclid(len)) as usize;
        let other_pane = ordered[j];
        crate::layout::swap_in_tree(
            &mut self.commands,
            pane,
            other_pane,
            &self.child_of,
            &self.children,
            &self.layout_nodes,
        );
        Ok(SwapOutcome::Swapped { other_pane })
    }

    /// Spawn a new Surface as a child of `pane`. Does NOT change
    /// `ActiveSurface` — call `set_active_surface` separately if needed.
    pub fn add_surface(&mut self, pane: Entity, kind: SurfaceKind) -> Entity {
        let surface = self
            .commands
            .spawn((SurfaceMarker, kind, Name::new("surface")))
            .id();
        self.commands
            .entity(surface)
            .insert((ChildOf(pane), SurfaceOf(pane)));
        surface
    }

    /// Split the surface's owning Pane and move the surface into the
    /// freshly-created Pane (where it becomes the only surface). The new
    /// Pane becomes the workspace's `ActivePane`. Caller must ensure the
    /// source Pane has at least 2 surfaces, else this returns
    /// `CannotRemoveLastSurface`.
    pub fn break_surface_to_pane(
        &mut self,
        surface: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<Entity> {
        let source_pane = self
            .pane_of_surface(surface)
            .ok_or(MultiplexerError::SurfaceNotFound(surface))?;

        let surface_count = self.surfaces_of_pane(source_pane).count();
        if surface_count < 2 {
            return Err(MultiplexerError::CannotRemoveLastSurface(source_pane));
        }

        // NOTE: split_pane_inner avoids spawning a bootstrap surface; otherwise
        //       the deferred `ChildOf` insertion would race with the immediate
        //       reparent below, leaving the bootstrap entity orphaned.
        let (new_pane, _) = self.split_pane_inner(source_pane, side, orientation)?;

        self.commands
            .entity(surface)
            .insert((ChildOf(new_pane), SurfaceOf(new_pane)));
        self.commands
            .entity(new_pane)
            .insert(ActiveSurface(surface));

        Ok(new_pane)
    }

    /// Inserts `bundle` on an entity the multiplexer spawned. The caller must
    /// ensure `entity` is a valid multiplexer-owned entity.
    pub fn insert_on(&mut self, entity: Entity, bundle: impl Bundle) {
        self.commands.entity(entity).insert(bundle);
    }

    /// Close a Workspace entirely. Cascading `ChildOf` despawn removes all
    /// Pane and Surface descendants.
    pub fn close_workspace(&mut self, workspace: Entity) {
        self.commands.entity(workspace).despawn();
    }

    /// Resize the split that controls `pane`'s extent in the given
    /// direction by `amount` cells. See `resize::resize_split_for_pane`
    /// for the underlying weight-based algorithm. Requires that
    /// `WorkspaceDimensions` has been set; returns `NoOp` if not.
    pub fn resize_pane(
        &mut self,
        pane: Entity,
        direction: PaneDirection,
        amount: u16,
    ) -> MultiplexerResult<ResizePaneOutcome> {
        let workspace = self
            .workspace_of_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let (cols, rows) = self
            .workspaces
            .get(workspace)
            .ok()
            .and_then(|(_, _, dims)| dims.map(|d| (d.cols, d.rows)))
            .unwrap_or((0, 0));
        if cols == 0 || rows == 0 {
            return Ok(ResizePaneOutcome::NoOp);
        }
        Ok(resize_split_for_pane(
            &mut self.commands,
            &self.tree,
            pane,
            direction,
            amount,
            cols,
            rows,
        ))
    }

    /// The Pane's owning Workspace, read from its `OwningWorkspace` back-pointer.
    pub fn workspace_of_pane(&self, pane: Entity) -> Option<Entity> {
        self.panes.get(pane).ok().map(|(_, _, _, owner)| owner.0)
    }

    /// The Pane that owns this Surface, via the `SurfaceOf` relationship.
    pub fn pane_of_surface(&self, surface: Entity) -> Option<Entity> {
        self.surface_owner.get(surface).ok().map(|o| o.0)
    }

    /// Read the Workspace's `ActivePane` pointer.
    pub fn workspaces_active_pane(&self, workspace: Entity) -> Option<Entity> {
        self.workspaces
            .get(workspace)
            .ok()
            .map(|(active, _, _)| active.0)
    }

    /// Read the Pane's `ActiveSurface` pointer.
    pub fn panes_active_surface(&self, pane: Entity) -> Option<Entity> {
        self.panes.get(pane).ok().map(|(active, _, _, _)| active.0)
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

    /// Inserts a `Split` node into the target's layout slot, reparents the
    /// target and `new_pane` under it, and promotes `new_pane` to `ActivePane`.
    /// Does not spawn a bootstrap surface; callers are responsible for attaching
    /// one. Returns `(new_pane, workspace)`.
    fn split_pane_inner(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<(Entity, Entity)> {
        let workspace = self
            .workspace_of_pane(target_pane)
            .ok_or(MultiplexerError::PaneNotFound(target_pane))?;

        let pane_node = crate::layout::pane_frame_node();
        let new_pane = self
            .commands
            .spawn((
                PaneMarker,
                OwningWorkspace(workspace),
                CopyMode::default(),
                pane_node,
                Name::new("pane: split"),
            ))
            .id();

        crate::layout::split_in_tree(
            &mut self.commands,
            target_pane,
            new_pane,
            side,
            orientation,
            &self.child_of,
            &self.children,
            &self.layout_nodes,
        );

        if let Ok((mut active_pane, _, _)) = self.workspaces.get_mut(workspace) {
            active_pane.0 = new_pane;
        }
        Ok((new_pane, workspace))
    }
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
    #[allow(dead_code)]
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
            Some(1.0)
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
        let other_pane = world
            .spawn((
                PaneMarker,
                ActiveSurface(outcome.surface),
                CopyMode::default(),
                Name::new("other"),
                ChildOf(outcome.workspace),
            ))
            .id();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_pane(outcome.workspace, other_pane).unwrap();
            })
            .unwrap();

        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(other_pane)
        );
    }

    #[test]
    fn set_active_surface_updates_active_surface_pointer() {
        let mut world = make_world();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let other_surface = world
            .spawn((
                SurfaceMarker,
                SurfaceKind::Terminal,
                Name::new("other"),
                ChildOf(outcome.pane),
            ))
            .id();

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
        assert_eq!(world.get::<Node>(split).map(|n| n.flex_grow), Some(1.0));
        let kids: Vec<Entity> = world.get::<Children>(split).unwrap().iter().collect();
        assert_eq!(kids, vec![outcome.pane, new_pane]);
        assert_eq!(
            world.get::<Node>(outcome.pane).map(|n| n.flex_grow),
            Some(1.0)
        );
        assert_eq!(world.get::<Node>(new_pane).map(|n| n.flex_grow), Some(1.0));
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
            Some(1.0)
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
    fn swap_pane_swaps_positions_and_slot_grows() {
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
        // Slot A (index 0) holds outcome.pane; slot B (index 1) holds `other`.
        // Give the two slots DISTINCT grows so slot-pinning is observable.
        world.get_mut::<Node>(outcome.pane).unwrap().flex_grow = 3.0;
        world.get_mut::<Node>(other).unwrap().flex_grow = 1.0;

        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.swap_pane(outcome.pane, SwapOffset::Next).unwrap()
            })
            .unwrap();
        world.flush();

        assert_eq!(result, SwapOutcome::Swapped { other_pane: other });
        let kids: Vec<Entity> = world.get::<Children>(split).unwrap().iter().collect();
        assert_eq!(kids, vec![other, outcome.pane]);
        assert_eq!(
            world.get::<Node>(other).unwrap().flex_grow,
            3.0,
            "slot A keeps its grow; pane moved in"
        );
        assert_eq!(
            world.get::<Node>(outcome.pane).unwrap().flex_grow,
            1.0,
            "slot B keeps its grow"
        );
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
}
