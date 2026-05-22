//! `OzmuxBootstrapPlugin` registers the Startup `bootstrap` system which
//! seeds the initial Session/Window/Pane/Activity and attaches the per-
//! window components to the primary GUI window.

use crate::input::PrefixState;
use crate::multiplexer::{AttachedSession, Multiplexer};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// Bevy Plugin that registers the `bootstrap` system in the `Startup`
/// schedule. Idempotent across app builds: each new app gets a fresh
/// Session/Window/Pane/Activity tree and the primary window gets the
/// `AttachedSession` + `PrefixState` components.
pub struct OzmuxBootstrapPlugin;

impl Plugin for OzmuxBootstrapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, bootstrap);
    }
}

fn bootstrap(
    mut commands: Commands,
    mut mux: ResMut<Multiplexer>,
    primary: Single<Entity, With<PrimaryWindow>>,
) {
    let sid = mux.create_session(Some("default".into()));
    if let Err(err) = mux.create_window(Some(&sid), Some("main".into())) {
        tracing::error!(?err, "bootstrap: create_window failed");
        return;
    }
    commands
        .entity(*primary)
        .insert((AttachedSession(sid), PrefixState::default()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::{Window, WindowResolution};

    #[test]
    fn bootstrap_inserts_components_on_primary_window() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(crate::multiplexer::OzmuxMultiplexerPlugin)
            .add_plugins(OzmuxBootstrapPlugin);

        let primary = app
            .world_mut()
            .spawn((
                Window {
                    resolution: WindowResolution::new(800, 600),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();

        app.update();

        assert!(
            app.world().get::<AttachedSession>(primary).is_some(),
            "AttachedSession must be inserted on the primary window"
        );
        assert!(
            app.world().get::<PrefixState>(primary).is_some(),
            "PrefixState must be inserted on the primary window"
        );

        let mux = app.world().resource::<Multiplexer>();
        assert_eq!(mux.sessions.len(), 1);
        assert_eq!(mux.windows.len(), 1);
        let window = mux.windows.values().next().expect("window exists");
        assert_eq!(window.pane_ids().count(), 1);
        let pane = window
            .pane(&window.active_pane)
            .expect("active pane resolves");
        assert_eq!(pane.activity_ids().count(), 1);
    }
}
