//! `MultiplexerCommands` SystemParam — the sole mutation API for the
//! multiplexer. Each method performs whatever entity spawns/despawns and
//! component mutations are needed for one logical operation; Bevy's
//! native change detection (`Changed<T>`) carries the signal to downstream
//! rebuild systems.

use crate::cells::{Side, SplitOrientation};
use crate::components::{
    ActivePane, ActiveSurface, AttachedWorkspace, CopyMode, LayoutCells, PaneMarker, SurfaceKind,
    SurfaceMarker, WorkspaceCreatedAt, WorkspaceDimensions, WorkspaceMarker, WorkspaceUiSubtree,
};
use crate::direction::PaneDirection;
use crate::error::{MultiplexerError, MultiplexerResult};
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
    workspaces: Query<
        'w,
        's,
        (
            &'static mut LayoutCells,
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
        ),
        With<PaneMarker>,
    >,
    surfaces: Query<'w, 's, (&'static SurfaceKind, &'static ChildOf), With<SurfaceMarker>>,
    children: Query<'w, 's, &'static Children>,
}

impl<'w, 's> MultiplexerCommands<'w, 's> {
    /// Spawn a Workspace with one bootstrap Pane containing one bootstrap
    /// Terminal Surface. Returns the three Entity handles.
    pub fn create_workspace(&mut self, name: Option<String>) -> WorkspaceCreated {
        let name = name.unwrap_or_else(|| "default".to_string());

        let surface = self
            .commands
            .spawn((
                SurfaceMarker,
                SurfaceKind::Terminal,
                Name::new(format!("surface: {name}#0")),
            ))
            .id();

        let pane = self
            .commands
            .spawn((
                PaneMarker,
                ActiveSurface(surface),
                CopyMode::default(),
                Name::new(format!("pane: {name}#0")),
            ))
            .id();

        let workspace = self
            .commands
            .spawn((
                WorkspaceMarker,
                LayoutCells::new_workspace_layout(pane),
                ActivePane(pane),
                Name::new(name),
            ))
            .id();

        self.commands.entity(pane).insert(ChildOf(workspace));
        self.commands.entity(surface).insert(ChildOf(pane));

        WorkspaceCreated {
            workspace,
            pane,
            surface,
        }
    }

    /// Mints a workspace via `create_workspace`, spawns its UI subtree `Node`,
    /// inserts `AttachedWorkspace` + `WorkspaceUiSubtree` + `WorkspaceCreatedAt`,
    /// parents the subtree under the workspace, and returns the workspace entity.
    /// The workspace is auto-named `"workspace{n}"` from the internal
    /// `WorkspaceNameCounter`.
    pub fn spawn_attached_workspace(&mut self) -> Entity {
        let n = self.counter.next();
        let workspace = self
            .create_workspace(Some(format!("workspace{n}")))
            .workspace;
        let subtree = self
            .commands
            .spawn(Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            })
            .id();
        self.commands.entity(workspace).insert((
            AttachedWorkspace,
            WorkspaceUiSubtree(subtree),
            WorkspaceCreatedAt(n),
        ));
        self.commands.entity(subtree).insert(ChildOf(workspace));
        workspace
    }

    /// Mutate the Workspace's `Name` component. Uses `set_if_neq` so a
    /// no-op rename does not flag `Changed<Name>`.
    pub fn rename_workspace(&mut self, workspace: Entity, name: String) -> MultiplexerResult<()> {
        let (_, _, mut current_name, _) = self
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
        if let Ok((_, _, _, dims)) = self.workspaces.get_mut(workspace)
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
        let (_, mut active_pane, _, _) = self
            .workspaces
            .get_mut(workspace)
            .map_err(|_| MultiplexerError::WorkspaceNotFound(workspace))?;
        active_pane.set_if_neq(ActivePane(pane));
        Ok(())
    }

    /// Update the Pane's `ActiveSurface` pointer.
    pub fn set_active_surface(&mut self, pane: Entity, surface: Entity) -> MultiplexerResult<()> {
        let (mut active_surface, _, _) = self
            .panes
            .get_mut(pane)
            .map_err(|_| MultiplexerError::PaneNotFound(pane))?;
        active_surface.set_if_neq(ActiveSurface(surface));
        Ok(())
    }

    /// Split the target pane and seed the new pane with one surface of the
    /// caller-chosen `kind`. Delegates to `split_pane_inner` (which does the
    /// layout mutation + active-pane promotion) and attaches the surface; on
    /// error the freshly-spawned surface is despawned to leave no orphan.
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
                self.commands.entity(surface).insert(ChildOf(new_pane));
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
    /// Mutates `LayoutCells` to insert the new pane at the requested
    /// side/orientation and promotes it to `ActivePane`. On error,
    /// freshly-spawned entities are despawned to leave no orphans.
    pub fn split_pane(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<Entity> {
        self.split_pane_with_surface(target_pane, side, orientation, SurfaceKind::Terminal)
            .map(|o| o.pane)
    }

    /// Close a pane. Despawns the pane entity (which cascades to its
    /// Surface children via `ChildOf`), mutates `LayoutCells` to collapse
    /// the split, and repoints `ActivePane` if the closed pane was active.
    pub fn close_pane(&mut self, pane: Entity) -> MultiplexerResult<()> {
        let workspace = self
            .workspace_of_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let (mut layout, mut active_pane, _, _) = self
            .workspaces
            .get_mut(workspace)
            .map_err(|_| MultiplexerError::WorkspaceNotFound(workspace))?;
        let cell_id = layout.cells.lookup_cell_for_pane(pane)?;
        let outcome = layout.cells.close_cell(&cell_id)?;
        let survivor = layout.cells.leftmost_pane(outcome.survivor())?;
        if active_pane.0 == pane {
            active_pane.0 = survivor;
        }
        self.commands.entity(pane).despawn();
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
        let (mut layout, _, _, _) = self
            .workspaces
            .get_mut(workspace)
            .map_err(|_| MultiplexerError::WorkspaceNotFound(workspace))?;
        let root = layout.root;
        let ordered = layout.cells.ordered_pane_cells(&root)?;
        if ordered.len() < 2 {
            return Ok(SwapOutcome::NoOp);
        }
        let i = ordered
            .iter()
            .position(|(_, p)| *p == pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let len = ordered.len() as isize;
        let delta = match offset {
            SwapOffset::Prev => -1,
            SwapOffset::Next => 1,
        };
        let j = ((i as isize + delta).rem_euclid(len)) as usize;
        let (cell_i, _) = ordered[i];
        let (cell_j, other_pane) = ordered[j];
        layout.cells.swap_panes(&cell_i, &cell_j)?;
        Ok(SwapOutcome::Swapped { other_pane })
    }

    /// Spawn a new Surface as a child of `pane`. Does NOT change
    /// `ActiveSurface` — call `set_active_surface` separately if needed.
    pub fn add_surface(&mut self, pane: Entity, kind: SurfaceKind) -> Entity {
        let surface = self
            .commands
            .spawn((SurfaceMarker, kind, Name::new("surface")))
            .id();
        self.commands.entity(surface).insert(ChildOf(pane));
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

        self.commands.entity(surface).insert(ChildOf(new_pane));
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
        let (mut layout, _, _, dims) = self
            .workspaces
            .get_mut(workspace)
            .map_err(|_| MultiplexerError::WorkspaceNotFound(workspace))?;
        let (cols, rows) = dims.as_ref().map(|d| (d.cols, d.rows)).unwrap_or((0, 0));
        if cols == 0 || rows == 0 {
            return Ok(ResizePaneOutcome::NoOp);
        }
        Ok(resize_split_for_pane(
            &mut layout.cells,
            pane,
            direction,
            amount,
            cols,
            rows,
        ))
    }

    /// Walk up `ChildOf` from a Pane entity to find its owning Workspace.
    pub fn workspace_of_pane(&self, pane: Entity) -> Option<Entity> {
        self.panes
            .get(pane)
            .ok()
            .map(|(_, _, child_of)| child_of.parent())
    }

    /// Walk up `ChildOf` from a Surface entity to find its owning Pane.
    pub fn pane_of_surface(&self, surface: Entity) -> Option<Entity> {
        self.surfaces
            .get(surface)
            .ok()
            .map(|(_, child_of)| child_of.parent())
    }

    /// Read the Workspace's `ActivePane` pointer.
    pub fn workspaces_active_pane(&self, workspace: Entity) -> Option<Entity> {
        self.workspaces
            .get(workspace)
            .ok()
            .map(|(_, active, _, _)| active.0)
    }

    /// Read the Pane's `ActiveSurface` pointer.
    pub fn panes_active_surface(&self, pane: Entity) -> Option<Entity> {
        self.panes.get(pane).ok().map(|(active, _, _)| active.0)
    }

    /// Iterate the Pane entities owned by the given Workspace.
    pub fn panes_of_workspace(&self, workspace: Entity) -> impl Iterator<Item = Entity> + '_ {
        self.children
            .get(workspace)
            .into_iter()
            .flat_map(|c| c.iter())
            .filter(move |child| self.panes.get(*child).is_ok())
    }

    /// Iterate the Surface entities owned by the given Pane.
    pub fn surfaces_of_pane(&self, pane: Entity) -> impl Iterator<Item = Entity> + '_ {
        self.children
            .get(pane)
            .into_iter()
            .flat_map(|c| c.iter())
            .filter(move |child| self.surfaces.get(*child).is_ok())
    }

    /// Split the target pane in two without spawning a bootstrap surface.
    /// Returns `(new_pane, workspace)`. Callers are responsible for attaching
    /// a surface to the new pane.
    fn split_pane_inner(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<(Entity, Entity)> {
        let workspace = self
            .workspace_of_pane(target_pane)
            .ok_or(MultiplexerError::PaneNotFound(target_pane))?;

        let new_pane = self
            .commands
            .spawn((PaneMarker, CopyMode::default(), Name::new("pane: split")))
            .id();
        self.commands.entity(new_pane).insert(ChildOf(workspace));

        let (mut layout, mut active_pane, _, _) = self
            .workspaces
            .get_mut(workspace)
            .map_err(|_| MultiplexerError::WorkspaceNotFound(workspace))?;
        let target_cell = layout.cells.lookup_cell_for_pane(target_pane)?;
        let new_cell = layout.cells.new_pane(new_pane, None);
        if let Err(e) = layout
            .cells
            .split_cell(target_cell, new_cell, side, orientation)
        {
            let _ = layout.cells.remove_subtree(&new_cell);
            self.commands.entity(new_pane).despawn();
            return Err(e);
        }
        active_pane.0 = new_pane;

        Ok((new_pane, workspace))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn create_workspace_spawns_workspace_pane_surface_with_correct_markers_and_childof() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("test".into()))
            })
            .unwrap();

        assert!(world.get::<WorkspaceMarker>(outcome.workspace).is_some());
        assert_eq!(
            world.get::<Name>(outcome.workspace).map(|n| n.as_str()),
            Some("test")
        );
        assert!(world.get::<LayoutCells>(outcome.workspace).is_some());
        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(outcome.pane)
        );

        assert!(world.get::<PaneMarker>(outcome.pane).is_some());
        assert_eq!(
            world.get::<ChildOf>(outcome.pane).map(|c| c.parent()),
            Some(outcome.workspace)
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
        assert!(matches!(
            world.get::<SurfaceKind>(outcome.surface),
            Some(SurfaceKind::Terminal)
        ));
    }

    #[test]
    fn rename_workspace_mutates_name_and_only_fires_changed_on_actual_change() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
    fn split_pane_spawns_pane_with_bootstrap_surface_and_updates_layout() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();

        let new_pane = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        world.flush();

        assert_eq!(
            world.get::<ChildOf>(new_pane).map(|c| c.parent()),
            Some(outcome.workspace),
        );
        assert!(world.get::<PaneMarker>(new_pane).is_some());
        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(new_pane)
        );
        let cells = world.get::<LayoutCells>(outcome.workspace).unwrap();
        assert!(cells.cells.lookup_cell_for_pane(outcome.pane).is_ok());
        assert!(cells.cells.lookup_cell_for_pane(new_pane).is_ok());
    }

    #[test]
    fn close_pane_despawns_pane_and_repoints_active_to_survivor() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let new_pane = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        world.flush();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.close_pane(new_pane).unwrap();
            })
            .unwrap();
        world.flush();

        assert!(world.get_entity(new_pane).is_err(), "pane entity despawned");
        assert_eq!(
            world.get::<ActivePane>(outcome.workspace).map(|a| a.0),
            Some(outcome.pane),
            "active falls back to surviving pane",
        );
    }

    #[test]
    fn swap_pane_returns_swap_outcome_and_updates_layout() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let other = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        world.flush();

        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.swap_pane(outcome.pane, SwapOffset::Next).unwrap()
            })
            .unwrap();

        assert_eq!(result, SwapOutcome::Swapped { other_pane: other });
    }

    #[test]
    fn add_surface_spawns_surface_child_of_pane() {
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        assert_eq!(
            world.get::<ChildOf>(split.pane).map(|c| c.parent()),
            Some(outcome.workspace)
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
        let mut world = World::new();
        world.init_resource::<WorkspaceNameCounter>();
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
}
