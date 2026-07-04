//! The `TmuxSessionPlugin`: connection state, the projection observers, and the
//! per-frame transport-drain system that triggers the projection events.

use crate::command::{
    ActivePane, AggressiveResize, CapturePane, CapturePanePending, CapturePaneSavedPrimary,
    CapturePaneWithHistory, ClientName, ListWindows, PaneStateQuery, SubscribeWindowFlags, Version,
};
use crate::components::{PaneRecaptureState, TmuxPane, TmuxSession};
use crate::connection::{TmuxAttached, TmuxClient};
use crate::enumerate::{
    EnumerationState, PaneRestore, PendingReply, Slot, version_supports_per_window_refresh,
};
use crate::event_pump::{
    detect_session_switch, detect_window_added, detect_window_switch, first_reply_line,
    log_transport_event, parse_active_pane, trigger_notification, trigger_seed,
};
use crate::events::{TmuxActivePaneChanged, TmuxWindowsRetained};
use crate::observers::{TmuxProjection, register_observers};
use crate::output::{PaneOutput, RequestPaneReseed, collect_pane_outputs};
use crate::state_restore::parse_pane_state;
use bevy::prelude::*;
use ozma_tty_engine::{AdoptedControlMode, ControlModeReleased, TerminalHandle, TerminalRawWrite};
use tmux_control::{ClientEvent, TmuxCommand, TransportEvent};
use tmux_control_parser::PaneId;

/// Soft per-frame event-count expectation. A single frame's feed produces the
/// events for all bytes the gateway PTY delivered that tick; exceeding this only
/// emits a warning (events are never dropped, since dropping a `CommandComplete`
/// would desync the FIFO command/reply correlation).
const MAX_EVENTS_PER_FRAME: usize = 4096;

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
        app.init_resource::<TmuxProjection>()
            .init_resource::<TmuxEventBatch>()
            .init_resource::<HistorySeedLines>()
            .add_observer(on_gateway_release)
            .add_message::<PaneOutput>()
            .add_message::<RequestPaneReseed>()
            .add_message::<TmuxClientAttached>()
            .add_systems(
                Update,
                (
                    drain_tmux_transport,
                    mark_attached_on_first_protocol.run_if(tmux_batch_pending),
                    send_attach_enumeration.run_if(on_message::<TmuxClientAttached>),
                    send_tmux_reenumeration.run_if(tmux_batch_pending),
                    apply_tmux_replies.run_if(tmux_batch_pending),
                    flush_tmux_outgoing,
                )
                    .chain()
                    .in_set(TmuxProjectionSet)
                    .run_if(any_with_component::<TmuxClient>),
            )
            .add_systems(
                Update,
                (
                    request_pane_captures,
                    recapture_settled_panes,
                    handle_pane_reseed_requests.run_if(on_message::<RequestPaneReseed>),
                )
                    .after(TmuxProjectionSet)
                    .run_if(any_with_component::<TmuxClient>),
            );
    }
}

/// Emitted the frame the control client's transport transitions to `Attached`
/// (including a reconnect). Gates [`send_attach_enumeration`]. A pure signal —
/// the init-send system reads the live client from the gateway's `TmuxClient`.
#[derive(Message)]
struct TmuxClientAttached;

/// This frame's drained transport events, shared across the drain chain.
/// Refreshed by `drain_tmux_transport` when the drain or the prior batch is
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

/// Lines of tmux history `RestoreDepth::Full` requests via `CapturePaneWithHistory`
/// on pane attach. Defaults to the engine's real scrollback cap; the binary
/// overrides it from `[scrollback] seed-lines` at startup.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistorySeedLines(pub usize);

impl Default for HistorySeedLines {
    fn default() -> Self {
        Self(TerminalHandle::default_scroll_cap())
    }
}

/// Sends the full restore command set once for each newly-projected pane so its
/// tmux-side state (history+screen, saved primary, terminal modes, pending
/// output) seeds the first paint. tmux `-CC` does not replay existing content on
/// attach (it only streams new `%output`), so without this a quiescent pane
/// stays blank until its program writes again. Gated on `Added<TmuxPane>` — runs
/// once per pane. The replies are consumed by [`apply_reply`]'s `Restore*` arms,
/// which synthesize and route the seed as `PaneOutput`.
fn request_pane_captures(
    mut client: Single<(&mut TmuxClient, &mut EnumerationState)>,
    new_panes: Query<&TmuxPane, Added<TmuxPane>>,
    seed_lines: Res<HistorySeedLines>,
) {
    let (client, enumeration) = &mut *client;
    let lines = seed_lines.0.min(TerminalHandle::default_scroll_cap());
    for pane in new_panes.iter() {
        request_pane_restore(
            client,
            enumeration,
            pane.id,
            pane.dims.height as u16,
            RestoreDepth::Full,
            lines,
        );
    }
}

/// How much of a pane's tmux-side state one restore pass fetches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RestoreDepth {
    /// Adopt-time: history+screen, saved primary, state, pending output.
    Full,
    /// Re-seed: visible screen + state (history accumulates via %output).
    Light,
}

/// Sends one restore command, registering its reply id. Returns false on a
/// send failure so the caller resolves that slot as Failed immediately —
/// a partially-sent restore still completes instead of wedging the pane.
fn send_restore_command(
    client: &mut TmuxClient,
    enumeration: &mut EnumerationState,
    command: impl TmuxCommand,
    reply: PendingReply,
) -> bool {
    match client.send(command) {
        Ok(id) => {
            enumeration.pending.insert(id, reply);
            true
        }
        Err(error) => {
            tracing::warn!(?error, ?reply, "failed to send restore command");
            false
        }
    }
}

/// Sends the restore command set for `pane` and registers a [`PaneRestore`]
/// buffer; [`apply_reply`] fills the slots and emits the synthesized seed
/// once every requested reply resolves. `lines` is only read on the
/// `RestoreDepth::Full` branch (the history-bearing capture); `Light` callers
/// may pass any value.
fn request_pane_restore(
    client: &mut TmuxClient,
    enumeration: &mut EnumerationState,
    pane: PaneId,
    pane_height: u16,
    depth: RestoreDepth,
    lines: usize,
) {
    let mut buffer = match depth {
        RestoreDepth::Full => PaneRestore::new_full(pane_height),
        RestoreDepth::Light => PaneRestore::new_light(pane_height),
    };
    let base_sent = match depth {
        RestoreDepth::Full => send_restore_command(
            client,
            enumeration,
            CapturePaneWithHistory { id: pane, lines },
            PendingReply::RestoreBase { pane },
        ),
        RestoreDepth::Light => send_restore_command(
            client,
            enumeration,
            CapturePane { id: pane },
            PendingReply::RestoreBase { pane },
        ),
    };
    if !base_sent {
        buffer.base = Slot::Failed;
    }
    if !send_restore_command(
        client,
        enumeration,
        PaneStateQuery { id: pane },
        PendingReply::RestoreState { pane },
    ) {
        buffer.state = Slot::Failed;
    }
    if depth == RestoreDepth::Full {
        if !send_restore_command(
            client,
            enumeration,
            CapturePaneSavedPrimary { id: pane },
            PendingReply::RestoreSavedPrimary { pane },
        ) {
            buffer.saved_primary = Slot::Failed;
        }
        if !send_restore_command(
            client,
            enumeration,
            CapturePanePending { id: pane },
            PendingReply::RestorePending { pane },
        ) {
            buffer.pending = Slot::Failed;
        }
    }
    // NOTE: when every send failed the buffer is already complete and no
    // reply will ever flush it; drop it (no seed) so the in-flight guard
    // clears and the aged reseed tracker can retry later.
    if !buffer.complete() {
        enumeration.restores.insert(pane, buffer);
    } else {
        tracing::warn!(pane = pane.0, "all restore commands failed to send");
    }
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
/// prompt to mid-screen. A light restore (clear + tmux's visible rows + terminal
/// state) overwrites both. The seed re-arms whenever the pane's dims change
/// (`done` is reset on a size change in the loop below), so the post-grow size is
/// re-seeded too; the settle delay debounces a window-drag to one re-seed per
/// settle. Re-seeding from tmux's authoritative grid is correct for a
/// display-only mirror — it resyncs and never loses real state (tmux owns the
/// content). Skipped while an earlier restore for the pane is still in flight so
/// the two restore passes never collide.
fn recapture_settled_panes(
    mut client: Single<(&mut TmuxClient, &mut EnumerationState)>,
    mut panes: Query<(&TmuxPane, &mut PaneRecaptureState)>,
) {
    let (client, enumeration) = &mut *client;
    for (pane, mut state) in panes.iter_mut() {
        let dims = (pane.dims.width, pane.dims.height);
        if state.dims != dims {
            // NOTE: re-arm the one-shot — a size change (e.g. a born-small
            // adopted pane grown to the client size) pulls scrollback onto the
            // screen and needs a fresh re-seed once the new size settles.
            *state = PaneRecaptureState {
                dims,
                stable: 0,
                done: false,
            };
        } else if !state.done && state.stable < RECAPTURE_SETTLE_FRAMES {
            state.stable += 1;
        }
        if !state.done
            && state.stable >= RECAPTURE_SETTLE_FRAMES
            && !enumeration.restores.contains_key(&pane.id)
        {
            state.done = true;
            request_pane_restore(
                client,
                enumeration,
                pane.id,
                pane.dims.height as u16,
                RestoreDepth::Light,
                TerminalHandle::default_scroll_cap(),
            );
        }
    }
}

/// Re-seeds each requested pane via a light `request_pane_restore`. Retry
/// cadence is owned by the binary's aged reseed tracker (spec §3.2); this
/// additionally skips a pane that already has a restore in flight so it cannot
/// collide with `recapture_settled_panes`.
fn handle_pane_reseed_requests(
    mut client: Single<(&mut TmuxClient, &mut EnumerationState)>,
    mut requests: MessageReader<RequestPaneReseed>,
    panes: Query<&TmuxPane>,
) {
    let (client, enumeration) = &mut *client;
    for req in requests.read() {
        // NOTE: a second restore for a pane whose buffer is still in flight
        // would land a duplicate seed; the aged reseed tracker retries later, so
        // suppressing here cannot wedge the pane.
        if enumeration.restores.contains_key(&req.pane) {
            continue;
        }
        let Some(pane_height) = panes
            .iter()
            .find(|p| p.id == req.pane)
            .map(|p| p.dims.height as u16)
        else {
            continue;
        };
        request_pane_restore(
            client,
            enumeration,
            req.pane,
            pane_height,
            RestoreDepth::Light,
            TerminalHandle::default_scroll_cap(),
        );
    }
}

fn tmux_batch_pending(batch: Res<TmuxEventBatch>) -> bool {
    !batch.0.is_empty()
}

/// Inserts [`TmuxAttached`] and emits [`TmuxClientAttached`] on the attach edge:
/// the first frame the connected gateway has a pending batch while it is not yet
/// attached. Gated on a pending batch.
///
/// The adopted stream's launch reply (flags=0) is skipped by the protocol client
/// and yields no event, so the attach edge is the first notification or
/// correlated reply — on a real `tmux -CC` attach, the session/window state tmux
/// streams immediately after the launch block. A launch-block-only frame (its
/// trailing notifications not yet read) merely defers attach to the next frame.
fn mark_attached_on_first_protocol(
    mut commands: Commands,
    mut attached: MessageWriter<TmuxClientAttached>,
    gateway: Single<Entity, With<TmuxClient>>,
    already: Query<(), With<TmuxAttached>>,
) {
    let gateway = *gateway;
    if already.get(gateway).is_ok() {
        return;
    }
    commands.entity(gateway).insert(TmuxAttached);
    attached.write(TmuxClientAttached);
}

/// Sends the one-time initial query suite when the client attaches:
/// `list-windows`, active-pane, window-flags subscription, client name, and
/// version. Gated by `on_message::<TmuxClientAttached>` so it runs exactly
/// once per attach edge.
fn send_attach_enumeration(mut client: Single<(&mut TmuxClient, &mut EnumerationState)>) {
    let (client, enumeration) = &mut *client;
    send_session_enumeration(client, enumeration);
    enumeration.register(client.send(ClientName), PendingReply::ClientName);
    enumeration.register(client.send(Version), PendingReply::Version);
}

/// Re-enumerates topology when the batch contains a session-switch, window-add,
/// or window-switch notification; re-arms the client-name query if the name has
/// not yet been learned after attach.
fn send_tmux_reenumeration(
    mut commands: Commands,
    mut client: Single<(Entity, &mut TmuxClient, &mut EnumerationState)>,
    already: Query<(), With<TmuxAttached>>,
    index: Res<TmuxProjection>,
    sessions: Query<&TmuxSession>,
    batch: Res<TmuxEventBatch>,
) {
    let (gateway, client, enumeration) = &mut *client;
    let gateway = *gateway;
    let events = &batch.0;
    let current_session = index
        .session
        .and_then(|e| sessions.get(e).ok())
        .map(|s| s.id);
    if detect_session_switch(events, current_session, client.client_name()).is_some() {
        commands.trigger(TmuxWindowsRetained {
            windows: Vec::new(),
        });
        // NOTE: aggressive-resize is a per-window option, so the switched-to
        // session must be re-checked; clear_for_session_switch resets the
        // one-shot guard along with the now-stale enumeration/capture ids.
        enumeration.clear_for_session_switch();
        send_session_enumeration(client, enumeration);
    } else {
        if detect_window_added(events) {
            enumeration.register(client.send(ListWindows), PendingReply::ListWindows);
        }
        if detect_window_switch(events, current_session) {
            enumeration.register(client.send(ActivePane), PendingReply::ActivePane);
        }
    }
    let is_attached = already.get(gateway).is_ok();
    if is_attached
        && client.client_name().is_none()
        && !enumeration.has_pending(PendingReply::ClientName)
    {
        enumeration.register(client.send(ClientName), PendingReply::ClientName);
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
    mut client: Single<(&mut AdoptedControlMode, &mut TmuxClient)>,
) {
    let (control, tmux) = &mut *client;
    let bytes = control.take_captured();
    let drained = match tmux.feed(&bytes) {
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
fn flush_tmux_outgoing(mut commands: Commands, mut client: Single<(Entity, &mut TmuxClient)>) {
    let (gateway, tmux) = &mut *client;
    let bytes = tmux.take_outgoing();
    if bytes.is_empty() {
        return;
    }
    commands.trigger(TerminalRawWrite {
        entity: *gateway,
        bytes,
    });
}

/// Applies this frame's command replies and notifications to the world: drains
/// each reply to what it answers, runs the active-pane→aggressive-resize
/// follow-up, and triggers the projection events the observers consume.
fn apply_tmux_replies(
    mut commands: Commands,
    mut client: Single<(&mut TmuxClient, &mut EnumerationState)>,
    mut pane_output: MessageWriter<PaneOutput>,
    batch: Res<TmuxEventBatch>,
) {
    let (client, enumeration) = &mut *client;
    let events = &batch.0;
    for event in events {
        match event {
            TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) => {
                let Some(reply) = enumeration.pending.remove(id) else {
                    continue;
                };
                apply_reply(
                    &mut commands,
                    enumeration,
                    &mut pane_output,
                    client,
                    reply,
                    *ok,
                    output,
                );
            }
            TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
                trigger_notification(&mut commands, client.client_name(), notification);
            }
            TransportEvent::Closed { .. } => {}
        }
    }
}

/// Routes one completed command's reply to the world state it answers,
/// reproducing the per-kind handler logic the old `take_*` wrappers held.
fn apply_reply(
    commands: &mut Commands,
    enumeration: &mut EnumerationState,
    pane_output: &mut MessageWriter<PaneOutput>,
    client: &mut TmuxClient,
    reply: PendingReply,
    ok: bool,
    output: &[String],
) {
    match reply {
        PendingReply::ListWindows if ok => trigger_seed(commands, output),
        PendingReply::ListWindows => tracing::warn!("list-windows enumeration command failed"),
        PendingReply::ClientName => {
            if let Some(name) = first_reply_line(ok, output, "client-name") {
                client.set_client_name(name);
            }
        }
        PendingReply::Version => {
            if let Some(version) = first_reply_line(ok, output, "version") {
                client.set_per_window_refresh(version_supports_per_window_refresh(&version));
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
            {
                enumeration.register(
                    client.send(AggressiveResize { win: window }),
                    PendingReply::AggressiveResize,
                );
            }
        }
        PendingReply::ActivePane => tracing::warn!("active-pane query command failed"),
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
        PendingReply::RestoreBase { pane } => {
            let slot = if ok {
                Slot::Ok(output.to_vec())
            } else {
                Slot::Failed
            };
            fill_restore_slot(enumeration, pane_output, pane, slot, |r, s| r.base = s);
        }
        PendingReply::RestoreSavedPrimary { pane } => {
            let slot = if ok {
                Slot::Ok(output.to_vec())
            } else {
                Slot::Failed
            };
            fill_restore_slot(enumeration, pane_output, pane, slot, |r, s| {
                r.saved_primary = s
            });
        }
        PendingReply::RestorePending { pane } => {
            let slot = if ok {
                Slot::Ok(output.to_vec())
            } else {
                Slot::Failed
            };
            fill_restore_slot(enumeration, pane_output, pane, slot, |r, s| r.pending = s);
        }
        PendingReply::RestoreState { pane } => {
            let slot = match first_reply_line(ok, output, "pane-state") {
                Some(line) => Slot::Ok(parse_pane_state(&line)),
                None => Slot::Failed,
            };
            fill_restore_slot(enumeration, pane_output, pane, slot, |r, s| r.state = s);
        }
    }
}

/// Resolves one slot of `pane`'s restore buffer and, once every requested
/// slot has resolved, synthesizes and emits the seed exactly once.
fn fill_restore_slot<T>(
    enumeration: &mut EnumerationState,
    pane_output: &mut MessageWriter<PaneOutput>,
    pane: PaneId,
    slot: Slot<T>,
    assign: impl FnOnce(&mut PaneRestore, Slot<T>),
) {
    let Some(restore) = enumeration.restores.get_mut(&pane) else {
        return;
    };
    assign(restore, slot);
    if restore.complete()
        && let Some(restore) = enumeration.restores.remove(&pane)
    {
        pane_output.write(PaneOutput {
            pane,
            data: restore.into_bytes(),
        });
    }
}

/// Sends the per-session enumeration queries (`list-windows` + active-pane) that
/// rebuild the projection. Shared by the attach transition and a session switch so
/// the two paths cannot drift (a switched-to session would otherwise risk stale
/// windows or a missing active-pane marker).
fn send_session_enumeration(client: &mut TmuxClient, enumeration: &mut EnumerationState) {
    enumeration.register(client.send(ListWindows), PendingReply::ListWindows);
    enumeration.register(client.send(ActivePane), PendingReply::ActivePane);
    if let Err(error) = client.send(SubscribeWindowFlags) {
        tracing::warn!(?error, "failed to subscribe to window flags");
    }
}

/// Strips the connection components from a gateway being released back to a
/// plain terminal on detach: `TmuxClient`, `TmuxAttached`, and the
/// crate-private `EnumerationState`.
///
/// Gated on [`ControlModeReleased`] — fired by the engine's own
/// `ReleaseControlMode` observer only when release genuinely completes — NOT
/// on `ReleaseControlMode` itself, which also fires when the released bytes
/// re-enter control mode (a fresh `tmux -CC` introducer glued into the
/// residue). Reacting to the raw `ReleaseControlMode` trigger would strip
/// these components even after that re-adoption re-inserts `TmuxClient`,
/// silently wedging the connection.
fn on_gateway_release(ev: On<ControlModeReleased>, mut commands: Commands) {
    commands
        .entity(ev.entity)
        .remove::<(TmuxClient, TmuxAttached, EnumerationState)>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_seed_lines_defaults_to_engine_cap() {
        assert_eq!(
            HistorySeedLines::default().0,
            TerminalHandle::default_scroll_cap()
        );
    }

    #[test]
    fn first_protocol_event_marks_attached_once() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        let gateway = app
            .world_mut()
            .spawn(AdoptedControlMode::from_captured(
                b"\x1bP1000p%begin 1 1 0\r\n%end 1 1 0\r\n%session-changed $0 0\r\n".to_vec(),
            ))
            .id();
        app.world_mut()
            .entity_mut(gateway)
            .insert(TmuxClient::new_adopted());
        app.update();
        assert!(
            app.world().get::<TmuxAttached>(gateway).is_some(),
            "first protocol event must mark the gateway TmuxAttached"
        );
        let attached = app.world().resource::<Messages<TmuxClientAttached>>();
        assert_eq!(attached.iter_current_update_messages().count(), 1);
    }

    #[test]
    fn drain_transport_clears_stale_batch_once_then_skips_idle() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.world_mut()
            .spawn((AdoptedControlMode::default(), TmuxClient::new_adopted()));
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
        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
        assert!(index.session.is_none());
    }

    #[test]
    fn send_attach_enumeration_runs_on_message() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.world_mut().spawn(TmuxClient::new_adopted());
        app.world_mut().write_message(TmuxClientAttached);
        app.update();
        let enumeration = app
            .world_mut()
            .query::<&EnumerationState>()
            .single(app.world())
            .expect("gateway entity must carry EnumerationState");
        // send_attach_enumeration fires on TmuxClientAttached and issues the
        // initial query suite; with a live client all registrations succeed, so
        // pending is non-empty.
        assert!(
            !enumeration.pending.is_empty(),
            "send_attach_enumeration must have registered at least one query"
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
        use tmux_control::{ClientEvent, ControlEvent};
        use tmux_control_parser::WindowId;
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        // Spawn a gateway entity carrying EnumerationState but NO TmuxClient: the
        // chain is gated on any_with_component::<TmuxClient>, and each driver takes
        // a Single<&mut TmuxClient>, so both systems are skipped entirely.
        app.world_mut().spawn(EnumerationState::default());
        // Non-empty batch but no live client: the gated chain must not run.
        app.insert_resource(TmuxEventBatch(vec![TransportEvent::Protocol(
            ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(9),
            }),
        )]));
        app.update();
        // No panic, and no enumeration was registered (nothing was sent).
        let enumeration = app
            .world_mut()
            .query::<&EnumerationState>()
            .single(app.world())
            .expect("gateway entity must carry EnumerationState");
        assert!(enumeration.pending.is_empty());
    }

    #[test]
    fn drive_feeds_captured_bytes_into_batch() {
        let mut app = App::new();
        app.init_resource::<TmuxEventBatch>()
            .add_message::<PaneOutput>();
        app.world_mut().spawn((
            AdoptedControlMode::from_captured(
                b"\x1bP1000p%begin 1 1 0\r\n%end 1 1 0\r\n%session-changed $0 0\r\n".to_vec(),
            ),
            TmuxClient::new_adopted(),
        ));
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
        app.world_mut().spawn(TmuxClient::new_adopted());
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

        let captured = |app: &mut App| {
            app.world_mut()
                .query::<&EnumerationState>()
                .single(app.world())
                .expect("gateway must carry EnumerationState")
                .pending
                .values()
                .any(|r| matches!(r, PendingReply::RestoreBase { pane } if *pane == PaneId(1)))
        };

        // Settle at the born size -> the re-seed fires once.
        for _ in 0..(RECAPTURE_SETTLE_FRAMES as usize + 1) {
            app.update();
        }
        assert!(captured(&mut app), "first settle must seed the pane");

        // Simulate the restore replies landing: clear the in-flight buffer (so
        // the next re-seed isn't blocked) and pending (so the second restore is
        // detectable).
        {
            let mut enumeration = app
                .world_mut()
                .query::<&mut EnumerationState>()
                .single_mut(app.world_mut())
                .expect("gateway must carry EnumerationState");
            enumeration.restores.clear();
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
            captured(&mut app),
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
        let mut client = TmuxClient::new_adopted();
        client.send_raw("list-windows").unwrap();
        let gateway = app.world_mut().spawn(client).id();
        app.add_systems(Update, flush_tmux_outgoing);
        app.update();

        let written = app.world().resource::<Written>().0.lock().unwrap().clone();
        assert_eq!(written, vec![(gateway, b"list-windows\n".to_vec())]);
    }

    #[test]
    fn apply_reply_client_name_sets_client_and_seeds_windows() {
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
        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();

        let mut system_state: SystemState<(
            Commands,
            Query<(&mut TmuxClient, &mut EnumerationState)>,
            MessageWriter<PaneOutput>,
        )> = SystemState::new(app.world_mut());
        {
            let (mut commands, mut client_q, mut pane_output) =
                system_state.get_mut(app.world_mut());
            let (mut client, mut enumeration) = client_q
                .single_mut()
                .expect("gateway entity must carry TmuxClient + EnumerationState");
            apply_reply(
                &mut commands,
                &mut enumeration,
                &mut pane_output,
                &mut client,
                PendingReply::ClientName,
                true,
                &["ozmux-0".to_string()],
            );
            apply_reply(
                &mut commands,
                &mut enumeration,
                &mut pane_output,
                &mut client,
                PendingReply::ListWindows,
                true,
                &["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tmain".to_string()],
            );
        }
        system_state.apply(app.world_mut());

        assert_eq!(
            app.world()
                .get::<TmuxClient>(gateway)
                .unwrap()
                .client_name(),
            Some("ozmux-0")
        );
        assert_eq!(
            *app.world().resource::<Added>().0.lock().unwrap(),
            vec![WindowId(1)]
        );
    }

    #[test]
    fn full_restore_emits_seed_once_after_all_replies_in_any_order() {
        use crate::enumerate::{PaneRestore, Slot};
        use bevy::ecs::system::SystemState;

        let base = vec!["line one".to_string(), "line two".to_string()];
        let saved_primary: Vec<String> = vec![];
        let pending = vec!["pending-tail".to_string()];
        let state_line = "cursor_x=3\tcursor_y=1\twrap_flag=1".to_string();

        // Independently synthesize the bytes the buffer must emit once complete.
        let expected = {
            let mut r = PaneRestore::new_full(2);
            r.base = Slot::Ok(base.clone());
            r.saved_primary = Slot::Ok(saved_primary.clone());
            r.pending = Slot::Ok(pending.clone());
            r.state = Slot::Ok(parse_pane_state(&state_line));
            r.into_bytes()
        };

        let mut app = App::new();
        app.add_message::<PaneOutput>();
        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        // Seed the in-flight full restore buffer request_pane_restore would have.
        app.world_mut()
            .get_mut::<EnumerationState>(gateway)
            .expect("TmuxClient auto-requires EnumerationState")
            .restores
            .insert(PaneId(1), PaneRestore::new_full(2));

        let mut system_state: SystemState<(
            Commands,
            Query<(&mut TmuxClient, &mut EnumerationState)>,
            MessageWriter<PaneOutput>,
        )> = SystemState::new(app.world_mut());
        {
            let (mut commands, mut client_q, mut pane_output) =
                system_state.get_mut(app.world_mut());
            let (mut client, mut enumeration) = client_q
                .single_mut()
                .expect("gateway entity must carry TmuxClient + EnumerationState");
            // Apply the four replies in a shuffled order to prove ordering does
            // not matter: state, pending, saved-primary, base.
            let replies = [
                (
                    PendingReply::RestoreState { pane: PaneId(1) },
                    vec![state_line.clone()],
                ),
                (
                    PendingReply::RestorePending { pane: PaneId(1) },
                    pending.clone(),
                ),
                (
                    PendingReply::RestoreSavedPrimary { pane: PaneId(1) },
                    saved_primary.clone(),
                ),
                (PendingReply::RestoreBase { pane: PaneId(1) }, base.clone()),
            ];
            for (reply, output) in replies {
                apply_reply(
                    &mut commands,
                    &mut enumeration,
                    &mut pane_output,
                    &mut client,
                    reply,
                    true,
                    &output,
                );
            }
        }
        system_state.apply(app.world_mut());

        let outputs: Vec<PaneOutput> = app
            .world()
            .resource::<Messages<PaneOutput>>()
            .iter_current_update_messages()
            .cloned()
            .collect();
        assert_eq!(
            outputs.len(),
            1,
            "the seed must be emitted exactly once, after the final reply resolves"
        );
        assert_eq!(outputs[0].pane, PaneId(1));
        assert_eq!(
            outputs[0].data, expected,
            "the emitted seed must match the synthesized restore bytes"
        );
        assert!(
            !app.world()
                .get::<EnumerationState>(gateway)
                .unwrap()
                .restores
                .contains_key(&PaneId(1)),
            "the restore buffer must be removed once the seed is emitted"
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
    /// `TmuxProjection` index. The leading DCS introducer plus the `%begin`/`%end`
    /// launch block (flags=0) are skipped as unsolicited (mirroring the `tmux -CC`
    /// entry block); the three notification lines that follow are the projected
    /// transcript.
    #[test]
    fn transcript_drives_ecs_projection_and_pane_output() {
        use tmux_control_parser::{PaneId, WindowId};

        // Canned transcript fed verbatim through the drain chain:
        //   * DCS introducer + initial %begin/%end launch block (flags=0) — the
        //     adopted tmux -CC entry block, skipped as an unsolicited block.
        //   * %window-add @1, %layout-change @1 (single 80x24 pane %1) — project a
        //     window and its pane. "b25f" is a real tmux layout checksum; the
        //     visible_layout field repeats the layout string (>= tmux 3.2 format).
        //   * %output %1 hello — routes to a PaneOutput message.
        let transcript: Vec<u8> = concat!(
            "\x1bP1000p%begin 1 1 0\r\n%end 1 1 0\r\n",
            "%window-add @1\r\n",
            "%layout-change @1 b25f,80x24,0,0,1 b25f,80x24,0,0,1\r\n",
            "%output %1 hello\r\n",
        )
        .as_bytes()
        .to_vec();

        // Build the app with the full projection pipeline registered: the plugin
        // inits TmuxProjection / TmuxEventBatch, the PaneOutput message, the
        // projection observers, plus the chained drain systems (gated on
        // any_with_component::<TmuxClient>).
        // EnumerationState is auto-required by TmuxClient.
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);

        // Stage the transcript in the gateway's capture buffer and insert a
        // TmuxClient, so drain_tmux_transport reads these bytes through the
        // gateway's own ProtocolClient — the same path live PTY output takes.
        app.world_mut().spawn((
            AdoptedControlMode::from_captured(transcript),
            TmuxClient::new_adopted(),
        ));

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
    /// re-send the on-attach enumeration. The attach edge inserts `TmuxAttached` on
    /// the gateway entity; despawning the gateway on teardown removes the marker, so
    /// the re-adoption's first protocol event can fire it again.
    #[test]
    fn second_adoption_after_reset_reattaches_and_reenumerates() {
        use crate::events::TmuxConnectionReset;

        // The DCS introducer + a %begin/%end launch block (flags=0, skipped as
        // unsolicited) followed by a %session-changed notification: the adopted
        // tmux -CC entry stream, whose first protocol event marks the gateway
        // TmuxAttached.
        fn entry_block() -> Vec<u8> {
            b"\x1bP1000p%begin 1 1 0\r\n%end 1 1 0\r\n%session-changed $0 0\r\n".to_vec()
        }

        fn attached_count(app: &App) -> usize {
            app.world()
                .resource::<Messages<TmuxClientAttached>>()
                .iter_current_update_messages()
                .count()
        }

        fn enumeration_pending_nonempty(app: &mut App) -> bool {
            !app.world_mut()
                .query::<&EnumerationState>()
                .single(app.world())
                .expect("gateway entity must carry EnumerationState")
                .pending
                .is_empty()
        }

        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);

        // First adoption: spawn the gateway with a TmuxClient (as
        // on_control_mode_detected does in production) and stage the entry block,
        // so the attach edge fires.
        let gateway1 = app
            .world_mut()
            .spawn((
                AdoptedControlMode::from_captured(entry_block()),
                TmuxClient::new_adopted(),
            ))
            .id();
        app.update();

        assert_eq!(
            attached_count(&app),
            1,
            "first adoption must fire the attach edge once"
        );
        assert!(
            app.world().get::<TmuxAttached>(gateway1).is_some(),
            "first adoption must mark gateway1 TmuxAttached"
        );
        assert!(
            enumeration_pending_nonempty(&mut app),
            "first adoption must send the on-attach enumeration"
        );

        // Teardown, mirroring src/session/tmux/adopt.rs::teardown's crate-facing effect:
        // despawn the gateway (taking its TmuxClient, EnumerationState, and
        // TmuxAttached marker with it) and trigger TmuxConnectionReset (which
        // resets the projection).
        app.world_mut().entity_mut(gateway1).despawn();
        app.world_mut().trigger(TmuxConnectionReset);
        app.update();

        assert!(
            app.world().get::<TmuxAttached>(gateway1).is_none(),
            "teardown must have despawned gateway1 (and its TmuxAttached marker)"
        );

        // Second adoption: a fresh gateway with a TmuxClient and a fresh entry
        // block. With the reset in place this folds Idle -> Attached again and
        // re-enumerates.
        let gateway2 = app
            .world_mut()
            .spawn((
                AdoptedControlMode::from_captured(entry_block()),
                TmuxClient::new_adopted(),
            ))
            .id();
        app.update();

        assert_eq!(
            attached_count(&app),
            1,
            "second adoption must fire the attach edge again"
        );
        assert!(
            app.world().get::<TmuxAttached>(gateway2).is_some(),
            "second adoption must mark gateway2 TmuxAttached"
        );
        assert!(
            enumeration_pending_nonempty(&mut app),
            "second adoption must re-send the on-attach enumeration"
        );
    }

    #[test]
    fn gateway_release_strips_connection_components_without_despawning() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_gateway_release);
        let gateway = app
            .world_mut()
            .spawn((
                AdoptedControlMode::default(),
                TmuxClient::new_adopted(),
                TmuxAttached,
            ))
            .id();

        app.world_mut()
            .trigger(ControlModeReleased { entity: gateway });
        app.update();

        assert!(
            app.world().get_entity(gateway).is_ok(),
            "the gateway entity survives release (it is not despawned)"
        );
        let entity = app.world().entity(gateway);
        assert!(entity.get::<TmuxClient>().is_none(), "TmuxClient stripped");
        assert!(
            entity.get::<TmuxAttached>().is_none(),
            "TmuxAttached stripped"
        );
        assert!(
            entity.get::<EnumerationState>().is_none(),
            "EnumerationState stripped"
        );
    }

    #[test]
    fn gateway_release_does_not_strip_on_reentrant_readoption() {
        // Regression: a fast `tmux -CC` reattach glued into the same detach
        // residue keeps the entity adopted — the engine fires
        // ControlModeDetected (re-inserting TmuxClient), NOT
        // ControlModeReleased. on_gateway_release must not react to that
        // reentrant path at all: it is only registered on ControlModeReleased,
        // so triggering ControlModeDetected here must leave the freshly
        // reinserted TmuxClient untouched.
        use ozma_tty_engine::ControlModeDetected;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_gateway_release);
        let gateway = app
            .world_mut()
            .spawn((AdoptedControlMode::default(), TmuxClient::new_adopted()))
            .id();

        app.world_mut()
            .trigger(ControlModeDetected { entity: gateway });
        app.update();

        assert!(
            app.world().get::<TmuxClient>(gateway).is_some(),
            "TmuxClient must survive: on_gateway_release only reacts to \
             ControlModeReleased, not the reentrant ControlModeDetected path"
        );
    }

    #[test]
    fn attach_edge_refires_after_component_strip_release() {
        use crate::events::TmuxConnectionReset;

        fn entry_block() -> Vec<u8> {
            b"\x1bP1000p%begin 1 1 0\r\n%end 1 1 0\r\n%session-changed $0 0\r\n".to_vec()
        }

        fn attached_count(app: &App) -> usize {
            app.world()
                .resource::<Messages<TmuxClientAttached>>()
                .iter_current_update_messages()
                .count()
        }

        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);

        let gateway = app
            .world_mut()
            .spawn((
                AdoptedControlMode::from_captured(entry_block()),
                TmuxClient::new_adopted(),
            ))
            .id();
        app.update();

        assert_eq!(
            attached_count(&app),
            1,
            "first adoption must fire the attach edge once"
        );
        assert!(
            app.world().get::<TmuxAttached>(gateway).is_some(),
            "first adoption must mark the gateway TmuxAttached"
        );

        app.world_mut()
            .trigger(ControlModeReleased { entity: gateway });
        app.world_mut().trigger(TmuxConnectionReset);
        app.update();

        assert!(
            app.world().get_entity(gateway).is_ok(),
            "the gateway entity survives release (it is not despawned)"
        );
        assert!(
            app.world().get::<TmuxClient>(gateway).is_none(),
            "TmuxClient stripped"
        );
        assert!(
            app.world().get::<TmuxAttached>(gateway).is_none(),
            "TmuxAttached stripped"
        );
        assert!(
            app.world().get::<EnumerationState>(gateway).is_none(),
            "EnumerationState stripped"
        );

        app.world_mut().entity_mut(gateway).insert((
            AdoptedControlMode::from_captured(entry_block()),
            TmuxClient::new_adopted(),
        ));
        app.update();

        assert_eq!(
            attached_count(&app),
            1,
            "re-adoption on the restored entity must fire the attach edge again"
        );
        assert!(
            app.world().get::<TmuxAttached>(gateway).is_some(),
            "re-adoption must mark the gateway TmuxAttached again"
        );
    }

    #[test]
    fn recapture_reseeds_reused_pane_id_after_pane_respawn() {
        // Regression: a reconnect to a restarted tmux server can reuse a pane id.
        // PaneRecaptureState now lives on the pane entity, so despawning the old
        // pane drops its `done: true` state and the respawned reused-id pane gets
        // a fresh component — the one-shot re-seed must fire again on reconnect.
        use tmux_control_parser::CellDims;

        let mut app = App::new();

        let dims = CellDims {
            width: 80,
            height: 24,
            xoff: 0,
            yoff: 0,
        };

        // First gateway + pane %1.
        let gateway1 = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let pane1 = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims,
            })
            .id();
        app.add_systems(Update, recapture_settled_panes);

        let pending_has_capture = |app: &mut App| {
            app.world_mut()
                .query::<&EnumerationState>()
                .single(app.world())
                .expect("gateway must carry EnumerationState")
                .pending
                .values()
                .any(|r| matches!(r, PendingReply::RestoreBase { pane } if *pane == PaneId(1)))
        };

        // Settle pane %1 on gateway1 so the re-seed fires and done=true.
        for _ in 0..(RECAPTURE_SETTLE_FRAMES as usize + 1) {
            app.update();
        }
        assert!(pending_has_capture(&mut app), "first settle must seed %1");

        // Simulate replies landing + teardown: despawn gateway1 and its pane.
        {
            let mut enumeration = app
                .world_mut()
                .query::<&mut EnumerationState>()
                .single_mut(app.world_mut())
                .expect("gateway must carry EnumerationState");
            enumeration.restores.clear();
            enumeration.pending.clear();
        }
        app.world_mut().entity_mut(pane1).despawn();
        app.world_mut().entity_mut(gateway1).despawn();
        // One update to flush despawns.
        app.update();

        // Re-adoption: fresh gateway entity + fresh pane reusing id %1.
        let _gateway2 = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        app.world_mut().spawn(TmuxPane {
            id: PaneId(1),
            dims,
        });

        // Settle the reconnected pane — the fresh PaneRecaptureState on the
        // respawned pane entity must re-arm the one-shot so the re-seed fires.
        for _ in 0..(RECAPTURE_SETTLE_FRAMES as usize + 1) {
            app.update();
        }
        assert!(
            pending_has_capture(&mut app),
            "re-adoption with a reused pane id must re-seed even after the first gateway's done=true"
        );
    }
}
