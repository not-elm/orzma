//! The `TmuxSessionPlugin`: connection state, projection, and the per-frame
//! event-drain + reconcile systems.

use crate::connection::TmuxConnection;
use crate::enumerate::{EnumerationState, list_windows_command};
use crate::event_pump::{advance_state, apply_events, drain_transport};
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
        app.init_resource::<EnumerationState>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, (drain_tmux_events, reconcile_projection).chain());
    }
}

/// Drains the live connection's transport events each frame: advances
/// `ConnectionState`, sends the `list-windows` enumeration once on attach, and
/// applies the batch (notifications + the enumeration reply, in stream order)
/// to the `ProjectionModel`. On `Closed` it reclaims the dead client and tears
/// the projection down so its entities do not linger.
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
    if events
        .iter()
        .any(|event| matches!(event, TransportEvent::Closed { .. }))
    {
        connection.take();
        enumeration.pending = None;
        *model.bypass_change_detection() = ProjectionModel::default();
        model.set_changed();
    } else if apply_events(
        model.bypass_change_detection(),
        &mut enumeration.pending,
        &events,
    ) {
        model.set_changed();
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
        assert!(app.world().resource::<EnumerationState>().pending.is_none());
    }
}
