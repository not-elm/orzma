//! The `TmuxSessionPlugin`: connection state, projection, and the per-frame
//! event-drain + reconcile systems.

use crate::connection::TmuxConnection;
use crate::enumerate::{EnumerationState, list_windows_command};
use crate::event_pump::{advance_state, drain_transport, route_to_model, seed_from_reply};
use crate::model::ProjectionModel;
use crate::reconcile::{TmuxProjection, reconcile_projection};
use crate::state::ConnectionState;
use bevy::prelude::*;
use tmux_control::{ClientEvent, TransportEvent};

/// Wires the tmux integration into the Bevy app: connection state, the
/// projection model + index, the per-frame drain system, and the reconcile
/// system. Phase 1b does not auto-connect.
pub struct TmuxSessionPlugin;

impl Plugin for TmuxSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectionState>();
        app.init_resource::<ProjectionModel>();
        app.init_resource::<TmuxProjection>();
        app.init_resource::<EnumerationState>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, (drain_tmux_events, reconcile_projection).chain());
    }
}

/// Drains the live connection's transport events each frame: advances
/// `ConnectionState`, sends the `list-windows` enumeration once on attach and
/// seeds the `ProjectionModel` from its reply, routes notifications into the
/// model, and reclaims the dead client on `Closed`.
///
/// State and model are mutated through `bypass_change_detection` and marked
/// changed only on real change, so an output flood does not force a reconcile
/// every frame.
fn drain_tmux_events(
    mut state: ResMut<ConnectionState>,
    mut model: ResMut<ProjectionModel>,
    mut enumeration: ResMut<EnumerationState>,
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
        if matches!(*state, ConnectionState::Attached)
            && let Some(client) = connection.client()
        {
            match client.handle().send(&list_windows_command()) {
                Ok(id) => enumeration.pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send list-windows enumeration"),
            }
        }
    }
    let mut model_changed = route_to_model(model.bypass_change_detection(), &events);
    if let Some(pending) = enumeration.pending {
        for event in &events {
            if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
                && *id == pending
            {
                enumeration.pending = None;
                if *ok {
                    model_changed |= seed_from_reply(model.bypass_change_detection(), output);
                } else {
                    tracing::warn!("list-windows enumeration command failed");
                }
                break;
            }
        }
    }
    if model_changed {
        model.set_changed();
    }
    if events
        .iter()
        .any(|event| matches!(event, TransportEvent::Closed { .. }))
    {
        connection.take();
        enumeration.pending = None;
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
        assert!(
            app.world()
                .resource::<EnumerationState>()
                .pending
                .is_none()
        );
    }
}
