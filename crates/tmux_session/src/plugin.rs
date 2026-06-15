//! The `TmuxSessionPlugin`: connection state, the projection observers, and the
//! per-frame transport-drain system that triggers the projection events.

use crate::components::TmuxPane;
use crate::connection::TmuxConnection;
use crate::enumerate::{
    EnumerationState, active_pane_command, capture_pane_command, client_name_command,
    list_windows_command,
};
use crate::event_pump::{
    advance_state, drain_transport, take_active_pane, take_client_name, take_pane_captures,
    trigger_events,
};
use crate::events::{TmuxActivePaneChanged, TmuxConnectionReset};
use crate::observers::{TmuxProjection, register_observers};
use crate::output::{PaneOutput, collect_pane_outputs};
use crate::state::ConnectionState;
use bevy::prelude::*;
use tmux_control::TransportEvent;

/// Present (inserted at plugin build) whenever the tmux backend is active, so
/// consumers can gate "tmux mode" from frame 0 — before any `%session-changed`.
#[derive(Resource, Default)]
pub struct TmuxPresence;

/// Wires the tmux integration into the Bevy app: connection state, the
/// projection observers + id->entity index, and the per-frame transport-drain
/// system that triggers the global projection events.
pub struct TmuxSessionPlugin;

/// Ordering label for the tmux drain system. The binary's render systems run
/// `.after(TmuxProjectionSet)` so a freshly-projected pane is attached and its
/// output routed in the same frame the projection spawns it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TmuxProjectionSet;

impl Plugin for TmuxSessionPlugin {
    fn build(&self, app: &mut App) {
        register_observers(app);
        app.init_resource::<ConnectionState>()
            .init_resource::<TmuxProjection>()
            .init_resource::<EnumerationState>()
            .insert_resource(TmuxPresence)
            .insert_non_send_resource(TmuxConnection::default())
            .add_message::<PaneOutput>()
            .add_systems(Update, drain_tmux_events.in_set(TmuxProjectionSet))
            .add_systems(Update, request_pane_captures.after(TmuxProjectionSet));
    }
}

/// Sends `capture-pane` once for each newly-projected pane so its current screen
/// seeds the first paint. tmux `-CC` does not replay existing content on attach
/// (it only streams new `%output`), so without this a quiescent pane stays blank
/// until its program writes again. Gated on `Added<TmuxPane>` — runs once per
/// pane. The reply is consumed by [`take_pane_captures`] and routed as
/// `PaneOutput`.
fn request_pane_captures(
    mut enumeration: ResMut<EnumerationState>,
    connection: NonSend<TmuxConnection>,
    new_panes: Query<&TmuxPane, Added<TmuxPane>>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    for pane in new_panes.iter() {
        match client.handle().send(&capture_pane_command(pane.id)) {
            Ok(id) => {
                enumeration.capture_pending.insert(id, pane.id);
            }
            Err(error) => {
                tracing::warn!(?error, pane = pane.id.0, "failed to send capture-pane")
            }
        }
    }
}

/// Drains the live connection's transport events each frame: advances
/// `ConnectionState`, sends the `list-windows` enumeration once on attach, and
/// triggers the global projection events (notifications + the enumeration
/// reply, in stream order) the observers consume. On `Closed` it reclaims the
/// dead client and triggers `TmuxConnectionReset` so the projected entities do
/// not linger.
///
/// `ConnectionState` is written back only when [`advance_state`] reports a real
/// transition, so normal change detection fires it exactly once per transition —
/// an output flood does not force the
/// `resource_exists_and_changed::<ConnectionState>`-gated consumers to re-run
/// every frame.
fn drain_tmux_events(
    mut commands: Commands,
    mut state: ResMut<ConnectionState>,
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
    if let Some(next) = advance_state(&state, &events) {
        *state = next;
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
            match client.handle().send(&active_pane_command()) {
                Ok(id) => enumeration.active_pane_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send active-pane query"),
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
        enumeration.active_pane_pending = None;
        enumeration.capture_pending.clear();
        commands.trigger(TmuxConnectionReset);
    } else {
        if let Some(name) = take_client_name(&mut enumeration.client_name_pending, &events) {
            connection.set_client_name(name);
        }
        if let Some((window, pane)) =
            take_active_pane(&mut enumeration.active_pane_pending, &events)
        {
            commands.trigger(TmuxActivePaneChanged { window, pane });
        }
        for output in take_pane_captures(&mut enumeration.capture_pending, &events) {
            pane_output.write(output);
        }
        trigger_events(&mut commands, &mut enumeration.pending, &events);
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
        assert!(app.world().resource::<EnumerationState>().pending.is_none());
        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
        assert!(index.session.is_none());
    }
}
