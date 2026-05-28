//! `OzmuxBootstrapPlugin` registers the Startup `bootstrap` system which
//! seeds the initial Session via `MultiplexerCommands` and attaches the
//! UI subtree pointer + `AttachedSession` marker to the spawned Session
//! entity.

use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozmux_multiplexer::{AttachedSession, MultiplexerCommands, SessionUiSubtree};

/// Bevy Plugin that registers the `bootstrap` system in the `Startup`
/// schedule.
pub struct OzmuxBootstrapPlugin;

impl Plugin for OzmuxBootstrapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (bootstrap, insert_initial_cursor_icon));
    }
}

// NOTE: `mux` must precede `commands` so its command buffer is applied first.
// Both params have separate deferred queues; apply order follows parameter order.
// If `commands` went first, `entity(outcome.session).insert(…)` would panic
// because `outcome.session` is only spawned when `mux`'s queue is applied.
pub(crate) fn bootstrap(mut mux: MultiplexerCommands, mut commands: Commands) {
    let outcome = mux.create_session(Some("default".into()));

    let subtree_root = commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        })
        .id();
    commands
        .entity(outcome.session)
        .insert((AttachedSession, SessionUiSubtree(subtree_root)));
    commands.entity(subtree_root).insert(ChildOf(outcome.session));
}

/// Inserts an initial `CursorIcon::System(SystemCursorIcon::Text)` on
/// the primary window so the hover system in `src/input/hyperlink.rs`
/// can mutate the component without first having to insert it.
fn insert_initial_cursor_icon(
    mut commands: Commands,
    windows: Query<Entity, (With<PrimaryWindow>, Without<CursorIcon>)>,
) {
    for window in windows.iter() {
        commands
            .entity(window)
            .insert(CursorIcon::System(SystemCursorIcon::Text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_multiplexer::{MultiplexerPlugin, SessionMarker};

    #[test]
    fn bootstrap_spawns_session_entity_with_attached_marker() {
        let _guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe { std::env::remove_var("OZMUX_CONFIG"); }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(OzmuxBootstrapPlugin);
        app.update();

        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, (With<SessionMarker>, With<AttachedSession>)>();
        assert_eq!(q.iter(world).count(), 1, "exactly one attached session entity");
    }

    #[test]
    fn bootstrap_attaches_subtree_pointer() {
        let _guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe { std::env::remove_var("OZMUX_CONFIG"); }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(OzmuxBootstrapPlugin);
        app.update();

        let world = app.world_mut();
        let mut q = world.query::<(&SessionMarker, &SessionUiSubtree)>();
        let row = q.iter(world).next().expect("session has subtree pointer");
        assert!(world.get_entity(row.1.0).is_ok());
    }
}
