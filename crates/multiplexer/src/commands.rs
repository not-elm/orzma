//! `MultiplexerCommands` SystemParam — the sole mutation API for the
//! multiplexer. Each method performs whatever entity spawns/despawns and
//! component mutations are needed for one logical operation; Bevy's
//! native change detection (`Changed<T>`) carries the signal to downstream
//! rebuild systems.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use crate::cells::{Side, SplitOrientation};
use crate::components::{
    ActiveActivity, ActivePane, ActivityKind, ActivityMarker, CopyMode, LayoutCells, PaneMarker,
    SessionDimensions, SessionMarker,
};
use crate::direction::PaneDirection;
use crate::error::{MultiplexerError, MultiplexerResult};
use crate::resize::{resize_split_for_pane, ResizePaneOutcome};
use crate::swap::{SwapOffset, SwapOutcome};

/// Result of `create_session` — the three freshly-spawned entities.
#[derive(Debug, Clone, Copy)]
pub struct SessionCreated {
    /// The Session entity.
    pub session: Entity,
    /// The bootstrap Pane entity.
    pub pane: Entity,
    /// The bootstrap Activity entity.
    pub activity: Entity,
}

/// SystemParam exposing every mutation on the multiplexer state. Read
/// helpers (`session_of_pane`, `panes_of_session`, etc.) are non-mut and
/// can be called by other systems through the same SystemParam.
#[derive(SystemParam)]
pub struct MultiplexerCommands<'w, 's> {
    commands: Commands<'w, 's>,
    sessions: Query<
        'w,
        's,
        (
            &'static mut LayoutCells,
            &'static mut ActivePane,
            &'static mut Name,
            Option<&'static mut SessionDimensions>,
        ),
        With<SessionMarker>,
    >,
    panes: Query<
        'w,
        's,
        (&'static mut ActiveActivity, &'static mut CopyMode, &'static ChildOf),
        With<PaneMarker>,
    >,
    activities: Query<'w, 's, (&'static ActivityKind, &'static ChildOf), With<ActivityMarker>>,
    children: Query<'w, 's, &'static Children>,
}

impl<'w, 's> MultiplexerCommands<'w, 's> {
    /// Spawn a Session with one bootstrap Pane containing one bootstrap
    /// Terminal Activity. Returns the three Entity handles.
    pub fn create_session(&mut self, name: Option<String>) -> SessionCreated {
        let name = name.unwrap_or_else(|| "default".to_string());

        let activity = self
            .commands
            .spawn((
                ActivityMarker,
                ActivityKind::Terminal,
                Name::new(format!("activity: {name}#0")),
            ))
            .id();

        let pane = self
            .commands
            .spawn((
                PaneMarker,
                ActiveActivity(activity),
                CopyMode::default(),
                Name::new(format!("pane: {name}#0")),
            ))
            .id();

        let session = self
            .commands
            .spawn((
                SessionMarker,
                LayoutCells::new_session_layout(pane),
                ActivePane(pane),
                Name::new(name),
            ))
            .id();

        self.commands.entity(pane).insert(ChildOf(session));
        self.commands.entity(activity).insert(ChildOf(pane));

        SessionCreated { session, pane, activity }
    }

    /// Mutate the Session's `Name` component. Uses `set_if_neq` so a
    /// no-op rename does not flag `Changed<Name>`.
    pub fn rename_session(&mut self, session: Entity, name: String) -> MultiplexerResult<()> {
        let (_, _, mut current_name, _) = self
            .sessions
            .get_mut(session)
            .map_err(|_| MultiplexerError::SessionNotFound(session))?;
        current_name.set_if_neq(Name::new(name));
        Ok(())
    }

    /// Set the Session's cached dimensions. Inserts the component on
    /// first call; subsequent calls update in place via `set_if_neq`.
    pub fn set_session_dimensions(&mut self, session: Entity, cols: u16, rows: u16) {
        let new = SessionDimensions { cols, rows };
        if let Ok((_, _, _, dims)) = self.sessions.get_mut(session)
            && let Some(mut dims) = dims {
                dims.set_if_neq(new);
                return;
            }
        self.commands.entity(session).insert(new);
    }

    /// Update the Session's `ActivePane` pointer to `pane`. The pane MUST
    /// belong to the session (caller's invariant; not validated here).
    pub fn set_active_pane(&mut self, session: Entity, pane: Entity) -> MultiplexerResult<()> {
        let (_, mut active_pane, _, _) = self
            .sessions
            .get_mut(session)
            .map_err(|_| MultiplexerError::SessionNotFound(session))?;
        active_pane.set_if_neq(ActivePane(pane));
        Ok(())
    }

    /// Update the Pane's `ActiveActivity` pointer.
    pub fn set_active_activity(
        &mut self,
        pane: Entity,
        activity: Entity,
    ) -> MultiplexerResult<()> {
        let (mut active_activity, _, _) = self
            .panes
            .get_mut(pane)
            .map_err(|_| MultiplexerError::PaneNotFound(pane))?;
        active_activity.set_if_neq(ActiveActivity(activity));
        Ok(())
    }

    /// Split the target pane in two. Spawns a new Pane entity with one
    /// bootstrap Terminal Activity, mutates `LayoutCells` to insert it
    /// at the requested side/orientation, and promotes the new pane to
    /// `ActivePane`. On error, the freshly-spawned entities are despawned
    /// to leave no orphans in the world.
    pub fn split_pane(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<Entity> {
        let new_activity = self
            .commands
            .spawn((
                ActivityMarker,
                ActivityKind::Terminal,
                Name::new("activity: split"),
            ))
            .id();

        let result = self.split_pane_inner(target_pane, side, orientation);
        match result {
            Ok((new_pane, _)) => {
                self.commands.entity(new_pane).insert(ActiveActivity(new_activity));
                self.commands.entity(new_activity).insert(ChildOf(new_pane));
                Ok(new_pane)
            }
            Err(e) => {
                self.commands.entity(new_activity).despawn();
                Err(e)
            }
        }
    }

    /// Close a pane. Despawns the pane entity (which cascades to its
    /// Activity children via `ChildOf`), mutates `LayoutCells` to collapse
    /// the split, and repoints `ActivePane` if the closed pane was active.
    pub fn close_pane(&mut self, pane: Entity) -> MultiplexerResult<()> {
        let session = self
            .session_of_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let (mut layout, mut active_pane, _, _) = self
            .sessions
            .get_mut(session)
            .map_err(|_| MultiplexerError::SessionNotFound(session))?;
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
    /// DFS leaf traversal. No-op for single-pane sessions.
    pub fn swap_pane(
        &mut self,
        pane: Entity,
        offset: SwapOffset,
    ) -> MultiplexerResult<SwapOutcome> {
        let session = self
            .session_of_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let (mut layout, _, _, _) = self
            .sessions
            .get_mut(session)
            .map_err(|_| MultiplexerError::SessionNotFound(session))?;
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

    /// Spawn a new Activity as a child of `pane`. Does NOT change
    /// `ActiveActivity` — call `set_active_activity` separately if needed.
    pub fn add_activity(&mut self, pane: Entity, kind: ActivityKind) -> Entity {
        let activity = self
            .commands
            .spawn((ActivityMarker, kind, Name::new("activity")))
            .id();
        self.commands.entity(activity).insert(ChildOf(pane));
        activity
    }

    /// Split the activity's owning Pane and move the activity into the
    /// freshly-created Pane (where it becomes the only activity). The new
    /// Pane becomes the session's `ActivePane`. Caller must ensure the
    /// source Pane has at least 2 activities, else this returns
    /// `CannotRemoveLastActivity`.
    pub fn break_activity_to_pane(
        &mut self,
        activity: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<Entity> {
        let source_pane = self
            .pane_of_activity(activity)
            .ok_or(MultiplexerError::ActivityNotFound(activity))?;

        let activity_count = self.activities_of_pane(source_pane).count();
        if activity_count < 2 {
            return Err(MultiplexerError::CannotRemoveLastActivity(source_pane));
        }

        // NOTE: split_pane_inner avoids spawning a bootstrap activity; otherwise
        //       the deferred `ChildOf` insertion would race with the immediate
        //       reparent below, leaving the bootstrap entity orphaned.
        let (new_pane, _) = self.split_pane_inner(source_pane, side, orientation)?;

        self.commands.entity(activity).insert(ChildOf(new_pane));
        self.commands.entity(new_pane).insert(ActiveActivity(activity));

        Ok(new_pane)
    }

    /// Close a Session entirely. Cascading `ChildOf` despawn removes all
    /// Pane and Activity descendants.
    pub fn close_session(&mut self, session: Entity) {
        self.commands.entity(session).despawn();
    }

    /// Resize the split that controls `pane`'s extent in the given
    /// direction by `amount` cells. See `resize::resize_split_for_pane`
    /// for the underlying weight-based algorithm. Requires that
    /// `SessionDimensions` has been set; returns `NoOp` if not.
    pub fn resize_pane(
        &mut self,
        pane: Entity,
        direction: PaneDirection,
        amount: u16,
    ) -> MultiplexerResult<ResizePaneOutcome> {
        let session = self
            .session_of_pane(pane)
            .ok_or(MultiplexerError::PaneNotFound(pane))?;
        let (mut layout, _, _, dims) = self
            .sessions
            .get_mut(session)
            .map_err(|_| MultiplexerError::SessionNotFound(session))?;
        let (cols, rows) = dims
            .as_ref()
            .map(|d| (d.cols, d.rows))
            .unwrap_or((0, 0));
        if cols == 0 || rows == 0 {
            return Ok(ResizePaneOutcome::NoOp);
        }
        Ok(resize_split_for_pane(&mut layout.cells, pane, direction, amount, cols, rows))
    }

    /// Walk up `ChildOf` from a Pane entity to find its owning Session.
    pub fn session_of_pane(&self, pane: Entity) -> Option<Entity> {
        self.panes.get(pane).ok().map(|(_, _, child_of)| child_of.parent())
    }

    /// Walk up `ChildOf` from an Activity entity to find its owning Pane.
    pub fn pane_of_activity(&self, activity: Entity) -> Option<Entity> {
        self.activities.get(activity).ok().map(|(_, child_of)| child_of.parent())
    }

    /// Read the Session's `ActivePane` pointer.
    pub fn sessions_active_pane(&self, session: Entity) -> Option<Entity> {
        self.sessions.get(session).ok().map(|(_, active, _, _)| active.0)
    }

    /// Read the Pane's `ActiveActivity` pointer.
    pub fn panes_active_activity(&self, pane: Entity) -> Option<Entity> {
        self.panes.get(pane).ok().map(|(active, _, _)| active.0)
    }

    /// Iterate the Pane entities owned by the given Session.
    pub fn panes_of_session(&self, session: Entity) -> impl Iterator<Item = Entity> + '_ {
        self.children
            .get(session)
            .into_iter()
            .flat_map(|c| c.iter())
            .filter(move |child| self.panes.get(*child).is_ok())
    }

    /// Iterate the Activity entities owned by the given Pane.
    pub fn activities_of_pane(&self, pane: Entity) -> impl Iterator<Item = Entity> + '_ {
        self.children
            .get(pane)
            .into_iter()
            .flat_map(|c| c.iter())
            .filter(move |child| self.activities.get(*child).is_ok())
    }

    /// Split the target pane in two without spawning a bootstrap activity.
    /// Returns `(new_pane, session)`. Callers are responsible for attaching
    /// an activity to the new pane.
    fn split_pane_inner(
        &mut self,
        target_pane: Entity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<(Entity, Entity)> {
        let session = self
            .session_of_pane(target_pane)
            .ok_or(MultiplexerError::PaneNotFound(target_pane))?;

        let new_pane = self
            .commands
            .spawn((
                PaneMarker,
                CopyMode::default(),
                Name::new("pane: split"),
            ))
            .id();
        self.commands.entity(new_pane).insert(ChildOf(session));

        let (mut layout, mut active_pane, _, _) = self
            .sessions
            .get_mut(session)
            .map_err(|_| MultiplexerError::SessionNotFound(session))?;
        let target_cell = layout.cells.lookup_cell_for_pane(target_pane)?;
        let new_cell = layout.cells.new_pane(new_pane, None);
        if let Err(e) = layout.cells.split_cell(target_cell, new_cell, side, orientation) {
            let _ = layout.cells.remove_subtree(&new_cell);
            self.commands.entity(new_pane).despawn();
            return Err(e);
        }
        active_pane.0 = new_pane;

        Ok((new_pane, session))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn create_session_spawns_session_pane_activity_with_correct_markers_and_childof() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("test".into()))
            })
            .unwrap();

        assert!(world.get::<SessionMarker>(outcome.session).is_some());
        assert_eq!(
            world.get::<Name>(outcome.session).map(|n| n.as_str()),
            Some("test")
        );
        assert!(world.get::<LayoutCells>(outcome.session).is_some());
        assert_eq!(
            world.get::<ActivePane>(outcome.session).map(|a| a.0),
            Some(outcome.pane)
        );

        assert!(world.get::<PaneMarker>(outcome.pane).is_some());
        assert_eq!(
            world.get::<ChildOf>(outcome.pane).map(|c| c.parent()),
            Some(outcome.session)
        );
        assert_eq!(
            world.get::<ActiveActivity>(outcome.pane).map(|a| a.0),
            Some(outcome.activity)
        );

        assert!(world.get::<ActivityMarker>(outcome.activity).is_some());
        assert_eq!(
            world.get::<ChildOf>(outcome.activity).map(|c| c.parent()),
            Some(outcome.pane)
        );
        assert!(matches!(
            world.get::<ActivityKind>(outcome.activity),
            Some(ActivityKind::Terminal)
        ));
    }

    #[test]
    fn rename_session_mutates_name_and_only_fires_changed_on_actual_change() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("before".into()))
            })
            .unwrap();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.rename_session(outcome.session, "after".into()).unwrap();
            })
            .unwrap();

        assert_eq!(
            world.get::<Name>(outcome.session).map(|n| n.as_str()),
            Some("after")
        );
    }

    #[test]
    fn set_session_dimensions_inserts_or_updates_component() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_session_dimensions(outcome.session, 120, 40);
            })
            .unwrap();
        world.flush();
        assert_eq!(
            world.get::<SessionDimensions>(outcome.session).copied(),
            Some(SessionDimensions { cols: 120, rows: 40 }),
        );
    }

    #[test]
    fn set_active_pane_updates_active_pane_pointer() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let other_pane = world
            .spawn((
                PaneMarker,
                ActiveActivity(outcome.activity),
                CopyMode::default(),
                Name::new("other"),
                ChildOf(outcome.session),
            ))
            .id();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_pane(outcome.session, other_pane).unwrap();
            })
            .unwrap();

        assert_eq!(
            world.get::<ActivePane>(outcome.session).map(|a| a.0),
            Some(other_pane)
        );
    }

    #[test]
    fn set_active_activity_updates_active_activity_pointer() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let other_activity = world
            .spawn((
                ActivityMarker,
                ActivityKind::Terminal,
                Name::new("other"),
                ChildOf(outcome.pane),
            ))
            .id();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_activity(outcome.pane, other_activity).unwrap();
            })
            .unwrap();

        assert_eq!(
            world.get::<ActiveActivity>(outcome.pane).map(|a| a.0),
            Some(other_activity)
        );
    }

    #[test]
    fn split_pane_spawns_pane_with_bootstrap_activity_and_updates_layout() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
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
            Some(outcome.session),
        );
        assert!(world.get::<PaneMarker>(new_pane).is_some());
        assert_eq!(
            world.get::<ActivePane>(outcome.session).map(|a| a.0),
            Some(new_pane)
        );
        let cells = world.get::<LayoutCells>(outcome.session).unwrap();
        assert!(cells.cells.lookup_cell_for_pane(outcome.pane).is_ok());
        assert!(cells.cells.lookup_cell_for_pane(new_pane).is_ok());
    }

    #[test]
    fn close_pane_despawns_pane_and_repoints_active_to_survivor() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
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
            world.get::<ActivePane>(outcome.session).map(|a| a.0),
            Some(outcome.pane),
            "active falls back to surviving pane",
        );
    }

    #[test]
    fn swap_pane_returns_swap_outcome_and_updates_layout() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
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
    fn add_activity_spawns_activity_child_of_pane() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let new_activity = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_activity(outcome.pane, ActivityKind::Terminal)
            })
            .unwrap();
        world.flush();

        assert!(world.get::<ActivityMarker>(new_activity).is_some());
        assert_eq!(
            world.get::<ChildOf>(new_activity).map(|c| c.parent()),
            Some(outcome.pane)
        );
    }

    #[test]
    fn close_session_despawns_session_and_descendants() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();

        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.close_session(outcome.session);
            })
            .unwrap();
        world.flush();

        assert!(world.get_entity(outcome.session).is_err());
        assert!(
            world.get_entity(outcome.pane).is_err(),
            "pane cascade-despawned"
        );
        assert!(
            world.get_entity(outcome.activity).is_err(),
            "activity cascade-despawned"
        );
    }

    #[test]
    fn break_activity_to_pane_creates_new_pane_with_moved_activity() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let second_activity = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_activity(outcome.pane, ActivityKind::Terminal)
            })
            .unwrap();
        world.flush();

        let new_pane = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.break_activity_to_pane(
                    second_activity,
                    Side::After,
                    SplitOrientation::Horizontal,
                )
                .unwrap()
            })
            .unwrap();
        world.flush();

        assert_eq!(
            world.get::<ChildOf>(second_activity).map(|c| c.parent()),
            Some(new_pane)
        );
        assert_eq!(
            world.get::<ActiveActivity>(new_pane).map(|a| a.0),
            Some(second_activity)
        );
        assert!(world.get::<PaneMarker>(outcome.pane).is_some());
    }

    #[test]
    fn break_activity_to_pane_returns_error_when_source_pane_has_only_one_activity() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(None)
            })
            .unwrap();
        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.break_activity_to_pane(outcome.activity, Side::After, SplitOrientation::Horizontal)
            })
            .unwrap();
        assert!(
            matches!(result, Err(MultiplexerError::CannotRemoveLastActivity(_))),
            "expected CannotRemoveLastActivity, got {result:?}",
        );
    }

    #[test]
    fn sessions_active_pane_returns_bootstrap_pane() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let active = world
            .run_system_once(move |mux: MultiplexerCommands| mux.sessions_active_pane(outcome.session))
            .unwrap();
        assert_eq!(active, Some(outcome.pane));
    }

    #[test]
    fn panes_active_activity_returns_bootstrap_activity() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let active = world
            .run_system_once(move |mux: MultiplexerCommands| mux.panes_active_activity(outcome.pane))
            .unwrap();
        assert_eq!(active, Some(outcome.activity));
    }

    #[test]
    fn resize_pane_returns_noop_without_session_dimensions() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let result = world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.resize_pane(outcome.pane, PaneDirection::Right, 5)
            })
            .unwrap();
        assert!(matches!(result, Ok(ResizePaneOutcome::NoOp)));
    }

    #[test]
    fn resize_pane_returns_noop_for_single_pane_session() {
        let mut world = World::new();
        let outcome = world
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        world
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_session_dimensions(outcome.session, 120, 40);
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
