//! The `TmuxSessionPlugin`: connection state, projection, and the per-frame
//! event-drain + reconcile systems.

use crate::connection::TmuxConnection;
use crate::event_pump::{advance_state, drain_transport, route_to_model};
use crate::model::ProjectionModel;
use crate::reconcile::{TmuxProjection, reconcile_projection};
use crate::state::ConnectionState;
use bevy::prelude::*;

/// Wires the tmux integration into the Bevy app: connection state, the
/// projection model + index, the per-frame drain system, and the reconcile
/// system. Phase 1b does not auto-connect.
pub struct TmuxSessionPlugin;

impl Plugin for TmuxSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectionState>();
        app.init_resource::<ProjectionModel>();
        app.init_resource::<TmuxProjection>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, (drain_tmux_events, reconcile_projection).chain());
    }
}

/// Drains the live connection's transport events each frame, advancing
/// `ConnectionState` and routing notifications into the `ProjectionModel`.
fn drain_tmux_events(
    mut state: ResMut<ConnectionState>,
    mut model: ResMut<ProjectionModel>,
    connection: NonSend<TmuxConnection>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    let events = drain_transport(client.events());
    if events.is_empty() {
        return;
    }
    advance_state(&mut state, &events);
    route_to_model(&mut model, &events);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_resources_and_stays_idle_without_connection() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.update();
        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Idle
        );
        assert!(
            app.world()
                .resource::<ProjectionModel>()
                .windows
                .is_empty()
        );
    }
}
