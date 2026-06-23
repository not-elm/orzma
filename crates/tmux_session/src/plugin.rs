//! The `TmuxSessionPlugin`: connection state, the projection observers, and the
//! per-frame transport-drain system that triggers the projection events.

use crate::command::{
    ActivePane, AggressiveResize, CapturePane, ClientName, CursorQuery, ListKeys, ListWindows,
    ModeKeys as ModeKeysCmd, PrefixOptions, SubscribeWindowFlags, Version,
};
use crate::components::{TmuxPane, TmuxSession};
use crate::connection::{AdoptedHandle, TmuxConnection};
use crate::copy_queries::{CopyModeQueries, CopyModeReply, drain_copy_replies};
use crate::enumerate::{EnumerationState, PendingReply, version_supports_per_window_refresh};
use crate::event_pump::{
    advance_state, capture_to_bytes, capture_to_bytes_with_cursor, detect_session_switch,
    detect_window_added, detect_window_switch, first_reply_line, log_transport_event,
    parse_active_pane, parse_cursor_pos, trigger_notification, trigger_seed,
};
use crate::events::{TmuxActivePaneChanged, TmuxWindowsRetained};
use crate::keybindings::{KeyBindings, ModeKeys, parse_list_keys, parse_prefix};
use crate::observers::{TmuxProjection, register_observers};
use crate::output::{PaneOutput, collect_pane_outputs};
use crate::state::ConnectionState;
use bevy::prelude::*;
use ozma_tty_engine::{AdoptedControlMode, TerminalRawWrite};
use std::collections::{HashMap, HashSet};
use tmux_control::{ClientEvent, TransportEvent};
use tmux_control_parser::PaneId;

/// Soft per-frame event-count expectation. A single frame's feed produces the
/// events for all bytes the gateway PTY delivered that tick; exceeding this only
/// emits a warning (events are never dropped, since dropping a `CommandComplete`
/// would desync the FIFO command/reply correlation).
const MAX_EVENTS_PER_FRAME: usize = 4096;

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
                    flush_tmux_outgoing,
                )
                    .chain()
                    .in_set(TmuxProjectionSet)
                    .run_if(resource_exists::<TmuxPresence>),
            )
            .add_systems(
                Update,
                (request_pane_captures, recapture_settled_panes)
                    .after(TmuxProjectionSet)
                    .run_if(resource_exists::<TmuxPresence>),
            );
    }
}

/// Emitted the frame the control client's transport transitions to `Attached`
/// (including a reconnect). Gates [`send_attach_enumeration`]. A pure signal —
/// the init-send system reads the live client from `TmuxConnection`.
#[derive(Message)]
struct TmuxClientAttached;

/// This frame's drained transport events, shared across the drain chain.
/// Refreshed by [`drain_tmux_transport`] when the drain or the prior batch is
/// non-empty; read-only downstream.
#[derive(Resource, Default)]
pub struct TmuxEventBatch(Vec<TransportEvent>);

impl TmuxEventBatch {
    /// Returns this frame's drained transport events.
    ///
    /// Lets the binary's adoption-lifecycle systems scan for the `%exit`
    /// notification that drives teardown without owning the drain.
    pub fn events(&self) -> &[TransportEvent] {
        &self.0
    }

    /// Drops any buffered events.
    ///
    /// Called on connection reset so a previous connection's events (notably a
    /// `%exit`) cannot leak into the next one — the drain is gated off in Default
    /// mode, so without this the stale batch would survive a teardown and tear
    /// down the next adopted connection on sight.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    #[cfg(test)]
    pub(crate) fn from_events(events: Vec<TransportEvent>) -> Self {
        Self(events)
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
    let Some(handle) = connection.handle() else {
        return;
    };
    for pane in new_panes.iter() {
        request_pane_capture(&mut enumeration, &handle, pane.id);
    }
}

/// Sends a `capture-pane` + cursor-position query for `pane` and registers the
/// pending replies so [`apply_reply`] seeds the mirror from tmux's authoritative
/// grid (clear-screen + rows + the real cursor position).
fn request_pane_capture(enumeration: &mut EnumerationState, handle: &AdoptedHandle, pane: PaneId) {
    match handle.send(CapturePane { id: pane }) {
        Ok(cap_id) => {
            enumeration
                .pending
                .insert(cap_id, PendingReply::Capture { pane });
            match handle.send(CursorQuery { id: pane }) {
                Ok(cur_id) => {
                    enumeration
                        .pending
                        .insert(cur_id, PendingReply::Cursor { pane });
                    enumeration.panes_with_cursor_pending.insert(pane);
                }
                Err(error) => {
                    tracing::warn!(?error, pane = pane.0, "failed to send cursor query");
                }
            }
        }
        Err(error) => {
            tracing::warn!(?error, pane = pane.0, "failed to send capture-pane")
        }
    }
}

/// Per-pane state for [`recapture_settled_panes`].
#[derive(Default)]
struct PaneRecaptureState {
    /// Last-seen cell dims, to detect size changes.
    dims: (u32, u32),
    /// Frames the dims have held steady since the last change.
    stable: u8,
    /// Whether this pane has been re-seeded since its last size change.
    done: bool,
}

/// Frames a pane's size must hold steady before its mirror is re-seeded from
/// tmux. Lets a born-small pane finish growing (and a window-drag finish) before
/// the re-seed fires.
const RECAPTURE_SETTLE_FRAMES: u8 = 3;

/// Re-seeds each pane's display mirror from tmux's authoritative grid a few
/// frames after the pane's size settles, re-arming on every size change.
///
/// NOTE: two divergences make a re-seed necessary. (1) On a fresh session's
/// first prompt the display-only `alacritty_terminal` mirror lands the prompt
/// one row low — zsh's `PROMPT_EOL_MARK` over-fills the line by one column and
/// alacritty wraps the cursor where tmux does not. (2) When a born-small pane is
/// grown to the control client's size (the common case when an adopted
/// `tmux -CC` session starts at tmux's default-size and the client then enlarges
/// it), alacritty's grow pulls local scrollback onto the screen and pushes the
/// prompt to mid-screen. A capture-pane + cursor seed (clear + tmux's rows + the
/// real cursor) overwrites both. The seed re-arms whenever the pane's dims change
/// (`done` is reset on a size change in the loop below), so the post-grow size is
/// re-seeded too; the settle delay debounces a window-drag to one re-seed per
/// settle. Re-seeding from tmux's authoritative grid is correct for a
/// display-only mirror — it resyncs and never loses real state (tmux owns the
/// content). Skipped while an earlier capture for the pane is still in flight so
/// the two capture/cursor pairs never collide.
fn recapture_settled_panes(
    mut watch: Local<HashMap<PaneId, PaneRecaptureState>>,
    mut enumeration: ResMut<EnumerationState>,
    connection: NonSend<TmuxConnection>,
    panes: Query<&TmuxPane>,
) {
    // NOTE: prune departed panes every frame, even while the control client is
    // absent, so a reconnect that reuses a pane id starts from a clean slate.
    let present: HashSet<PaneId> = panes.iter().map(|pane| pane.id).collect();
    watch.retain(|id, _| present.contains(id));

    let Some(handle) = connection.handle() else {
        return;
    };
    for pane in panes.iter() {
        let dims = (pane.dims.width, pane.dims.height);
        let state = watch.entry(pane.id).or_insert(PaneRecaptureState {
            dims,
            stable: 0,
            done: false,
        });
        if state.dims != dims {
            state.dims = dims;
            state.stable = 0;
            // NOTE: re-arm the one-shot — a size change (e.g. a born-small
            // adopted pane grown to the client size) pulls scrollback onto the
            // screen and needs a fresh re-seed once the new size settles.
            state.done = false;
        } else {
            state.stable = state.stable.saturating_add(1);
        }
        if !state.done
            && state.stable >= RECAPTURE_SETTLE_FRAMES
            && !enumeration.panes_with_cursor_pending.contains(&pane.id)
        {
            state.done = true;
            request_pane_capture(&mut enumeration, &handle, pane.id);
        }
    }
}

fn tmux_batch_pending(batch: Res<TmuxEventBatch>) -> bool {
    !batch.0.is_empty()
}

/// Folds the batch through `advance_state`, writes `ConnectionState` only on a
/// real transition (so change detection fires once per transition), and emits
/// `TmuxClientAttached` on the attach edge.
///
/// NOTE: connection close/teardown (the former `TransportEvent::Closed` arm) is
/// reintroduced by the adoption-lifecycle task — the in-world feed produces only
/// `Protocol` events, so there is no `Closed` event to act on here yet.
fn advance_tmux_connection(
    mut state: ResMut<ConnectionState>,
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
}

/// Sends the one-time initial query suite when the client attaches:
/// `list-windows`, active-pane, window-flags subscription, client name, the four
/// `list-keys` tables, prefix options, mode-keys, and version. Gated by
/// `on_message::<TmuxClientAttached>` so it runs exactly once per attach edge.
fn send_attach_enumeration(
    mut enumeration: ResMut<EnumerationState>,
    connection: NonSend<TmuxConnection>,
) {
    let Some(handle) = connection.handle() else {
        return;
    };
    send_session_enumeration(&mut enumeration, &handle);
    enumeration.register(handle.send(ClientName), PendingReply::ClientName);
    enumeration.register(
        handle.send(ListKeys { table: "root" }),
        PendingReply::KeyBindings,
    );
    enumeration.register(
        handle.send(ListKeys { table: "prefix" }),
        PendingReply::KeyBindings,
    );
    enumeration.register(handle.send(PrefixOptions), PendingReply::PrefixKeys);
    enumeration.register(
        handle.send(ListKeys { table: "copy-mode" }),
        PendingReply::KeyBindings,
    );
    enumeration.register(
        handle.send(ListKeys {
            table: "copy-mode-vi",
        }),
        PendingReply::KeyBindings,
    );
    enumeration.register(handle.send(ModeKeysCmd), PendingReply::ModeKeys);
    enumeration.register(handle.send(Version), PendingReply::Version);
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
    let Some(handle) = connection.handle() else {
        return;
    };
    let events = &batch.0;
    let current_session = index
        .session
        .and_then(|e| sessions.get(e).ok())
        .map(|s| s.id);
    if detect_session_switch(events, current_session, connection.client_name()).is_some() {
        commands.trigger(TmuxWindowsRetained {
            windows: Vec::new(),
        });
        // NOTE: aggressive-resize is a per-window option, so the switched-to
        // session must be re-checked; clear_for_session_switch resets the
        // one-shot guard along with the now-stale enumeration/capture ids.
        enumeration.clear_for_session_switch();
        send_session_enumeration(&mut enumeration, &handle);
    } else {
        if detect_window_added(events) {
            enumeration.register(handle.send(ListWindows), PendingReply::ListWindows);
        }
        if detect_window_switch(events, current_session) {
            enumeration.register(handle.send(ActivePane), PendingReply::ActivePane);
        }
    }
    if matches!(*state, ConnectionState::Attached)
        && connection.client_name().is_none()
        && !enumeration.has_pending(PendingReply::ClientName)
    {
        enumeration.register(handle.send(ClientName), PendingReply::ClientName);
    }
}

/// Feeds the adopted gateway's captured PTY bytes through the in-world protocol
/// into [`TmuxEventBatch`] and routes `%output` to `PaneOutput`. Skips the write
/// on a fully-idle frame so change detection fires only when the batch's
/// contents actually change; still clears a previously-non-empty batch to empty
/// exactly once.
fn drain_tmux_transport(
    mut batch: ResMut<TmuxEventBatch>,
    mut pane_output: MessageWriter<PaneOutput>,
    mut adopted: Query<&mut AdoptedControlMode>,
    connection: NonSend<TmuxConnection>,
) {
    let bytes = match connection.gateway() {
        Some(gateway) => adopted
            .get_mut(gateway)
            .map(|mut control| control.take_captured())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let drained = match connection.feed(&bytes) {
        Ok(events) => {
            if events.len() > MAX_EVENTS_PER_FRAME {
                tracing::warn!(
                    count = events.len(),
                    cap = MAX_EVENTS_PER_FRAME,
                    "tmux feed produced an unusually large event batch this frame"
                );
            }
            let drained: Vec<TransportEvent> =
                events.into_iter().map(TransportEvent::Protocol).collect();
            for event in &drained {
                log_transport_event(event);
            }
            drained
        }
        Err(error) => {
            tracing::warn!(?error, "tmux protocol feed failed");
            Vec::new()
        }
    };
    if drained.is_empty() && batch.0.is_empty() {
        return;
    }
    for output in collect_pane_outputs(&drained) {
        pane_output.write(output);
    }
    batch.0 = drained;
}

/// Flushes the protocol's outgoing buffer to the adopted gateway PTY via
/// [`TerminalRawWrite`], so commands queued by this frame's send sites reach
/// tmux. Registered last in the chained tmux set so every send completes first.
fn flush_tmux_outgoing(mut commands: Commands, connection: NonSend<TmuxConnection>) {
    if !connection.is_connected() {
        return;
    }
    let bytes = connection.take_outgoing();
    if bytes.is_empty() {
        return;
    }
    if let Some(gateway) = connection.gateway() {
        commands.trigger(TerminalRawWrite {
            entity: gateway,
            bytes,
        });
    }
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
    if !connection.is_connected() {
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
                && let Some(handle) = connection.handle()
            {
                enumeration.register(
                    handle.send(AggressiveResize { win: window }),
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
fn send_session_enumeration(enumeration: &mut EnumerationState, handle: &AdoptedHandle) {
    enumeration.register(handle.send(ListWindows), PendingReply::ListWindows);
    enumeration.register(handle.send(ActivePane), PendingReply::ActivePane);
    if let Err(error) = handle.send(SubscribeWindowFlags) {
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
    fn drive_feeds_captured_bytes_into_batch() {
        let mut app = App::new();
        app.init_resource::<TmuxEventBatch>()
            .add_message::<PaneOutput>();
        let gateway = app
            .world_mut()
            .spawn(AdoptedControlMode::from_captured(
                b"\x1bP1000p%begin 1 1 1\r\n%end 1 1 1\r\n".to_vec(),
            ))
            .id();
        let mut conn = TmuxConnection::default();
        let _ = conn.adopt(gateway);
        app.insert_non_send_resource(conn);
        app.add_systems(Update, drain_tmux_transport);
        app.update();
        assert!(!app.world().resource::<TmuxEventBatch>().0.is_empty());
    }

    #[test]
    fn recapture_rearms_after_pane_size_change() {
        // Regression for the adoption mid-screen-prompt bug: a born-small pane
        // grown to the control client's size pulls local scrollback onto the
        // screen and pushes the prompt mid-screen. The one-shot re-seed must
        // re-arm on the size change so the post-grow size is re-captured from
        // tmux's authoritative grid (otherwise the misrender persists).
        use tmux_control_parser::CellDims;
        let mut app = App::new();
        app.init_resource::<EnumerationState>();
        let gateway = app.world_mut().spawn_empty().id();
        let mut conn = TmuxConnection::default();
        let _ = conn.adopt(gateway);
        app.insert_non_send_resource(conn);
        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 80,
                    height: 24,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        app.add_systems(Update, recapture_settled_panes);

        let captured = |app: &App| {
            app.world()
                .resource::<EnumerationState>()
                .pending
                .values()
                .any(|r| matches!(r, PendingReply::Capture { pane } if *pane == PaneId(1)))
        };

        // Settle at the born size -> the re-seed fires once.
        for _ in 0..(RECAPTURE_SETTLE_FRAMES as usize + 1) {
            app.update();
        }
        assert!(captured(&app), "first settle must seed the pane");

        // Simulate the capture/cursor replies landing: clear the in-flight
        // markers (so the next re-seed isn't blocked) and pending (so the second
        // capture is detectable).
        {
            let mut enumeration = app.world_mut().resource_mut::<EnumerationState>();
            enumeration.panes_with_cursor_pending.clear();
            enumeration.pending.clear();
        }

        // Grow the pane, as when the client pins a larger size after adoption.
        app.world_mut()
            .get_mut::<TmuxPane>(pane)
            .unwrap()
            .dims
            .height = 48;

        // Settle at the new size -> the re-armed one-shot must fire AGAIN.
        for _ in 0..(RECAPTURE_SETTLE_FRAMES as usize + 1) {
            app.update();
        }
        assert!(
            captured(&app),
            "a pane size change must re-arm the re-seed so the grown size is re-captured"
        );
    }

    #[test]
    fn flush_outgoing_triggers_raw_write_to_gateway() {
        use std::sync::{Arc, Mutex};

        #[derive(Resource, Default, Clone)]
        struct Written(Arc<Mutex<Vec<(Entity, Vec<u8>)>>>);

        let mut app = App::new();
        app.init_resource::<Written>();
        app.add_observer(|ev: On<TerminalRawWrite>, written: Res<Written>| {
            written
                .0
                .lock()
                .unwrap()
                .push((ev.entity, ev.bytes.clone()));
        });
        let gateway = app.world_mut().spawn_empty().id();
        let mut conn = TmuxConnection::default();
        let _ = conn.adopt(gateway);
        conn.handle().unwrap().send_raw("list-windows").unwrap();
        app.insert_non_send_resource(conn);
        app.add_systems(Update, flush_tmux_outgoing);
        app.update();

        let written = app.world().resource::<Written>().0.lock().unwrap().clone();
        assert_eq!(written, vec![(gateway, b"list-windows\n".to_vec())]);
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
        let gateway = app.world_mut().spawn_empty().id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);

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

    /// End-to-end integration: a canned `tmux -CC` byte transcript, staged in the
    /// gateway's `AdoptedControlMode` buffer, is driven through the REAL drain
    /// chain by `app.update()` and projected into ECS state.
    ///
    /// Transcript path: Option A (notification-driven). The bytes are NOT
    /// pre-parsed by the test; they flow through the production pipeline exactly
    /// as live PTY output would:
    /// `AdoptedControlMode.captured` -> `drain_tmux_transport`
    /// (`ProtocolClient::feed` -> `TmuxEventBatch`, and `collect_pane_outputs`
    /// -> `MessageWriter<PaneOutput>` for `%output`) -> `apply_tmux_replies`
    /// (`trigger_notification` for `%window-add` / `%layout-change`) -> the
    /// projection observers -> `TmuxWindow` / `TmuxPane` entities in the
    /// `TmuxProjection` index. The leading DCS introducer plus a `%begin`/`%end`
    /// block correlate with the external pending reply that `adopt` pre-registers
    /// (mirroring the `tmux -CC` entry block); the three notification lines that
    /// follow are the projected transcript.
    #[test]
    fn transcript_drives_ecs_projection_and_pane_output() {
        use tmux_control_parser::{PaneId, WindowId};

        // Canned transcript fed verbatim through the drain chain:
        //   * DCS introducer + initial %begin/%end block — the adopted tmux -CC
        //     entry block, correlated by adopt()'s pre-registered external reply.
        //   * %window-add @1, %layout-change @1 (single 80x24 pane %1) — project a
        //     window and its pane. "b25f" is a real tmux layout checksum; the
        //     visible_layout field repeats the layout string (>= tmux 3.2 format).
        //   * %output %1 hello — routes to a PaneOutput message.
        let transcript: Vec<u8> = concat!(
            "\x1bP1000p%begin 1 1 1\r\n%end 1 1 1\r\n",
            "%window-add @1\r\n",
            "%layout-change @1 b25f,80x24,0,0,1 b25f,80x24,0,0,1\r\n",
            "%output %1 hello\r\n",
        )
        .as_bytes()
        .to_vec();

        // Build the app with the full projection pipeline registered: the plugin
        // inits TmuxProjection / EnumerationState / KeyBindings / CopyModeQueries
        // / TmuxEventBatch, the PaneOutput / CopyModeReply messages, the
        // projection observers, and the NonSend TmuxConnection, plus the chained
        // drain systems (gated on TmuxPresence).
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.insert_resource(TmuxPresence);

        // Stage the transcript in the gateway's capture buffer and adopt it, so
        // drain_tmux_transport reads these bytes through the connection's own
        // ProtocolClient — the same path live PTY output takes.
        let gateway = app
            .world_mut()
            .spawn(AdoptedControlMode::from_captured(transcript))
            .id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);

        // Drive the real chain: drain_tmux_transport -> apply_tmux_replies -> ...
        app.update();

        // TmuxWindow for @1 must appear in the projection index.
        let index = app.world().resource::<TmuxProjection>();
        assert!(
            index.windows.contains_key(&WindowId(1)),
            "TmuxWindow for @1 must be projected from %window-add + %layout-change"
        );
        assert!(
            index.panes.contains_key(&PaneId(1)),
            "TmuxPane for %1 must be projected from %layout-change"
        );

        let window_entity = index.windows[&WindowId(1)];
        let (pane_entity, owning_window) = index.panes[&PaneId(1)];
        assert_eq!(
            owning_window,
            WindowId(1),
            "pane %1 must belong to window @1"
        );

        // Verify the ECS component values on the projected entities.
        assert_eq!(
            app.world()
                .get::<crate::components::TmuxWindow>(window_entity)
                .unwrap()
                .id,
            WindowId(1)
        );
        let pane = app
            .world()
            .get::<crate::components::TmuxPane>(pane_entity)
            .unwrap();
        assert_eq!(pane.id, PaneId(1));
        assert_eq!(
            (pane.dims.width, pane.dims.height),
            (80, 24),
            "pane dims from %layout-change must be 80x24"
        );

        // Verify %output routing through the real message bus: the drain system's
        // `MessageWriter<PaneOutput>` must have written exactly one message.
        let pane_outputs: Vec<PaneOutput> = app
            .world()
            .resource::<Messages<PaneOutput>>()
            .iter_current_update_messages()
            .cloned()
            .collect();
        assert_eq!(
            pane_outputs.len(),
            1,
            "%output %1 must produce exactly one PaneOutput message"
        );
        assert_eq!(pane_outputs[0].pane, PaneId(1));
        assert_eq!(
            pane_outputs[0].data, b"hello",
            "%output body must reach PaneOutput.data verbatim"
        );
    }

    /// A second adoption (after a teardown reset) must re-fire the attach edge and
    /// re-send the on-attach enumeration. The attach edge is gated on a real
    /// `ConnectionState` transition; teardown's `TmuxConnectionReset` must restore
    /// `ConnectionState` to the initial `Idle` so the re-adoption folds
    /// `Idle -> Attached` again. Without the reset, `ConnectionState` stays
    /// `Attached`, `advance_state` folds to `None`, `TmuxClientAttached` never
    /// fires the second time, and the re-adopted session never enumerates.
    #[test]
    fn second_adoption_after_reset_reattaches_and_reenumerates() {
        use crate::events::TmuxConnectionReset;

        // The DCS introducer + a %begin/%end block: the adopted tmux -CC entry
        // block, which correlates with adopt()'s pre-registered external reply and
        // produces one Protocol event that flips ConnectionState to Attached.
        fn entry_block() -> Vec<u8> {
            b"\x1bP1000p%begin 1 1 1\r\n%end 1 1 1\r\n".to_vec()
        }

        fn attached_count(app: &App) -> usize {
            app.world()
                .resource::<Messages<TmuxClientAttached>>()
                .iter_current_update_messages()
                .count()
        }

        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.insert_resource(TmuxPresence);

        // First adoption: stage the entry block in the gateway capture buffer and
        // adopt, then drive the real chain so the attach edge fires.
        let gateway1 = app
            .world_mut()
            .spawn(AdoptedControlMode::from_captured(entry_block()))
            .id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway1);
        app.update();

        assert_eq!(
            attached_count(&app),
            1,
            "first adoption must fire the attach edge once"
        );
        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Attached,
            "first adoption must reach Attached"
        );
        assert!(
            !app.world()
                .resource::<EnumerationState>()
                .pending
                .is_empty(),
            "first adoption must send the on-attach enumeration"
        );

        // Teardown, mirroring src/tmux/adopt.rs::teardown's crate-facing effect:
        // close the connection and trigger TmuxConnectionReset (which resets the
        // projection AND ConnectionState back to Idle).
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .close();
        app.world_mut().trigger(TmuxConnectionReset);
        app.update();

        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Idle,
            "teardown reset must restore ConnectionState to Idle"
        );
        assert!(
            app.world()
                .resource::<EnumerationState>()
                .pending
                .is_empty(),
            "teardown reset must clear the prior enumeration"
        );

        // Second adoption: a fresh gateway with a fresh entry block. With the
        // reset in place this folds Idle -> Attached again and re-enumerates.
        let gateway2 = app
            .world_mut()
            .spawn(AdoptedControlMode::from_captured(entry_block()))
            .id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway2);
        app.update();

        assert_eq!(
            attached_count(&app),
            1,
            "second adoption must fire the attach edge again (regressed before the \
             ConnectionState reset: it stayed Attached so advance_state folded to None)"
        );
        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Attached,
            "second adoption must reach Attached again"
        );
        assert!(
            !app.world()
                .resource::<EnumerationState>()
                .pending
                .is_empty(),
            "second adoption must re-send the on-attach enumeration"
        );
    }
}
