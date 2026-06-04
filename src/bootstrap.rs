//! `OzmuxBootstrapPlugin` registers the Startup `bootstrap` system which
//! seeds the initial Workspace via `MultiplexerCommands` and attaches the
//! UI subtree pointer + `AttachedWorkspace` marker to the spawned Workspace
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
    let _ = mux.spawn_attached_workspace();
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
    use ozmux_multiplexer::{AttachedWorkspace, MultiplexerPlugin, WorkspaceMarker, WorkspaceUiSubtree};

    #[test]
    fn bootstrap_spawns_workspace_entity_with_attached_marker() {
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
        let mut q = world.query_filtered::<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>();
        assert_eq!(
            q.iter(world).count(),
            1,
            "exactly one attached workspace entity"
        );
    }

    #[test]
    fn bootstrap_names_the_initial_workspace_workspace1() {
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
        let mut q = world.query_filtered::<&Name, With<WorkspaceMarker>>();
        let names: Vec<&str> = q.iter(world).map(|n| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["workspace1"],
            "bootstrap workspace must be auto-named 'workspace1' (the first counter value)",
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
        let mut q = world.query::<(&WorkspaceMarker, &WorkspaceUiSubtree)>();
        let row = q.iter(world).next().expect("workspace has subtree pointer");
        assert!(world.get_entity(row.1.0).is_ok());
    }
}
