//! The `TmuxSessionPlugin`: connection state, projection, and the per-frame
//! event-drain + reconcile systems.

use crate::connection::TmuxConnection;
use crate::enumerate::{EnumerationState, client_name_command, list_windows_command};
use crate::event_pump::{advance_state, apply_events, drain_transport, take_client_name};
use crate::model::ProjectionModel;
use crate::output::{PaneOutput, collect_pane_outputs};
use crate::reconcile::{TmuxProjection, reconcile_projection};
use crate::state::ConnectionState;
use bevy::prelude::*;
use tmux_control::TransportEvent;

/// Present (inserted at plugin build) whenever the tmux backend is active, so
/// consumers can gate "tmux mode" from frame 0 — before any `%session-changed`.
#[derive(Resource, Default)]
pub struct TmuxPresence;

/// Wires the tmux integration into the Bevy app: connection state, the
/// projection model + index, the per-frame drain system, and the reconcile
/// system.
pub struct TmuxSessionPlugin;

/// Ordering label for the tmux drain + reconcile chain. The binary's render
/// systems run `.after(TmuxProjectionSet)` so a freshly-projected pane is
/// attached and its output routed in the same frame the projection spawns it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TmuxProjectionSet;

impl Plugin for TmuxSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectionState>()
            .init_resource::<ProjectionModel>()
            .init_resource::<TmuxProjection>()
            .init_resource::<EnumerationState>()
            .insert_resource(TmuxPresence)
            .insert_non_send_resource(TmuxConnection::default())
            .add_message::<PaneOutput>()
            .add_systems(
                Update,
                (
                    drain_tmux_events,
                    reconcile_projection.run_if(resource_exists_and_changed::<ProjectionModel>),
                )
                    .chain()
                    .in_set(TmuxProjectionSet),
            );
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
    mut pane_output: MessageWriter<PaneOutput>,
) {
    let events = match connection.client() {
        Some(client) => drain_transport(client.events()),
        None => return,
    };
    if events.is_empty() {
        return;
    }
    for output in collect_pane_outputs(&events) {
        pane_output.write(output);
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
            match client.handle().send(&client_name_command()) {
                Ok(id) => enumeration.client_name_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send client-name query"),
            }
        }
    }
    if events
        .iter()
        .any(|event| matches!(event, TransportEvent::Closed { .. }))
    {
        connection.take();
        enumeration.pending = None;
        enumeration.client_name_pending = None;
        *model.bypass_change_detection() = ProjectionModel::default();
        model.set_changed();
    } else {
        if let Some(name) = take_client_name(&mut enumeration.client_name_pending, &events) {
            connection.set_client_name(name);
        }
        if apply_events(
            model.bypass_change_detection(),
            &mut enumeration.pending,
            &events,
        ) {
            model.set_changed();
        }
    }
    // NOTE: runs after the Closed branch took the connection, so `client()` is
    // None there and this is a no-op — safe to re-arm only while still attached.
    if matches!(*state, ConnectionState::Attached)
        && connection.client_name().is_none()
        && enumeration.client_name_pending.is_none()
        && let Some(client) = connection.client()
    {
        match client.handle().send(&client_name_command()) {
            Ok(id) => enumeration.client_name_pending = Some(id),
            Err(error) => tracing::warn!(?error, "failed to re-send client-name query"),
        }
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
