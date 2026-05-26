//! `OzmuxBootstrapPlugin` registers the Startup `bootstrap` system which
//! seeds the initial Session and spawns the corresponding Bevy entity
//! with `AttachedSession`.

use crate::multiplexer::{AttachedSession, Multiplexer, SessionEntityId};
use bevy::prelude::*;

/// Bevy Plugin that registers the `bootstrap` system in the `Startup`
/// schedule.
pub struct OzmuxBootstrapPlugin;

impl Plugin for OzmuxBootstrapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, bootstrap);
    }
}

pub(crate) fn bootstrap(mut commands: Commands, mut mux: ResMut<Multiplexer>) {
    let (sid, _pid, _aid) = mux.create_session(Some("default".into()));
    commands.spawn((
        SessionEntityId(sid),
        AttachedSession,
        Name::new("Session:default"),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_spawns_session_entity_with_attached_marker() {
        let _guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(crate::multiplexer::OzmuxMultiplexerPlugin)
            .add_plugins(crate::configs::OzmuxConfigsPlugin)
            .add_plugins(OzmuxBootstrapPlugin);

        app.update();

        let world = app.world_mut();
        let mut q = world.query::<(&SessionEntityId, &AttachedSession)>();
        let count = q.iter(world).count();
        assert_eq!(count, 1, "exactly one attached session entity");

        let mux = app.world().resource::<Multiplexer>();
        assert_eq!(mux.sessions.len(), 1);
        let session = mux.sessions.values().next().expect("session exists");
        assert_eq!(session.pane_ids().count(), 1);
        let pane = session
            .pane(&session.active_pane)
            .expect("active pane resolves");
        assert_eq!(pane.activity_ids().count(), 1);
    }
}
