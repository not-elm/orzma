//! Session-lifecycle shortcut actions dispatched as `EntityEvent`s.
//!
//! `NewSessionActionEvent` and `FocusSessionActionEvent` are triggered by
//! the keyboard dispatcher (`crate::input::execute_action`) and handled by
//! the observers below. The observers re-query the live `AttachedSession`
//! marker rather than trusting the event's target, so two same-frame
//! triggers preserve the single-holder invariant (Bevy flushes each
//! observer's commands before the next queued trigger runs).

use crate::multiplexer::{SessionCreatedAt, SessionNameCounter};
use bevy::prelude::*;
use ozmux_multiplexer::{AttachedSession, MultiplexerCommands, SessionMarker, SessionUiSubtree};

/// Bevy Plugin that registers the session-action observers.
pub struct OzmuxSessionActionPlugin;

impl Plugin for OzmuxSessionActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_new_session)
            .add_observer(apply_focus_session);
    }
}

/// Request to mint a new session and attach it. Triggered by
/// `ShortcutAction::NewSession`.
#[derive(EntityEvent, Debug)]
pub struct NewSessionActionEvent {
    /// The session attached at dispatch time (trigger target only; the
    /// observer re-queries the live marker).
    #[event_target]
    pub session: Entity,
}

/// Request to move session focus. Triggered by
/// `ShortcutAction::FocusSession` and `ShortcutAction::FocusSessionNumber`.
#[derive(EntityEvent, Debug)]
pub struct FocusSessionActionEvent {
    /// The session attached at dispatch time (trigger target only; the
    /// observer re-queries the live marker).
    #[event_target]
    pub session: Entity,
    /// Which session to focus.
    pub target: FocusSessionTarget,
}

/// Selector for `FocusSessionActionEvent`, unifying `FocusSession{offset}` and `FocusSessionNumber{index}`. `Debug` is required because
/// `FocusSessionActionEvent` derives `Debug`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusSessionTarget {
    Next,
    Prev,
    Last,
    Number(u8),
}

/// Spawns a Session via `MultiplexerCommands` plus its UI subtree node,
/// inserts `AttachedSession` + `SessionUiSubtree` + `SessionCreatedAt`
/// on the session entity, and parents the subtree under the session.
/// Returns the new session entity.
///
/// # Invariants
///
/// Inserts `AttachedSession` on the new session **without** removing it
/// from any prior holder. A caller that must keep the "exactly one
/// `AttachedSession`" invariant (the new-session path) is responsible for
/// detaching the previous marker first; `bootstrap` calls it with no prior
/// attached session, so it is safe there.
pub(crate) fn spawn_attached_session(
    commands: &mut Commands,
    mux: &mut MultiplexerCommands,
    counter: &mut SessionNameCounter,
) -> Entity {
    let n = counter.next();
    let outcome = mux.create_session(Some(format!("session{n}")));
    let subtree = commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        })
        .id();
    commands.entity(outcome.session).insert((
        AttachedSession,
        SessionUiSubtree(subtree),
        SessionCreatedAt(n),
    ));
    commands.entity(subtree).insert(ChildOf(outcome.session));
    outcome.session
}

// NOTE: `mux` must precede `commands` in this observer's signature. Both
// own separate deferred command queues; Bevy applies them in parameter
// order. `spawn_attached_session` queues the new session-entity spawn into
// `mux`, then inserts components on it via `commands`. If `commands`
// applied first, those inserts would reference an entity that does not
// exist yet and panic.
fn apply_new_session(
    _trigger: On<NewSessionActionEvent>,
    mut mux: MultiplexerCommands,
    mut commands: Commands,
    mut counter: ResMut<SessionNameCounter>,
    attached_session: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>,
) {
    match attached_session.single() {
        Ok(previous_attached) => {
            tracing::debug!(
                target: "ozmux_gui::action",
                ?previous_attached,
                "apply_new_session: queued AttachedSession remove from previous"
            );
            commands
                .entity(previous_attached)
                .remove::<AttachedSession>();
        }
        Err(err) => {
            tracing::debug!(
                target: "ozmux_gui::action",
                ?err,
                "apply_new_session: no single previously-attached session (skipping remove)"
            );
        }
    }
    let new_session = spawn_attached_session(&mut commands, &mut mux, &mut counter);
    tracing::debug!(
        target: "ozmux_gui::action",
        ?new_session,
        "apply_new_session: queued spawn of new attached session"
    );
}

fn apply_focus_session(
    trigger: On<FocusSessionActionEvent>,
    mut commands: Commands,
    sessions: Query<(Entity, Option<&SessionCreatedAt>), With<SessionMarker>>,
    attached_session: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>,
) {
    let mut pairs: Vec<(Entity, u32)> = sessions
        .iter()
        .map(|(e, created)| (e, created.map(|c| c.0).unwrap_or(u32::MAX)))
        .collect();
    if pairs.len() < 2 {
        return;
    }
    pairs.sort_by_key(|(_, c)| *c);
    let entries: Vec<Entity> = pairs.into_iter().map(|(e, _)| e).collect();

    let Ok(current_entity) = attached_session.single() else {
        return;
    };
    let Some(current_idx) = entries.iter().position(|e| *e == current_entity) else {
        return;
    };

    let target_idx = match trigger.event().target {
        FocusSessionTarget::Next => (current_idx + 1) % entries.len(),
        FocusSessionTarget::Prev => current_idx.checked_sub(1).unwrap_or(entries.len() - 1),
        FocusSessionTarget::Last => {
            tracing::debug!(
                target: "ozmux_gui::action",
                "FocusSession::Last not yet implemented"
            );
            return;
        }
        FocusSessionTarget::Number(index) => {
            let i = index as usize;
            if i >= entries.len() {
                return;
            }
            i
        }
    };

    let target_entity = entries[target_idx];
    if target_entity == current_entity {
        return;
    }

    commands.entity(current_entity).remove::<AttachedSession>();
    commands.entity(target_entity).insert(AttachedSession);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{MultiplexerPlugin, SessionUiSubtree};

    /// Builds an app with the multiplexer + session-action observers and a
    /// single attached "default" session (no `SessionCreatedAt`, mirroring
    /// the pre-counter bootstrap session). Returns the session entity.
    fn setup_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(OzmuxSessionActionPlugin);
        app.init_resource::<SessionNameCounter>();
        let session = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("default".into()))
            })
            .unwrap()
            .session;
        app.world_mut().flush();
        app.world_mut().entity_mut(session).insert(AttachedSession);
        (app, session)
    }

    fn count_session_entities(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<SessionMarker>>()
            .iter(app.world())
            .count()
    }

    fn count_attached_session_entities(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, (With<SessionMarker>, With<AttachedSession>)>()
            .iter(app.world())
            .count()
    }

    fn attached_now(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, (With<SessionMarker>, With<AttachedSession>)>()
            .iter(app.world())
            .next()
            .unwrap()
    }

    #[test]
    fn plugin_builds_without_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(OzmuxSessionActionPlugin);
        app.update();
    }

    #[test]
    fn new_session_event_spawns_entity_with_subtree_and_moves_marker() {
        let (mut app, bootstrap) = setup_app();
        assert_eq!(count_session_entities(&mut app), 1);
        assert_eq!(count_attached_session_entities(&mut app), 1);

        app.world_mut()
            .trigger(NewSessionActionEvent { session: bootstrap });
        app.update();

        assert_eq!(count_session_entities(&mut app), 2);
        assert_eq!(count_attached_session_entities(&mut app), 1);
        let new_attached = attached_now(&mut app);
        assert_ne!(new_attached, bootstrap);
        assert!(
            app.world().get::<SessionUiSubtree>(new_attached).is_some(),
            "new attached session must carry a SessionUiSubtree pointer",
        );
    }

    #[test]
    fn two_new_session_events_same_frame_keep_marker_invariant() {
        // §6.2: queue two triggers in ONE system run (mirroring
        // execute_action's keyboard loop), flush once. Bevy applies the
        // first observer's marker move before the second observer runs, so
        // exactly one AttachedSession survives and TWO new sessions exist.
        let (mut app, bootstrap) = setup_app();
        assert_eq!(count_session_entities(&mut app), 1);

        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                commands.trigger(NewSessionActionEvent { session: bootstrap });
                commands.trigger(NewSessionActionEvent { session: bootstrap });
            })
            .unwrap();
        app.update();

        assert_eq!(
            count_attached_session_entities(&mut app),
            1,
            "exactly one AttachedSession after two same-frame NewSession triggers",
        );
        assert_eq!(
            count_session_entities(&mut app),
            3,
            "two same-frame NewSession triggers must create two new sessions",
        );
    }

    #[test]
    fn new_session_event_uses_monotonic_name_and_created_at() {
        let (mut app, bootstrap) = setup_app();
        app.world_mut()
            .trigger(NewSessionActionEvent { session: bootstrap });
        app.update();
        let first_new = attached_now(&mut app);
        app.world_mut()
            .trigger(NewSessionActionEvent { session: first_new });
        app.update();

        let world = app.world_mut();
        let mut created = world
            .query_filtered::<&SessionCreatedAt, With<SessionMarker>>()
            .iter(world)
            .map(|c| c.0)
            .collect::<Vec<u32>>();
        created.sort_unstable();
        assert_eq!(created, vec![1, 2]);

        let mut names = world
            .query_filtered::<&Name, With<SessionMarker>>()
            .iter(world)
            .map(|n| n.as_str().to_owned())
            .collect::<Vec<String>>();
        names.sort();
        assert_eq!(names, vec!["default", "session1", "session2"]);
    }

    #[test]
    fn focus_session_number_targets_sorted_index() {
        let (mut app, bootstrap) = setup_app();
        // Add a second session via the event; it gets SessionCreatedAt(1),
        // so sort order is [session1(1), default(u32::MAX)].
        app.world_mut()
            .trigger(NewSessionActionEvent { session: bootstrap });
        app.update();
        let session1 = attached_now(&mut app);

        // Index 1 → the "default" session (sorts last because it has no
        // SessionCreatedAt). The current attached is session1 (index 0).
        app.world_mut().trigger(FocusSessionActionEvent {
            session: session1,
            target: FocusSessionTarget::Number(1),
        });
        app.update();

        assert_eq!(count_attached_session_entities(&mut app), 1);
        let focused = attached_now(&mut app);
        assert_eq!(focused, bootstrap, "Number(1) targets the default session");
    }

    #[test]
    fn focus_session_next_moves_marker_to_other_session() {
        let (mut app, bootstrap) = setup_app();
        app.world_mut()
            .trigger(NewSessionActionEvent { session: bootstrap });
        app.update();
        let session1 = attached_now(&mut app);

        app.world_mut().trigger(FocusSessionActionEvent {
            session: session1,
            target: FocusSessionTarget::Next,
        });
        app.update();

        assert_eq!(count_attached_session_entities(&mut app), 1);
        assert_ne!(
            attached_now(&mut app),
            session1,
            "Next must move the marker off the currently-attached session",
        );
    }

    #[test]
    fn focus_session_prev_moves_marker_to_other_session() {
        let (mut app, bootstrap) = setup_app();
        app.world_mut()
            .trigger(NewSessionActionEvent { session: bootstrap });
        app.update();
        let session1 = attached_now(&mut app);

        app.world_mut().trigger(FocusSessionActionEvent {
            session: session1,
            target: FocusSessionTarget::Prev,
        });
        app.update();

        assert_eq!(count_attached_session_entities(&mut app), 1);
        assert_ne!(
            attached_now(&mut app),
            session1,
            "Prev must move the marker off the currently-attached session",
        );
    }

    #[test]
    fn focus_session_number_out_of_bounds_is_noop() {
        let (mut app, bootstrap) = setup_app();
        app.world_mut()
            .trigger(NewSessionActionEvent { session: bootstrap });
        app.update();
        let session1 = attached_now(&mut app);

        app.world_mut().trigger(FocusSessionActionEvent {
            session: session1,
            target: FocusSessionTarget::Number(99),
        });
        app.update();

        assert_eq!(count_attached_session_entities(&mut app), 1);
        assert_eq!(
            attached_now(&mut app),
            session1,
            "out-of-bounds Number must leave the marker unchanged",
        );
    }
}
