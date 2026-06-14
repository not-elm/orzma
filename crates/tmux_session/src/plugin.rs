//! The `TmuxSessionPlugin` and its per-frame event-drain system.

use crate::connection::TmuxConnection;
use crate::event_pump::drain_events;
use crate::state::ConnectionState;
use bevy::prelude::*;

/// Wires the tmux integration into the Bevy app: registers the
/// [`ConnectionState`] resource, the [`TmuxConnection`] `NonSend` resource,
/// and the per-frame drain system. Phase 0 does not auto-connect.
pub struct TmuxSessionPlugin;

impl Plugin for TmuxSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectionState>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, drain_tmux_events);
    }
}

/// Drains the live tmux connection's transport events each frame and
/// advances [`ConnectionState`]. A no-op while disconnected.
fn drain_tmux_events(mut state: ResMut<ConnectionState>, connection: NonSend<TmuxConnection>) {
    if let Some(client) = connection.client() {
        drain_events(&mut state, client.events());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_state_and_stays_idle_without_connection() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.update();
        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Idle
        );
    }
}
