//! `MultiplexerCommands` SystemParam — the sole mutation API for the
//! multiplexer. Each method performs whatever entity spawns/despawns and
//! component mutations are needed for one logical operation; Bevy's
//! native change detection (`Changed<T>`) carries the signal to downstream
//! rebuild systems.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use crate::cells::LayoutCellState;
use crate::components::{
    ActiveActivity, ActivePane, ActivityKind, ActivityMarker, CopyMode, LayoutCells, PaneMarker,
    SessionMarker,
};

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

        let mut cells = LayoutCellState::default();
        let (_root_cell_id, _pane_cell_id) = cells.new_session_layout(pane);

        let session = self
            .commands
            .spawn((
                SessionMarker,
                LayoutCells(cells),
                ActivePane(pane),
                Name::new(name),
            ))
            .id();

        self.commands.entity(pane).insert(ChildOf(session));
        self.commands.entity(activity).insert(ChildOf(pane));

        SessionCreated { session, pane, activity }
    }

    /// Walk up `ChildOf` from a Pane entity to find its owning Session.
    pub fn session_of_pane(&self, pane: Entity) -> Option<Entity> {
        self.panes.get(pane).ok().map(|(_, _, child_of)| child_of.parent())
    }

    /// Walk up `ChildOf` from an Activity entity to find its owning Pane.
    pub fn pane_of_activity(&self, activity: Entity) -> Option<Entity> {
        self.activities.get(activity).ok().map(|(_, child_of)| child_of.parent())
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
}
