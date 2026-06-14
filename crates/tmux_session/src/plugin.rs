//! The `TmuxSessionPlugin`: connection state, projection, and the per-frame
//! event-drain + reconcile systems.

use crate::connection::TmuxConnection;
use crate::event_pump::{advance_state, drain_transport, route_to_model};
use crate::model::ProjectionModel;
use crate::reconcile::{TmuxProjection, reconcile_projection};
use crate::state::ConnectionState;
use bevy::prelude::*;
use tmux_control::TransportEvent;

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
///
/// State and model are mutated through `bypass_change_detection` and marked
/// changed only when something actually changed, so a pane flooding
/// `%output` does not force a reconcile pass every frame. A `Closed` event
/// reclaims the dead client so the connection slot is freed for reconnect.
fn drain_tmux_events(
    mut state: ResMut<ConnectionState>,
    mut model: ResMut<ProjectionModel>,
    mut connection: NonSendMut<TmuxConnection>,
) {
    let events = match connection.client() {
        Some(client) => drain_transport(client.events()),
        None => return,
    };
    if events.is_empty() {
        return;
    }
    if advance_state(state.bypass_change_detection(), &events) {
        state.set_changed();
    }
    if route_to_model(model.bypass_change_detection(), &events) {
        model.set_changed();
    }
    if events
        .iter()
        .any(|event| matches!(event, TransportEvent::Closed { .. }))
    {
        connection.take();
    }
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
        assert!(app.world().resource::<ProjectionModel>().windows.is_empty());
    }
}
