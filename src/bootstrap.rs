//! `OzmuxBootstrapPlugin` registers the Startup `bootstrap` system which
//! seeds the initial Session via `MultiplexerCommands` and attaches the
//! UI subtree pointer + `AttachedSession` marker to the spawned Session
//! entity.

use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozmux_multiplexer::MultiplexerCommands;

/// Bevy Plugin that registers the `bootstrap` system in the `Startup`
/// schedule.
pub struct OzmuxBootstrapPlugin;

impl Plugin for OzmuxBootstrapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (bootstrap, insert_initial_cursor_icon));
    }
}

pub(crate) fn bootstrap(mut mux: MultiplexerCommands) {
    let _ = mux.spawn_attached_session();
}

/// Inserts an initial `CursorIcon::System(SystemCursorIcon::Default)`
/// (the arrow) on the primary window so the hover system in
/// `src/input/hyperlink.rs` can mutate the component without first
/// having to insert it. The arrow is the default for non-terminal
/// regions; the hover system narrows it to the I-beam over terminal text.
fn insert_initial_cursor_icon(
    mut commands: Commands,
    windows: Query<Entity, (With<PrimaryWindow>, Without<CursorIcon>)>,
) {
    for window in windows.iter() {
        commands
            .entity(window)
            .insert(CursorIcon::System(SystemCursorIcon::Default));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_multiplexer::{AttachedSession, MultiplexerPlugin, SessionMarker, SessionUiSubtree};

    #[test]
    fn bootstrap_spawns_session_entity_with_attached_marker() {
        let _guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(OzmuxBootstrapPlugin);
        app.update();

        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, (With<SessionMarker>, With<AttachedSession>)>();
        assert_eq!(
            q.iter(world).count(),
            1,
            "exactly one attached session entity"
        );
    }

    #[test]
    fn bootstrap_names_the_initial_session_session1() {
        let _guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(OzmuxBootstrapPlugin);
        app.update();

        let world = app.world_mut();
        let mut q = world.query_filtered::<&Name, With<SessionMarker>>();
        let names: Vec<&str> = q.iter(world).map(|n| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["session1"],
            "bootstrap session must be auto-named 'session1' (the first counter value)",
        );
    }

    #[test]
    fn bootstrap_attaches_subtree_pointer() {
        let _guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
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
