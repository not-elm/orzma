//! The `TmuxSessionPlugin`: connection state, the projection observers, and the
//! per-frame transport-drain system that triggers the projection events.

use crate::components::{TmuxPane, TmuxSession};
use crate::connection::TmuxConnection;
use crate::enumerate::{
    EnumerationState, active_pane_command, capture_pane_command, client_name_command,
    list_windows_command,
};
use crate::event_pump::{
    advance_state, detect_session_switch, drain_transport, take_active_pane, take_client_name,
    take_keybindings, take_pane_captures, take_prefix_keys, trigger_events,
};
use crate::events::{TmuxActivePaneChanged, TmuxConnectionReset, TmuxWindowsRetained};
use crate::keybindings::{KeyBindings, list_keys_command, prefix_options_command};
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
            .init_resource::<KeyBindings>()
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
    mut keybindings: ResMut<KeyBindings>,
    mut connection: NonSendMut<TmuxConnection>,
    mut pane_output: MessageWriter<PaneOutput>,
    index: Res<TmuxProjection>,
    sessions: Query<&TmuxSession>,
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
    let current_session = index
        .session
        .and_then(|e| sessions.get(e).ok())
        .map(|s| s.id);
    if detect_session_switch(&events, current_session).is_some()
        && let Some(client) = connection.client()
    {
        commands.trigger(TmuxWindowsRetained {
            windows: Vec::new(),
        });
        send_session_enumeration(&mut enumeration, client);
    }
    if let Some(next) = advance_state(&state, &events) {
        *state = next;
        if matches!(*state, ConnectionState::Attached)
            && let Some(client) = connection.client()
        {
            send_session_enumeration(&mut enumeration, client);
            match client.handle().send(&client_name_command()) {
                Ok(id) => enumeration.client_name_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send client-name query"),
            }
            match client.handle().send(&list_keys_command("root")) {
                Ok(id) => enumeration.keys_root_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send list-keys -T root"),
            }
            match client.handle().send(&list_keys_command("prefix")) {
                Ok(id) => enumeration.keys_prefix_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send list-keys -T prefix"),
            }
            match client.handle().send(&prefix_options_command()) {
                Ok(id) => enumeration.prefix_keys_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send prefix query"),
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
        enumeration.keys_root_pending = None;
        enumeration.keys_prefix_pending = None;
        enumeration.prefix_keys_pending = None;
        keybindings.clear();
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
        if let Some(bindings) = take_keybindings(&mut enumeration.keys_root_pending, &events) {
            keybindings.install(bindings);
        }
        if let Some(bindings) = take_keybindings(&mut enumeration.keys_prefix_pending, &events) {
            keybindings.install(bindings);
        }
        if let Some(keys) = take_prefix_keys(&mut enumeration.prefix_keys_pending, &events) {
            keybindings.set_prefix_keys(keys);
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

/// Sends the per-session enumeration queries (`list-windows` + active-pane) that
/// rebuild the projection. Shared by the attach transition and a session switch so
/// the two paths cannot drift (a switched-to session would otherwise risk stale
/// windows or a missing active-pane marker).
fn send_session_enumeration(enumeration: &mut EnumerationState, client: &tmux_control::TmuxClient) {
    match client.handle().send(&list_windows_command()) {
        Ok(id) => enumeration.pending = Some(id),
        Err(error) => tracing::warn!(?error, "failed to send list-windows enumeration"),
    }
    match client.handle().send(&active_pane_command()) {
        Ok(id) => enumeration.active_pane_pending = Some(id),
        Err(error) => tracing::warn!(?error, "failed to send active-pane query"),
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

    #[test]
    fn empty_windows_retained_clears_windows_and_panes_keeps_session() {
        use crate::events::{
            TmuxLayoutChanged, TmuxSessionChanged, TmuxWindowAdded, TmuxWindowsRetained, pane_geoms,
        };
        use tmux_control_parser::{SessionId, WindowId, WindowLayout};

        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.world_mut().trigger(TmuxSessionChanged {
            session: SessionId(1),
            name: "a".into(),
        });
        app.world_mut().trigger(TmuxWindowAdded {
            window: WindowId(1),
            index: 0,
            name: "w".into(),
        });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            panes: pane_geoms(&WindowLayout::parse(b"abcd,80x24,0,0,1").unwrap()),
        });
        app.update();
        app.world_mut().trigger(TmuxWindowsRetained {
            windows: Vec::new(),
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
        assert!(index.session.is_some());
    }
}
