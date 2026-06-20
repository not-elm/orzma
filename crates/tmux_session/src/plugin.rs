//! The `TmuxSessionPlugin`: connection state, the projection observers, and the
//! per-frame transport-drain system that triggers the projection events.

use crate::components::{TmuxPane, TmuxSession};
use crate::connection::TmuxConnection;
use crate::copy_queries::{CopyModeQueries, CopyModeReply, drain_copy_replies};
use crate::enumerate::{
    EnumerationState, PendingReply, active_pane_command, aggressive_resize_command,
    capture_pane_command, client_name_command, cursor_query_command, list_windows_command,
    mode_keys_command, subscribe_window_flags_command, version_command,
    version_supports_per_window_refresh,
};
use crate::event_pump::{
    advance_state, capture_to_bytes, capture_to_bytes_with_cursor, detect_session_switch,
    detect_window_added, detect_window_switch, drain_transport, first_reply_line,
    parse_active_pane, parse_cursor_pos, trigger_notification, trigger_seed,
};
use crate::events::{
    TmuxActivePaneChanged, TmuxConnectionClosed, TmuxConnectionReset, TmuxWindowsRetained,
};
use crate::keybindings::{
    KeyBindings, ModeKeys, list_keys_command, parse_list_keys, parse_prefix, prefix_options_command,
};
use crate::observers::{TmuxProjection, register_observers};
use crate::output::{PaneOutput, collect_pane_outputs};
use crate::state::ConnectionState;
use bevy::prelude::*;
use tmux_control::{ClientEvent, TmuxClient, TransportEvent};

/// Marker resource inserted when the tmux backend is active. Drain systems are
/// gated on its presence; insert it to activate tmux mode, remove it to idle.
#[derive(Resource, Default)]
pub struct TmuxPresence;

/// Emitted the frame the control client's transport transitions to `Attached`
/// (including a reconnect). Gates [`send_attach_enumeration`]. A pure signal —
/// the init-send system reads the live client from `TmuxConnection`.
#[derive(Message)]
struct TmuxClientAttached;

/// This frame's drained transport events, shared across the drain chain.
/// Refreshed by [`drain_tmux_transport`] when the drain or the prior batch is
/// non-empty; read-only downstream.
#[derive(Resource, Default)]
struct TmuxEventBatch(Vec<TransportEvent>);

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
            .init_resource::<TmuxEventBatch>()
            .insert_non_send_resource(TmuxConnection::default())
            .add_message::<PaneOutput>()
            .add_message::<CopyModeReply>()
            .add_message::<TmuxClientAttached>()
            .add_systems(
                Update,
                (
                    drain_tmux_transport,
                    advance_tmux_connection.run_if(tmux_batch_pending),
                    send_attach_enumeration.run_if(on_message::<TmuxClientAttached>),
                    send_tmux_reenumeration.run_if(tmux_batch_pending),
                    apply_tmux_replies.run_if(tmux_batch_pending),
                )
                    .chain()
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
/// `Added<TmuxPane>` — runs once per pane. The capture/cursor replies are
/// consumed by [`apply_reply`]'s `Capture`/`Cursor` arms and routed as
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
            Ok(cap_id) => {
                enumeration
                    .pending
                    .insert(cap_id, PendingReply::Capture { pane: pane.id });
                match client.handle().send(&cursor_query_command(pane.id)) {
                    Ok(cur_id) => {
                        enumeration
                            .pending
                            .insert(cur_id, PendingReply::Cursor { pane: pane.id });
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

fn tmux_batch_pending(batch: Res<TmuxEventBatch>) -> bool {
    !batch.0.is_empty()
}

/// Folds the batch through `advance_state`, writes `ConnectionState` only on a
/// real transition (so change detection fires once per transition), emits
/// `TmuxClientAttached` on the attach edge, and on `Closed` reclaims the dead
/// client and triggers the projection teardown.
fn advance_tmux_connection(
    mut commands: Commands,
    mut state: ResMut<ConnectionState>,
    mut connection: NonSendMut<TmuxConnection>,
    mut attached: MessageWriter<TmuxClientAttached>,
    batch: Res<TmuxEventBatch>,
) {
    if let Some(next) = advance_state(&state, &batch.0) {
        let is_attached = matches!(next, ConnectionState::Attached);
        *state = next;
        if is_attached {
            attached.write(TmuxClientAttached);
        }
    }
    if batch
        .0
        .iter()
        .any(|event| matches!(event, TransportEvent::Closed { .. }))
    {
        connection.take();
        commands.trigger(TmuxConnectionReset);
        commands.trigger(TmuxConnectionClosed);
    }
}

/// Sends the one-time initial query suite when the client attaches:
/// `list-windows`, active-pane, window-flags subscription, client name, the four
/// `list-keys` tables, prefix options, mode-keys, and version. Gated by
/// `on_message::<TmuxClientAttached>` so it runs exactly once per attach edge.
fn send_attach_enumeration(
    mut enumeration: ResMut<EnumerationState>,
    connection: NonSend<TmuxConnection>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    send_session_enumeration(&mut enumeration, client);
    enumeration.register(
        client.handle().send(&client_name_command()),
        PendingReply::ClientName,
    );
    enumeration.register(
        client.handle().send(&list_keys_command("root")),
        PendingReply::KeyBindings,
    );
    enumeration.register(
        client.handle().send(&list_keys_command("prefix")),
        PendingReply::KeyBindings,
    );
    enumeration.register(
        client.handle().send(&prefix_options_command()),
        PendingReply::PrefixKeys,
    );
    enumeration.register(
        client.handle().send(&list_keys_command("copy-mode")),
        PendingReply::KeyBindings,
    );
    enumeration.register(
        client.handle().send(&list_keys_command("copy-mode-vi")),
        PendingReply::KeyBindings,
    );
    enumeration.register(
        client.handle().send(&mode_keys_command()),
        PendingReply::ModeKeys,
    );
    enumeration.register(
        client.handle().send(&version_command()),
        PendingReply::Version,
    );
}

/// Re-enumerates topology when the batch contains a session-switch, window-add,
/// or window-switch notification; re-arms the client-name query if the name has
/// not yet been learned after attach.
///
/// Body-guards on the live client (see [`apply_tmux_replies`]).
fn send_tmux_reenumeration(
    mut commands: Commands,
    mut enumeration: ResMut<EnumerationState>,
    connection: NonSend<TmuxConnection>,
    state: Res<ConnectionState>,
    index: Res<TmuxProjection>,
    sessions: Query<&TmuxSession>,
    batch: Res<TmuxEventBatch>,
) {
    // NOTE: connection liveness is a body guard, not a run_if — a run condition
    // reading NonSend<TmuxConnection> is unsound (bevyengine/bevy#21230).
    if connection.client().is_none() {
        return;
    }
    let events = &batch.0;
    let current_session = index
        .session
        .and_then(|e| sessions.get(e).ok())
        .map(|s| s.id);
    if detect_session_switch(events, current_session, connection.client_name()).is_some()
        && let Some(client) = connection.client()
    {
        commands.trigger(TmuxWindowsRetained {
            windows: Vec::new(),
        });
        // NOTE: aggressive-resize is a per-window option, so the switched-to
        // session must be re-checked; clear_for_session_switch resets the
        // one-shot guard along with the now-stale enumeration/capture ids.
        enumeration.clear_for_session_switch();
        send_session_enumeration(&mut enumeration, client);
    } else if let Some(client) = connection.client() {
        if detect_window_added(events) {
            enumeration.register(
                client.handle().send(&list_windows_command()),
                PendingReply::ListWindows,
            );
        }
        if detect_window_switch(events, current_session) {
            enumeration.register(
                client.handle().send(&active_pane_command()),
                PendingReply::ActivePane,
            );
        }
    }
    if matches!(*state, ConnectionState::Attached)
        && connection.client_name().is_none()
        && !enumeration.has_pending(PendingReply::ClientName)
        && let Some(client) = connection.client()
    {
        enumeration.register(
            client.handle().send(&client_name_command()),
            PendingReply::ClientName,
        );
    }
}

/// Drains the live connection's transport channel into [`TmuxEventBatch`] and
/// routes `%output` to `PaneOutput`. Skips the write on a fully-idle frame so
/// change detection fires only when the batch's contents actually change; still
/// clears a previously-non-empty batch to empty exactly once.
fn drain_tmux_transport(
    mut batch: ResMut<TmuxEventBatch>,
    mut pane_output: MessageWriter<PaneOutput>,
    connection: NonSend<TmuxConnection>,
) {
    let drained = match connection.client() {
        Some(client) => drain_transport(client.events()),
        None => Vec::new(),
    };
    if drained.is_empty() && batch.0.is_empty() {
        return;
    }
    for output in collect_pane_outputs(&drained) {
        pane_output.write(output);
    }
    batch.0 = drained;
}

/// Applies this frame's command replies and notifications to the world: drains
/// each reply to what it answers, runs the active-pane→aggressive-resize
/// follow-up, surfaces copy-mode replies, and triggers the projection events the
/// observers consume.
///
/// Body-guards on the live client (see [`send_tmux_reenumeration`]).
fn apply_tmux_replies(
    mut commands: Commands,
    mut enumeration: ResMut<EnumerationState>,
    mut keybindings: ResMut<KeyBindings>,
    mut copy_queries: ResMut<CopyModeQueries>,
    mut connection: NonSendMut<TmuxConnection>,
    mut pane_output: MessageWriter<PaneOutput>,
    mut copy_replies: MessageWriter<CopyModeReply>,
    batch: Res<TmuxEventBatch>,
) {
    // NOTE: connection liveness is a body guard, not a run_if — a run condition
    // reading NonSend<TmuxConnection> is unsound (bevyengine/bevy#21230).
    if connection.client().is_none() {
        return;
    }
    let connection = &mut *connection;
    let events = &batch.0;
    // NOTE: this MUST stay a single in-order pass. tmux CC-mode replies are
    // FIFO, and capture is sent before its paired cursor query, so the Capture
    // arm fills capture_awaiting_cursor before the Cursor arm drains it.
    // Splitting into two passes silently drops cursor fixes on same-batch
    // arrivals.
    for event in events {
        match event {
            TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) => {
                let Some(reply) = enumeration.pending.remove(id) else {
                    continue;
                };
                apply_reply(
                    &mut commands,
                    &mut enumeration,
                    &mut keybindings,
                    &mut pane_output,
                    connection,
                    reply,
                    *ok,
                    output,
                );
            }
            TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
                trigger_notification(&mut commands, connection.client_name(), notification);
            }
            TransportEvent::Closed { .. } => {}
        }
    }
    for reply in drain_copy_replies(&mut copy_queries, events) {
        copy_replies.write(reply);
    }
}

/// Routes one completed command's reply to the world state it answers,
/// reproducing the per-kind handler logic the old `take_*` wrappers held.
#[expect(
    clippy::too_many_arguments,
    reason = "single apply seam over many reply kinds"
)]
fn apply_reply(
    commands: &mut Commands,
    enumeration: &mut EnumerationState,
    keybindings: &mut KeyBindings,
    pane_output: &mut MessageWriter<PaneOutput>,
    connection: &mut TmuxConnection,
    reply: PendingReply,
    ok: bool,
    output: &[String],
) {
    match reply {
        PendingReply::ListWindows if ok => trigger_seed(commands, output),
        PendingReply::ListWindows => tracing::warn!("list-windows enumeration command failed"),
        PendingReply::ClientName => {
            if let Some(name) = first_reply_line(ok, output, "client-name") {
                connection.set_client_name(name);
            }
        }
        PendingReply::Version => {
            if let Some(version) = first_reply_line(ok, output, "version") {
                connection.set_per_window_refresh(version_supports_per_window_refresh(&version));
            }
        }
        PendingReply::ActivePane if ok => {
            let Some((window, pane)) = output.iter().find_map(|line| parse_active_pane(line))
            else {
                return;
            };
            commands.trigger(TmuxActivePaneChanged {
                window,
                pane,
                from_notification: false,
            });
            if !enumeration.aggressive_resize_checked
                && !enumeration.has_pending(PendingReply::AggressiveResize)
                && let Some(client) = connection.client()
            {
                enumeration.register(
                    client.handle().send(&aggressive_resize_command(window)),
                    PendingReply::AggressiveResize,
                );
            }
        }
        PendingReply::ActivePane => tracing::warn!("active-pane query command failed"),
        PendingReply::KeyBindings if ok => keybindings.install(parse_list_keys(output)),
        PendingReply::KeyBindings => tracing::warn!("list-keys command failed"),
        PendingReply::PrefixKeys if ok => {
            keybindings.set_prefix_keys(
                output
                    .first()
                    .map(|line| parse_prefix(line))
                    .unwrap_or_default(),
            );
        }
        PendingReply::PrefixKeys => tracing::warn!("prefix query command failed"),
        PendingReply::ModeKeys if ok => {
            keybindings.set_mode_keys(
                output
                    .first()
                    .map(|line| ModeKeys::parse(line))
                    .unwrap_or_default(),
            );
        }
        PendingReply::ModeKeys => tracing::warn!("mode-keys query failed"),
        PendingReply::AggressiveResize => {
            // NOTE: only the successful reply marks the one-shot check done; a
            // failed query leaves aggressive_resize_checked false so the next
            // active-pane reply re-issues it (matches the old take_* behavior).
            if let Some(value) = first_reply_line(ok, output, "aggressive-resize") {
                enumeration.aggressive_resize_checked = true;
                if value.trim() == "on" {
                    tracing::warn!(
                        "tmux 'aggressive-resize on' is incompatible with control-mode \
                         integration; windows may resize unexpectedly"
                    );
                }
            }
        }
        PendingReply::Capture { pane } if ok => {
            if enumeration.panes_with_cursor_pending.contains(&pane) {
                enumeration
                    .capture_awaiting_cursor
                    .insert(pane, output.to_vec());
            } else {
                pane_output.write(PaneOutput {
                    pane,
                    data: capture_to_bytes(output),
                });
            }
        }
        PendingReply::Capture { pane } => {
            tracing::warn!(pane = pane.0, "capture-pane command failed");
        }
        PendingReply::Cursor { pane } => {
            enumeration.panes_with_cursor_pending.remove(&pane);
            let Some(lines) = enumeration.capture_awaiting_cursor.remove(&pane) else {
                return;
            };
            let (cx, cy) = if ok {
                parse_cursor_pos(output).unwrap_or((0, 0))
            } else {
                tracing::warn!(pane = pane.0, "cursor-position query failed");
                (0, 0)
            };
            pane_output.write(PaneOutput {
                pane,
                data: capture_to_bytes_with_cursor(&lines, cx, cy),
            });
        }
    }
}

/// Sends the per-session enumeration queries (`list-windows` + active-pane) that
/// rebuild the projection. Shared by the attach transition and a session switch so
/// the two paths cannot drift (a switched-to session would otherwise risk stale
/// windows or a missing active-pane marker).
fn send_session_enumeration(enumeration: &mut EnumerationState, client: &TmuxClient) {
    enumeration.register(
        client.handle().send(&list_windows_command()),
        PendingReply::ListWindows,
    );
    enumeration.register(
        client.handle().send(&active_pane_command()),
        PendingReply::ActivePane,
    );
    if let Err(error) = client.handle().send(&subscribe_window_flags_command()) {
        tracing::warn!(?error, "failed to subscribe to window flags");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_transport_clears_stale_batch_once_then_skips_idle() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.insert_resource(TmuxPresence);
        app.insert_resource(TmuxEventBatch(vec![TransportEvent::Closed {
            reason: "x".into(),
        }]));
        app.update();
        assert!(app.world().resource::<TmuxEventBatch>().0.is_empty());
        let changed_tick = app.world().resource_ref::<TmuxEventBatch>().last_changed();
        app.update();
        assert_eq!(
            app.world().resource_ref::<TmuxEventBatch>().last_changed(),
            changed_tick,
            "idle frame must not re-fire change detection on an already-empty batch"
        );
    }

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
                .resource::<EnumerationState>()
                .pending
                .is_empty()
        );
        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
        assert!(index.session.is_none());
    }

    #[test]
    fn advance_to_attached_emits_client_attached_message() {
        use bevy::ecs::system::RunSystemOnce;
        use tmux_control::{ClientEvent, ControlEvent};
        use tmux_control_parser::WindowId;
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        *app.world_mut().resource_mut::<ConnectionState>() = ConnectionState::Connecting;
        app.world_mut()
            .resource_mut::<TmuxEventBatch>()
            .0
            .push(TransportEvent::Protocol(ClientEvent::Notification(
                ControlEvent::WindowAdd {
                    window: WindowId(1),
                },
            )));
        app.world_mut()
            .run_system_once(advance_tmux_connection)
            .unwrap();
        let messages = app.world().resource::<Messages<TmuxClientAttached>>();
        assert_eq!(messages.iter_current_update_messages().count(), 1);
        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Attached
        );
    }

    #[test]
    fn send_attach_enumeration_runs_on_message() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.insert_resource(TmuxPresence);
        app.world_mut().write_message(TmuxClientAttached);
        app.update();
        assert!(
            app.world()
                .resource::<EnumerationState>()
                .pending
                .is_empty()
        );
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

    #[test]
    fn apply_and_reenumeration_skip_without_client() {
        use bevy::ecs::system::RunSystemOnce;
        use tmux_control::{ClientEvent, ControlEvent};
        use tmux_control_parser::WindowId;
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        // Non-empty batch but no live client: both systems must body-guard out.
        app.insert_resource(TmuxEventBatch(vec![TransportEvent::Protocol(
            ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(9),
            }),
        )]));
        app.world_mut()
            .run_system_once(send_tmux_reenumeration)
            .unwrap();
        app.world_mut().run_system_once(apply_tmux_replies).unwrap();
        // No panic, and no enumeration was registered (nothing was sent).
        assert!(
            app.world()
                .resource::<EnumerationState>()
                .pending
                .is_empty()
        );
    }

    #[test]
    fn apply_reply_client_name_sets_connection_and_seeds_windows() {
        use crate::events::TmuxWindowAdded;
        use bevy::ecs::system::SystemState;
        use std::sync::{Arc, Mutex};
        use tmux_control_parser::WindowId;

        #[derive(Resource, Default, Clone)]
        struct Added(Arc<Mutex<Vec<WindowId>>>);

        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.init_resource::<Added>();
        app.add_observer(|ev: On<TmuxWindowAdded>, added: Res<Added>| {
            added.0.lock().unwrap().push(ev.window);
        });

        let mut system_state: SystemState<(
            Commands,
            ResMut<EnumerationState>,
            ResMut<KeyBindings>,
            MessageWriter<PaneOutput>,
            NonSendMut<TmuxConnection>,
        )> = SystemState::new(app.world_mut());
        {
            let (mut commands, mut enumeration, mut keybindings, mut pane_output, mut connection) =
                system_state.get_mut(app.world_mut());
            apply_reply(
                &mut commands,
                &mut enumeration,
                &mut keybindings,
                &mut pane_output,
                &mut connection,
                PendingReply::ClientName,
                true,
                &["ozmux-0".to_string()],
            );
            apply_reply(
                &mut commands,
                &mut enumeration,
                &mut keybindings,
                &mut pane_output,
                &mut connection,
                PendingReply::ListWindows,
                true,
                &["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tmain".to_string()],
            );
        }
        system_state.apply(app.world_mut());

        assert_eq!(
            app.world()
                .non_send_resource::<TmuxConnection>()
                .client_name(),
            Some("ozmux-0")
        );
        assert_eq!(
            *app.world().resource::<Added>().0.lock().unwrap(),
            vec![WindowId(1)]
        );
    }
}
