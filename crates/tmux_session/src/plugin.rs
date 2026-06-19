//! The `TmuxSessionPlugin`: connection state, the projection observers, and the
//! per-frame transport-drain system that triggers the projection events.

use crate::components::{TmuxPane, TmuxSession};
use crate::connection::TmuxConnection;
use crate::copy_queries::{CopyModeQueries, CopyModeReply, drain_copy_replies};
use crate::enumerate::{
    EnumerationState, active_pane_command, aggressive_resize_command, capture_pane_command,
    client_name_command, cursor_query_command, list_windows_command, mode_keys_command,
    subscribe_window_flags_command, version_command, version_supports_per_window_refresh,
};
use crate::event_pump::{
    advance_state, detect_session_switch, detect_window_added, detect_window_switch,
    drain_transport, take_active_pane, take_aggressive_resize, take_client_name,
    take_cursor_positions, take_keybindings, take_mode_keys, take_pane_captures, take_prefix_keys,
    take_version, trigger_events,
};
use crate::events::{
    TmuxActivePaneChanged, TmuxConnectionClosed, TmuxConnectionReset, TmuxWindowsRetained,
};
use crate::keybindings::{KeyBindings, list_keys_command, prefix_options_command};
use crate::observers::{TmuxProjection, register_observers};
use crate::output::{PaneOutput, collect_pane_outputs};
use crate::state::ConnectionState;
use bevy::prelude::*;
use tmux_control::TransportEvent;

/// Marker resource inserted when the tmux backend is active. Drain systems are
/// gated on its presence; insert it to activate tmux mode, remove it to idle.
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
            .init_resource::<CopyModeQueries>()
            .insert_non_send_resource(TmuxConnection::default())
            .add_message::<PaneOutput>()
            .add_message::<CopyModeReply>()
            .add_systems(
                Update,
                drain_tmux_events
                    .in_set(TmuxProjectionSet)
                    .run_if(resource_exists::<TmuxPresence>),
            )
            .add_systems(
                Update,
                request_pane_captures
                    .after(TmuxProjectionSet)
                    .run_if(resource_exists::<TmuxPresence>),
            );
    }
}

/// Sends `capture-pane` and a companion cursor-position `display-message` once
/// for each newly-projected pane so its current screen seeds the first paint
/// with the cursor at the correct position. tmux `-CC` does not replay existing
/// content on attach (it only streams new `%output`), so without this a
/// quiescent pane stays blank until its program writes again. Gated on
/// `Added<TmuxPane>` — runs once per pane. Replies are consumed by
/// [`take_pane_captures`] / [`take_cursor_positions`] and routed as `PaneOutput`.
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
            Ok(cap_id) => {
                enumeration.capture_pending.insert(cap_id, pane.id);
                match client.handle().send(&cursor_query_command(pane.id)) {
                    Ok(cur_id) => {
                        enumeration.cursor_pending.insert(cur_id, pane.id);
                        enumeration.panes_with_cursor_pending.insert(pane.id);
                    }
                    Err(error) => {
                        tracing::warn!(?error, pane = pane.id.0, "failed to send cursor query");
                    }
                }
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
    mut copy_queries: ResMut<CopyModeQueries>,
    mut connection: NonSendMut<TmuxConnection>,
    mut pane_output: MessageWriter<PaneOutput>,
    mut copy_replies: MessageWriter<CopyModeReply>,
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
    if detect_session_switch(&events, current_session, connection.client_name()).is_some()
        && let Some(client) = connection.client()
    {
        commands.trigger(TmuxWindowsRetained {
            windows: Vec::new(),
        });
        enumeration.capture_pending.clear();
        enumeration.cursor_pending.clear();
        enumeration.panes_with_cursor_pending.clear();
        enumeration.capture_awaiting_cursor.clear();
        // NOTE: aggressive-resize is a per-window option, so the switched-to
        // session must be re-checked — clear the one-shot guard or its `on`
        // setting would go undetected after a switch.
        enumeration.aggressive_resize_checked = false;
        enumeration.aggressive_resize_pending = None;
        send_session_enumeration(&mut enumeration, client);
    } else if let Some(client) = connection.client() {
        if detect_window_added(&events) {
            match client.handle().send(&list_windows_command()) {
                Ok(id) => enumeration.pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to re-enumerate on window-add"),
            }
        }
        if detect_window_switch(&events, current_session) {
            match client.handle().send(&active_pane_command()) {
                Ok(id) => enumeration.active_pane_pending = Some(id),
                Err(error) => {
                    tracing::warn!(?error, "failed to re-query active pane on window switch")
                }
            }
        }
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
            match client.handle().send(&list_keys_command("copy-mode")) {
                Ok(id) => enumeration.keys_copy_mode_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send list-keys -T copy-mode"),
            }
            match client.handle().send(&list_keys_command("copy-mode-vi")) {
                Ok(id) => enumeration.keys_copy_mode_vi_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send list-keys -T copy-mode-vi"),
            }
            match client.handle().send(&mode_keys_command()) {
                Ok(id) => enumeration.mode_keys_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send mode-keys query"),
            }
            match client.handle().send(&version_command()) {
                Ok(id) => enumeration.version_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send version query"),
            }
        }
    }
    if events
        .iter()
        .any(|event| matches!(event, TransportEvent::Closed { .. }))
    {
        connection.take();
        commands.trigger(TmuxConnectionReset);
        commands.trigger(TmuxConnectionClosed);
    } else {
        if let Some(name) = take_client_name(&mut enumeration.client_name_pending, &events) {
            connection.set_client_name(name);
        }
        if let Some(version) = take_version(&mut enumeration.version_pending, &events) {
            connection.set_per_window_refresh(version_supports_per_window_refresh(&version));
        }
        if let Some((window, pane)) =
            take_active_pane(&mut enumeration.active_pane_pending, &events)
        {
            commands.trigger(TmuxActivePaneChanged {
                window,
                pane,
                from_notification: false,
            });
            if !enumeration.aggressive_resize_checked
                && enumeration.aggressive_resize_pending.is_none()
                && let Some(client) = connection.client()
            {
                match client.handle().send(&aggressive_resize_command(window)) {
                    Ok(id) => enumeration.aggressive_resize_pending = Some(id),
                    Err(error) => tracing::warn!(?error, "failed to query aggressive-resize"),
                }
            }
        }
        if let Some(value) =
            take_aggressive_resize(&mut enumeration.aggressive_resize_pending, &events)
        {
            enumeration.aggressive_resize_checked = true;
            if value.trim() == "on" {
                tracing::warn!(
                    "tmux 'aggressive-resize on' is incompatible with control-mode integration; \
                     windows may resize unexpectedly"
                );
            }
        }
        // NOTE: deref once so the borrow checker can see these as distinct
        // field borrows rather than overlapping borrows through DerefMut.
        // NOTE: take_pane_captures MUST run before take_cursor_positions: when
        // both a capture reply and its paired cursor reply arrive in the same
        // event batch, captures populates capture_awaiting_cursor first, then
        // cursor_positions drains it. Swapping the calls silently drops cursor
        // fixes on same-batch arrivals.
        let e = &mut *enumeration;
        for output in take_pane_captures(
            &mut e.capture_pending,
            &mut e.capture_awaiting_cursor,
            &e.panes_with_cursor_pending,
            &events,
        ) {
            pane_output.write(output);
        }
        for output in take_cursor_positions(
            &mut e.cursor_pending,
            &mut e.panes_with_cursor_pending,
            &mut e.capture_awaiting_cursor,
            &events,
        ) {
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
        if let Some(bindings) = take_keybindings(&mut enumeration.keys_copy_mode_pending, &events) {
            keybindings.install(bindings);
        }
        if let Some(bindings) =
            take_keybindings(&mut enumeration.keys_copy_mode_vi_pending, &events)
        {
            keybindings.install(bindings);
        }
        if let Some(mode_keys) = take_mode_keys(&mut enumeration.mode_keys_pending, &events) {
            keybindings.set_mode_keys(mode_keys);
        }
        for reply in drain_copy_replies(&mut copy_queries, &events) {
            copy_replies.write(reply);
        }
        trigger_events(
            &mut commands,
            &mut enumeration.pending,
            &events,
            connection.client_name(),
        );
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
    if let Err(error) = client.handle().send(&subscribe_window_flags_command()) {
        tracing::warn!(?error, "failed to subscribe to window flags");
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
            TmuxLayoutChanged, TmuxSessionChanged, TmuxWindowAdded, TmuxWindowsRetained,
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
            layout: WindowLayout::parse(b"abcd,80x24,0,0,1").unwrap(),
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
