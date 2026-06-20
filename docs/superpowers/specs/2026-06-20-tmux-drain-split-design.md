# tmux Drain Split + Enum Reply Correlation Design

**Date:** 2026-06-20  
**Status:** Approved

## Problem

`drain_tmux_events` (`crates/tmux_session/src/plugin.rs`) is a single ~200-line
Bevy system that, every frame, does all of:

1. **Receive** — drain the transport channel and route `%output` to `PaneOutput`.
2. **Connection detection** — fold the batch through `advance_state`, write
   `ConnectionState`, and tear down on `Closed`.
3. **Reconnect-time init sending** — on the attach transition, send the initial
   query suite (`list-windows`, active-pane, window-flags subscription,
   client-name, four `list-keys` tables, prefix options, mode-keys, version).
4. **Topology re-enumeration** — on `%session-changed` / `%window-add` /
   `%session-window-changed`, send the targeted re-query.
5. **Event receiving** — correlate command replies back to what was asked and
   apply them to the world; trigger the projection events.

The single body interleaves immutable reads, broad `&mut`/`NonSendMut` access,
and outbound sends, so it is hard to read and hard to test in isolation.

Two structural problems compound the length:

- **Reply correlation is a flat scan-per-kind.** `EnumerationState` holds ~10
  separate `Option<CommandId>` fields plus two `HashMap`/`HashSet`s, and the
  body calls a matching `take_*` function for each, every one of which
  re-scans the whole event batch looking for its single id. Adding a query
  means adding a field, a `take_*`, and a call site.
- **An implicit ordering requirement.** `take_pane_captures` must run before
  `take_cursor_positions` (the `// NOTE:` at `plugin.rs:261-265`): when a
  capture reply and its paired cursor reply arrive in the same batch, captures
  populates `capture_awaiting_cursor` first and cursor drains it. Swapping the
  two calls silently drops cursor fixes.

## Goal / Non-goals

**Goal:** readability and responsibility clarity. Split the monolith into
focused, chained single-purpose systems, and replace the per-field `take_*`
correlation with a single `HashMap<CommandId, PendingReply>` enum dispatch
(mirroring the existing `CopyModeQueries` / `CopyQueryKind` pattern in
`copy_queries.rs`). Behavior is preserved except for the small, documented
ordering changes listed below.

**Non-goals (this change):**

- The gather→decide→apply `TmuxEffect`-intent rewrite (a deeper, testability-
  driven restructure) — noted as a future option, not done here.
- Reconnect automation, multiple concurrent connections, or finer connection
  states.
- Folding `CopyModeQueries` into the new enum — it is intentionally a separate,
  binary-owned channel and already follows the target pattern.

## Decision

Two changes land together (same PR), because the receive system is the consumer
of the new enum dispatch:

1. **Split `drain_tmux_events` into five chained systems** sharing a per-frame
   `TmuxEventBatch` resource, plus a new `TmuxClientAttached` message to signal
   the attach edge.
2. **Restructure reply correlation** in `EnumerationState` from per-field
   `Option<CommandId>` + `take_*` scans into one `HashMap<CommandId, PendingReply>`
   dispatched by a single ordered `match`.

## System Decomposition

All five systems are registered as one `.chain().in_set(TmuxProjectionSet)`
under `Update`, every member gated `run_if(resource_exists::<TmuxPresence>)`.
`TmuxProjectionSet` still wraps the chain, so the binary's downstream
`.after(TmuxProjectionSet)` systems (`render`, `window_bar`, `pane_focus`,
`divider_handle`, `request_pane_captures`) see the projection triggers applied
in the same frame, unchanged.

| # | System | Extra gate | Responsibility |
|---|---|---|---|
| ① | `drain_tmux_transport` | — | Drain the channel into `TmuxEventBatch`; route `%output` → `PaneOutput`. |
| ② | `advance_tmux_connection` | batch-pending | `advance_state` → conditional `ConnectionState` write; on attach transition emit `TmuxClientAttached`; on `Closed` → `connection.take()` + trigger `TmuxConnectionReset` + `TmuxConnectionClosed`. |
| ③a | `send_attach_enumeration` | `on_message::<TmuxClientAttached>` | Send the initial query suite. |
| ③b | `send_tmux_reenumeration` | batch-pending + connection-alive | Topology re-enumeration (session switch / window add / window switch) + client-name re-arm. |
| ④ | `apply_tmux_replies` | batch-pending + connection-alive | Single-pass enum dispatch (replies → world), active-pane→aggressive-resize follow-up, copy replies, notifications → projection triggers. |

### New shared resource

```rust
/// This frame's drained transport events, shared across the drain chain.
#[derive(Resource, Default)]
struct TmuxEventBatch(Vec<TransportEvent>);
```

`ResMut` in ① (overwritten each frame with the freshly drained `Vec`, empty when
the channel is empty), `Res` everywhere downstream. No cloning of events: every
consumer borrows `&batch.0`.

### New message

```rust
/// Emitted the frame the control client's transport transitions to `Attached`
/// (including reconnect). Gates `send_attach_enumeration`.
#[derive(Message)]
struct TmuxClientAttached;
```

Registered with `app.add_message::<TmuxClientAttached>()`. A unit struct — a pure
signal; the init-send system gets the live client from `connection.client()`.

`② advance_tmux_connection` emits it precisely on the Attached transition:

```rust
if let Some(next) = advance_state(&state, &batch.0) {
    let attached = matches!(next, ConnectionState::Attached);
    *state = next;
    if attached {
        attached_writer.write(TmuxClientAttached);
    }
}
```

Because `.chain()` orders ② before ③a, the message is delivered same-frame, and
the `on_message` run condition's internal cursor fires the system exactly once.

### New run conditions

```rust
fn tmux_batch_pending(batch: Res<TmuxEventBatch>) -> bool { !batch.0.is_empty() }
fn tmux_connection_alive(connection: NonSend<TmuxConnection>) -> bool {
    connection.client().is_some()
}
```

These express, as `run_if` gates (per the repo's "gate with `run_if`" rule), the
two whole-system guards the monolith did inline: skip when nothing was drained,
and skip the reply/teardown path once the connection is gone.

## Reply Correlation — `PendingReply`

`EnumerationState` collapses from ~16 fields to four, mirroring
`CopyModeQueries`:

```rust
/// What an in-flight command's reply will populate, keyed by its `CommandId`.
enum PendingReply {
    ListWindows,              // list-windows → trigger_seed (per-row projection)
    ClientName,               // display-message #{client_name}
    Version,                  // display-message #{version}
    ActivePane,               // #{window_id} #{pane_id}
    KeyBindings,              // list-keys -T <table> → install (root/prefix/copy-mode/copy-mode-vi)
    PrefixKeys,               // prefix options → set_prefix_keys
    ModeKeys,                 // #{mode-keys} → set_mode_keys
    AggressiveResize,         // show-options aggressive-resize → warn if "on"
    Capture { pane: PaneId }, // capture-pane → seed pane screen
    Cursor  { pane: PaneId }, // cursor position → pair with capture
}

#[derive(Resource, Default)]
struct EnumerationState {
    pending: HashMap<CommandId, PendingReply>,
    aggressive_resize_checked: bool,
    capture_awaiting_cursor: HashMap<PaneId, Vec<String>>,
    panes_with_cursor_pending: HashSet<PaneId>,
}
```

Notes:

- The four `list-keys` tables share one `KeyBindings` variant — their reply
  handling is identical (`install`); the table distinction only matters at send
  time.
- `AggressiveResize` carries no payload: the window id is needed to *build* the
  command, not to handle the reply.
- `aggressive_resize_checked` (completion flag) and the capture/cursor pairing
  buffers are state, not pending ids — they stay as dedicated fields.

### Send side

Registration is uniform across ②/③a/③b and `request_pane_captures`:

```rust
match client.handle().send(&client_name_command()) {
    Ok(id) => { enumeration.pending.insert(id, PendingReply::ClientName); }
    Err(error) => tracing::warn!(?error, "..."),
}
```

A small `register(state, send_result, reply)` helper folds the
`Ok→insert / Err→warn` shape. The "at most one in flight" guarantee the
`Option` fields used to give is only relied on for the client-name re-arm and
aggressive-resize, which already carry their own guards; those checks become
`pending.values().any(|r| matches!(r, PendingReply::ClientName))` over the small
map. Session-switch cache clearing becomes
`pending.retain(|_, r| !matches!(r, PendingReply::Capture { .. } | PendingReply::Cursor { .. }))`.

### Dispatch (`④ apply_tmux_replies`)

The `take_*` `if`-ladder becomes one ordered pass:

```rust
for event in &batch.0 {
    match event {
        TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) => {
            let Some(reply) = enumeration.pending.remove(id) else { continue }; // copy-mode / unknown
            match reply {
                PendingReply::ListWindows      => trigger_seed(&mut commands, output),
                PendingReply::ClientName       => { /* set_client_name */ }
                PendingReply::Version          => { /* set_per_window_refresh */ }
                PendingReply::ActivePane       => { /* trigger + aggressive follow-up */ }
                PendingReply::KeyBindings      => { /* install */ }
                PendingReply::PrefixKeys       => { /* set_prefix_keys */ }
                PendingReply::ModeKeys         => { /* set_mode_keys */ }
                PendingReply::AggressiveResize => { /* checked = true; warn if "on" */ }
                PendingReply::Capture { pane }  => { /* cache-or-emit */ }
                PendingReply::Cursor  { pane }  => { /* pair + emit */ }
            }
        }
        TransportEvent::Protocol(ClientEvent::Notification(n)) =>
            trigger_notification(&mut commands, own_client, n),
        TransportEvent::Closed { .. } => {} // handled in ②
    }
}
for reply in drain_copy_replies(&mut copy_queries, &batch.0) { copy_replies.write(reply); }
```

The per-arm `ok` handling and the pure parse helpers (`parse_list_keys`,
`parse_prefix`, `parse_active_pane`, `parse_cursor_pos`, `capture_to_bytes`,
etc.) are preserved as-is; only the per-kind `take_*` correlation wrappers go
away. Copy-mode replies stay on their separate pass (`drain_copy_replies`).

## Ordering Decisions and Documented Behavior Changes

Behavior is preserved except for these, each carrying a `// NOTE:`:

1. **Capture-before-cursor becomes structural.** The single ordered pass relies
   on tmux's FIFO replies (capture is sent before cursor for a pane), so the
   `Capture` arm populates `capture_awaiting_cursor` before the `Cursor` arm
   drains it — no explicit call-ordering requirement remains. The old
   `take_pane_captures`-before-`take_cursor_positions` `// NOTE:` is deleted.

2. **Teardown precedes sends.** In the monolith, topology sends ran before the
   `Closed` teardown. In the chain, ② (teardown) runs before ③b (sends). For
   the rare batch containing both a topology notification *and* `Closed`, the
   connection is taken first and ③b/④ skip via `tmux_connection_alive` —
   i.e. ozmux no longer sends on a dying connection. This is the more correct
   direction; documented with a `// NOTE:`.

3. **`%output` is still routed on a closing batch.** Output routing lives in ①
   (gated only on presence + batch-pending, *not* connection-alive), so a batch
   containing both `%output` and `Closed` still flushes the output — matching
   the monolith, where `%output` routing preceded the `Closed` check.

4. **No false attach on a same-batch close.** `advance_state` folds to the final
   state, so a `[protocol, Closed]` batch yields `Some(Detached)` and emits no
   `TmuxClientAttached` — equivalent to the old `matches!(*state, Attached)`
   guard.

## Module Placement

- `enumerate.rs` — `PendingReply` enum and the reshaped `EnumerationState`; the
  send-command builders are unchanged.
- `event_pump.rs` — keep the pure parse/detect helpers (`advance_state`,
  `detect_*`, `parse_*`, `capture_to_bytes*`, `trigger_notification`,
  `trigger_seed`). Remove the `take_*` correlation wrappers, superseded by the
  `match` arms. `collect_pane_outputs` stays (used by ①).
- `plugin.rs` — the five systems, the two run conditions, `TmuxEventBatch`, and
  the plugin wiring (`add_message::<TmuxClientAttached>()`, the chained
  `add_systems`). Bulky arms extracted into named helper `fn`s so each system
  body reads as gate → collect → trigger.
- `TmuxClientAttached` lives with the other tmux events in `events.rs`.

## Testing

- **Preserved:** the pure-helper unit tests in `event_pump.rs`, `state.rs`,
  `enumerate.rs` keep passing (helpers are unchanged). The two plugin
  integration tests are updated for the new resource/message wiring and the
  `run_if`-gated chain.
- **`send_attach_enumeration` in isolation:** write a `TmuxClientAttached`
  message and assert the expected `pending` entries are inserted — no transport
  transition needs to be simulated.
- **Enum dispatch:** build a `TmuxEventBatch` plus a seeded
  `EnumerationState.pending` and assert the resulting world/resource effects;
  the FIFO capture-then-cursor pairing is covered by ordering the two
  `CommandComplete`s in the batch.
- Any test that registers these systems adds the matching `run_if` so it
  exercises real scheduling.

## Out of Scope

- The `TmuxEffect`-intent gather→decide→apply rewrite.
- `CopyModeQueries` / `CopyQueryKind` (separate owner; already the target shape).
- `request_pane_captures` stays a separate `.after(TmuxProjectionSet)` system;
  only its inserts change to `pending.insert(.., PendingReply::Capture/Cursor)`.
- Reconnect automation, multiple connections, connection-state granularity.
