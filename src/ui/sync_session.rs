//! System that reparents the attached Session's UI subtree between
//! `SessionUiRoot` (active) and its owning Session entity (parked). The
//! Session entity is non-`Node`, so a parked subtree is skipped by Bevy's
//! UI walker — no layout, no `ComputedNode` updates, no resize-pass work.

use crate::multiplexer::{AttachedSession, SessionEntityId, SessionUiSubtree};
use crate::ui::SessionUiRoot;
use bevy::prelude::*;

/// Runs every Update; only does work when the set of `AttachedSession`
/// markers changes. Tracks the previously-attached session's Entity in a
/// `Local<Option<Entity>>` so we can look up its `SessionUiSubtree` and
/// park it back under the Session entity.
pub(crate) fn sync_active_session(
    mut commands: Commands,
    mut last_attached: Local<Option<Entity>>,
    attached_q: Query<(Entity, &SessionEntityId, &SessionUiSubtree), With<AttachedSession>>,
    sessions_q: Query<(Entity, &SessionUiSubtree)>,
    session_ui_root_q: Query<Entity, With<SessionUiRoot>>,
) {
    let Ok(session_ui_root) = session_ui_root_q.single() else {
        return;
    };
    let Ok((newly_attached_entity, _newly_attached_sid, newly_attached_subtree)) =
        attached_q.single()
    else {
        return;
    };

    if *last_attached == Some(newly_attached_entity) {
        return;
    }

    if let Some(prev_session_entity) = *last_attached
        && let Ok((_, prev_subtree)) = sessions_q.get(prev_session_entity)
    {
        commands
            .entity(prev_subtree.0)
            .insert(ChildOf(prev_session_entity));
    }

    commands
        .entity(newly_attached_subtree.0)
        .insert(ChildOf(session_ui_root));

    *last_attached = Some(newly_attached_entity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::SessionUiRoot;
    use bevy::window::{PrimaryWindow, WindowResolution};
    use ozmux_multiplexer::SessionId;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        let ui_root = app.world_mut().spawn(Node::default()).id();
        app.world_mut()
            .spawn((Node::default(), SessionUiRoot, ChildOf(ui_root)));
        app.add_systems(Update, sync_active_session);
        app
    }

    #[test]
    fn attaches_initial_session_subtree_to_session_ui_root() {
        let mut app = build_app();

        let subtree = app.world_mut().spawn(Node::default()).id();
        let session = app
            .world_mut()
            .spawn((
                SessionEntityId(SessionId(0)),
                AttachedSession,
                SessionUiSubtree(subtree),
            ))
            .id();
        app.world_mut().entity_mut(subtree).insert(ChildOf(session));

        app.update();

        let session_ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<SessionUiRoot>>()
            .single(app.world())
            .expect("SessionUiRoot");
        let parent = app
            .world()
            .get::<ChildOf>(subtree)
            .expect("subtree has parent")
            .parent();
        assert_eq!(
            parent, session_ui_root,
            "active session's subtree must be under SessionUiRoot"
        );
    }

    #[test]
    fn switching_active_session_parks_previous_subtree_under_its_session_entity() {
        let mut app = build_app();

        let subtree_a = app.world_mut().spawn(Node::default()).id();
        let session_a = app
            .world_mut()
            .spawn((
                SessionEntityId(SessionId(0)),
                AttachedSession,
                SessionUiSubtree(subtree_a),
            ))
            .id();
        app.world_mut()
            .entity_mut(subtree_a)
            .insert(ChildOf(session_a));

        let subtree_b = app.world_mut().spawn(Node::default()).id();
        let session_b = app
            .world_mut()
            .spawn((SessionEntityId(SessionId(1)), SessionUiSubtree(subtree_b)))
            .id();
        app.world_mut()
            .entity_mut(subtree_b)
            .insert(ChildOf(session_b));

        app.update();

        app.world_mut()
            .entity_mut(session_a)
            .remove::<AttachedSession>();
        app.world_mut()
            .entity_mut(session_b)
            .insert(AttachedSession);
        app.update();

        let session_ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<SessionUiRoot>>()
            .single(app.world())
            .expect("SessionUiRoot");
        let parent_a = app
            .world()
            .get::<ChildOf>(subtree_a)
            .expect("a subtree has parent")
            .parent();
        let parent_b = app
            .world()
            .get::<ChildOf>(subtree_b)
            .expect("b subtree has parent")
            .parent();
        assert_eq!(
            parent_a, session_a,
            "previous subtree must park under its Session entity"
        );
        assert_eq!(
            parent_b, session_ui_root,
            "new subtree must attach to SessionUiRoot"
        );
    }
}
