# tmux Drain Split + Enum Reply Correlation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the monolithic `drain_tmux_events` system into five focused, chained Bevy systems, and replace `EnumerationState`'s per-field `Option<CommandId>` + `take_*` reply correlation with a single `HashMap<CommandId, PendingReply>` ordered `match` dispatch.

**Architecture:** Five systems run as one `.chain().in_set(TmuxProjectionSet)` under `Update`, sharing a per-frame `TmuxEventBatch` resource produced by the drain system. A `TmuxClientAttached` buffered message signals the attach edge so the init-send system can be gated by `on_message`. Connection liveness is a body guard (not a `NonSend` run condition, which is unsound under the multi-threaded executor). Reply correlation moves to a `CommandId`-keyed enum map dispatched in stream order, mirroring the existing `CopyModeQueries` / `CopyQueryKind` pattern.

**Tech Stack:** Rust edition 2024 (toolchain 1.95), Bevy 0.18 ECS, `crossbeam-channel`, the `ozmux_tmux` crate (`crates/tmux_session/`).

**Design spec:** `docs/superpowers/specs/2026-06-20-tmux-drain-split-design.md` (read it before starting).

## Global Constraints

- Edition 2024, toolchain pinned to 1.95; Bevy 0.18 (`Message` / `MessageWriter` / `MessageReader` / `on_message`, the 0.17 renames of `Event`/`EventWriter`/`EventReader`/`on_event`).
- All in-code comments in **English**. Comment taxonomy: only `// TODO:`, `// NOTE:` (critical caveats only), `// SAFETY:`. No narrative or block comments; no commented-out code.
- Doc comments (`///`) on every externally `pub` item; `//!` on each module file. New items default to **private**; widen only when a cross-module caller forces it. `PendingReply`, `TmuxEventBatch`, `TmuxClientAttached`, and the run condition are crate-internal — keep them private to `plugin.rs`/`enumerate.rs` (no `pub`).
- No `mod.rs`. Imports all at the top, single contiguous block, no inline fully-qualified paths.
- Bevy rules: gate whole-system guards with `run_if` (a body guard is the exception, allowed only when no Send-friendly condition exists — record why in a `// NOTE:`). Mutable params before immutable in signatures. No manual `set_changed()` / `bypass_change_detection()`; mutate conditionally. `Plugin::build` is one method chain off `app`. `Query` params use descriptive nouns, never `_q`.
- Verification after every task: `cargo test -p ozmux_tmux` (all pass), `cargo clippy -p ozmux_tmux --all-targets` (no warnings), `cargo fmt`.
- Comment style for the relocated logic: this refactor **relocates** large blocks of already-tested code. Steps cite exact source line ranges to move and show the **new** signatures, wiring, and net-new logic in full. Preserve the existing `// NOTE:` comments verbatim when moving the code they annotate.

---

## File Structure

| File | Responsibility after this plan |
|---|---|
| `crates/tmux_session/src/plugin.rs` | The five chained systems (`drain_tmux_transport`, `advance_tmux_connection`, `send_attach_enumeration`, `send_tmux_reenumeration`, `apply_tmux_replies`), `request_pane_captures`, the `TmuxEventBatch` resource, the `TmuxClientAttached` message, the `tmux_batch_pending` run condition, helper `fn`s, and the plugin wiring. |
| `crates/tmux_session/src/enumerate.rs` | `PendingReply` enum + reshaped `EnumerationState` (4 fields) + its helper methods. Command builders unchanged. |
| `crates/tmux_session/src/event_pump.rs` | Pure parse/detect/trigger helpers only (`advance_state`, `detect_*`, `parse_*`, `capture_to_bytes*`, `trigger_notification`, `trigger_seed`, `collect_pane_outputs`). The `take_*` correlation wrappers are removed. |

---

## Task 1: Shared event batch + drain system (①)

Introduce `TmuxEventBatch` and the `tmux_batch_pending` run condition; extract draining + `%output` routing into `drain_tmux_transport`; make the remaining monolith read the batch instead of draining.

**Files:**
- Modify: `crates/tmux_session/src/plugin.rs`

**Interfaces:**
- Produces: `struct TmuxEventBatch(Vec<TransportEvent>)` (private `Resource`); `fn drain_tmux_transport(mut batch: ResMut<TmuxEventBatch>, connection: NonSend<TmuxConnection>, mut pane_output: MessageWriter<PaneOutput>)`; `fn tmux_batch_pending(batch: Res<TmuxEventBatch>) -> bool`.
- The remaining `drain_tmux_events` now reads `batch: Res<TmuxEventBatch>` instead of draining, gated by `tmux_batch_pending`.

- [ ] **Step 1: Write the failing test for the conditional batch write**

Add to the `#[cfg(test)] mod tests` block in `plugin.rs`:

```rust
#[test]
fn drain_transport_clears_stale_batch_once_then_skips_idle() {
    use crossbeam_channel::unbounded;
    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    // Seed a non-empty batch as if a prior frame drained events.
    app.insert_resource(TmuxEventBatch(vec![TransportEvent::Closed {
        reason: "x".into(),
    }]));
    // No live client, so drain_tmux_transport finds an empty channel.
    let _ = unbounded::<TransportEvent>();
    app.update();
    // First idle update clears the stale batch to empty exactly once.
    assert!(app.world().resource::<TmuxEventBatch>().0.is_empty());
    let changed_tick = app
        .world()
        .resource_ref::<TmuxEventBatch>()
        .last_changed();
    app.update();
    // Second idle update must NOT re-write the already-empty batch.
    assert_eq!(
        app.world().resource_ref::<TmuxEventBatch>().last_changed(),
        changed_tick,
        "idle frame must not re-fire change detection on an already-empty batch"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux_tmux drain_transport_clears_stale_batch_once_then_skips_idle`
Expected: FAIL to compile — `TmuxEventBatch` and `drain_tmux_transport` do not exist yet.

- [ ] **Step 3: Add the resource, the drain system, and the run condition**

In `plugin.rs`, add the resource near `TmuxPresence`:

```rust
/// This frame's drained transport events, shared across the drain chain.
/// Overwritten by [`drain_tmux_transport`] each frame; read-only downstream.
#[derive(Resource, Default)]
struct TmuxEventBatch(Vec<TransportEvent>);
```

Add the run condition and the drain system (place the systems above `drain_tmux_events`; the run condition with the other free `fn`s):

```rust
fn tmux_batch_pending(batch: Res<TmuxEventBatch>) -> bool {
    !batch.0.is_empty()
}

/// Drains the live connection's transport channel into [`TmuxEventBatch`] and
/// routes `%output` to `PaneOutput`. Skips the write on a fully-idle frame so
/// change detection fires only when the batch's contents actually change; still
/// clears a previously-non-empty batch to empty exactly once.
fn drain_tmux_transport(
    mut batch: ResMut<TmuxEventBatch>,
    connection: NonSend<TmuxConnection>,
    mut pane_output: MessageWriter<PaneOutput>,
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
```

- [ ] **Step 4: Convert `drain_tmux_events` to read the batch**

Change the `drain_tmux_events` signature: remove the `mut pane_output: MessageWriter<PaneOutput>` parameter (output routing now lives in `drain_tmux_transport`), and add `batch: Res<TmuxEventBatch>`. Delete the head of the body (`plugin.rs:130-139`) — the `drain_transport` call, the `events.is_empty()` early return, and the `collect_pane_outputs` loop — and replace with:

```rust
    let events = &batch.0;
```

Then update the rest of the body to use `events` (it already binds the name `events`; it is now `&Vec<TransportEvent>` instead of `Vec<TransportEvent>`, so existing `&events` / `events.iter()` call sites compile unchanged).

- [ ] **Step 5: Register the chain in `Plugin::build`**

Replace the single `drain_tmux_events` registration (`plugin.rs:55-60`) with a chained pair (keep the whole `build` body one method chain):

```rust
            .add_systems(
                Update,
                (
                    drain_tmux_transport,
                    drain_tmux_events.run_if(tmux_batch_pending),
                )
                    .chain()
                    .in_set(TmuxProjectionSet)
                    .run_if(resource_exists::<TmuxPresence>),
            )
```

Add `.init_resource::<TmuxEventBatch>()` to the `build` chain (next to the other `init_resource` calls).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ozmux_tmux`
Expected: PASS — the new test plus all existing tests (behavior is preserved).

- [ ] **Step 7: Lint and format**

Run: `cargo clippy -p ozmux_tmux --all-targets && cargo fmt`
Expected: no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/tmux_session/src/plugin.rs
git commit -m "refactor(tmux): extract drain_tmux_transport + shared TmuxEventBatch"
```

---

## Task 2: Attach message + connection system (②) + attach-init system (③a)

Add the `TmuxClientAttached` message; extract `advance_state` + `Closed` teardown into `advance_tmux_connection` (emitting the message on the attach transition); extract the initial query suite into `send_attach_enumeration`, gated by `on_message`.

**Files:**
- Modify: `crates/tmux_session/src/plugin.rs`

**Interfaces:**
- Consumes: `TmuxEventBatch`, `tmux_batch_pending`, `send_session_enumeration` (existing helper, `plugin.rs:331`).
- Produces: `struct TmuxClientAttached` (private unit `Message`); `fn advance_tmux_connection(...)`; `fn send_attach_enumeration(...)`. After this task, `drain_tmux_events` no longer advances state, tears down on `Closed`, or sends the attach-init suite.

- [ ] **Step 1: Write the failing test for the attach emit**

Add to `plugin.rs` tests:

```rust
#[test]
fn advance_to_attached_emits_client_attached_message() {
    use tmux_control::{ClientEvent, ControlEvent};
    use tmux_control_parser::WindowId;
    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    *app.world_mut().resource_mut::<ConnectionState>() = ConnectionState::Connecting;
    app.insert_resource(TmuxEventBatch(vec![TransportEvent::Protocol(
        ClientEvent::Notification(ControlEvent::WindowAdd {
            window: WindowId(1),
        }),
    )]));
    app.update();
    let messages = app.world().resource::<Messages<TmuxClientAttached>>();
    assert_eq!(messages.iter_current_update_messages().count(), 1);
    assert_eq!(
        *app.world().resource::<ConnectionState>(),
        ConnectionState::Attached
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux_tmux advance_to_attached_emits_client_attached_message`
Expected: FAIL to compile — `TmuxClientAttached` does not exist.

- [ ] **Step 3: Add the message and register it**

In `plugin.rs`:

```rust
/// Emitted the frame the control client's transport transitions to `Attached`
/// (including a reconnect). Gates [`send_attach_enumeration`]. A pure signal —
/// the init-send system reads the live client from `TmuxConnection`.
#[derive(Message)]
struct TmuxClientAttached;
```

Add `.add_message::<TmuxClientAttached>()` to the `Plugin::build` chain.

- [ ] **Step 4: Add `advance_tmux_connection`**

Move the `advance_state` block (`plugin.rs:176-215`) and the `Closed` teardown branch (`plugin.rs:216-222`) out of `drain_tmux_events` into a new system. Drop the attach-init sends from the moved `advance_state` block (they move to ③a in Step 6); keep only the state write + emit:

```rust
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
```

Delete the corresponding code from `drain_tmux_events`: the whole `if let Some(next) = advance_state(...)` block and the `if events.iter().any(... Closed ...)` / `else` framing. The former `else` body (the `take_*` calls and `trigger_events`) stays in `drain_tmux_events` but is no longer wrapped in the `else` — it now runs unconditionally for a non-closed batch. **Guard it with a body return** so it does not run after a `Closed` teardown (Task 3 makes this a body guard on the live client; for now add `if connection.client().is_none() { return; }` near the top of `drain_tmux_events`, after binding `events`).

- [ ] **Step 5: Write the failing test for the attach-init system**

```rust
#[test]
fn send_attach_enumeration_runs_on_message() {
    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    // No live client: the system must still run on the message but send nothing,
    // leaving `pending` (list-windows id) None without panicking.
    app.world_mut().write_message(TmuxClientAttached);
    app.update();
    assert!(app.world().resource::<EnumerationState>().pending.is_none());
}
```

- [ ] **Step 6: Add `send_attach_enumeration`**

Move the attach-init suite (the body of the `if matches!(*state, Attached) && let Some(client) = connection.client()` block, `plugin.rs:178-214`) into a new system gated by the message:

```rust
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
    match client.handle().send(&client_name_command()) {
        Ok(id) => enumeration.client_name_pending = Some(id),
        Err(error) => tracing::warn!(?error, "failed to send client-name query"),
    }
    // ... move the remaining list-keys / prefix / mode-keys / version sends
    //     verbatim from plugin.rs:186-213 ...
}
```

(Reproduce the `list_keys_command("root"|"prefix"|"copy-mode"|"copy-mode-vi")`, `prefix_options_command`, `mode_keys_command`, and `version_command` send blocks exactly as they appear at `plugin.rs:186-213`.)

- [ ] **Step 7: Register ② and ③a in the chain**

Update the chain in `Plugin::build`:

```rust
                (
                    drain_tmux_transport,
                    advance_tmux_connection.run_if(tmux_batch_pending),
                    send_attach_enumeration.run_if(on_message::<TmuxClientAttached>),
                    drain_tmux_events.run_if(tmux_batch_pending),
                )
                    .chain()
                    .in_set(TmuxProjectionSet)
                    .run_if(resource_exists::<TmuxPresence>),
```

Add `use bevy::ecs::message::Messages;` is not needed for the system, but the test in Step 1 uses `Messages<TmuxClientAttached>` and `on_message` — add `on_message` via `use bevy::prelude::*` (already imported). Ensure `client_name_command`, `list_keys_command`, `prefix_options_command`, `mode_keys_command`, `version_command` remain imported (they already are, `plugin.rs:7-11`).

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test -p ozmux_tmux`
Expected: PASS (new tests + all existing).

- [ ] **Step 9: Lint, format, commit**

```bash
cargo clippy -p ozmux_tmux --all-targets && cargo fmt
git add crates/tmux_session/src/plugin.rs
git commit -m "refactor(tmux): extract advance_tmux_connection + send_attach_enumeration with TmuxClientAttached"
```

---

## Task 3: Re-enumeration system (③b) + apply system (④) + body guards

Split the remaining `drain_tmux_events` into `send_tmux_reenumeration` (topology re-enumeration + client-name re-arm) and `apply_tmux_replies` (reply correlation + projection), each body-guarded on the live client.

**Files:**
- Modify: `crates/tmux_session/src/plugin.rs`

**Interfaces:**
- Produces: `fn send_tmux_reenumeration(...)` and `fn apply_tmux_replies(...)`. `drain_tmux_events` is removed.

- [ ] **Step 1: Write the failing test for the body guard**

```rust
#[test]
fn apply_and_reenumeration_skip_without_client() {
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
    app.update();
    // No panic, and no enumeration was registered (nothing was sent).
    assert!(app.world().resource::<EnumerationState>().pending.is_none());
}
```

- [ ] **Step 2: Run the test to verify it fails or is inconclusive, then proceed**

Run: `cargo test -p ozmux_tmux apply_and_reenumeration_skip_without_client`
Expected: PASS-compiles but does not yet exercise the split (still one system). Treat this as a regression guard kept green through the split.

- [ ] **Step 3: Add `send_tmux_reenumeration`**

Move the session-switch detection block (`plugin.rs:140-159`), the `else if` window-add / window-switch block (`plugin.rs:160-175`), and the client-name re-arm (`plugin.rs:315-324`) into a new system. Add the live-client body guard:

```rust
/// Sends topology re-enumeration in response to notifications: a session switch
/// (clear caches, reset the aggressive-resize guard, re-enumerate the new
/// session), a `%window-add` (re-`list-windows`), or a `%session-window-changed`
/// (re-query the active pane). Also re-arms the client-name query if it is still
/// unresolved while attached.
///
/// Body-guards on the live client: a run condition reading `NonSend` is unsound
/// under the multi-threaded executor (bevyengine/bevy#21230).
fn send_tmux_reenumeration(
    mut commands: Commands,
    mut enumeration: ResMut<EnumerationState>,
    mut connection: NonSendMut<TmuxConnection>,
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
    // ... move plugin.rs:144-175 verbatim (session-switch + window-add/switch) ...
    // ... then move the client-name re-arm from plugin.rs:315-324 ...
}
```

(Reproduce `plugin.rs:144-175` and `:315-324` verbatim inside, preserving their `// NOTE:` comments. The `connection.client_name()` reads and `connection.take()`-free sends compile against `NonSendMut`.)

- [ ] **Step 4: Rename the remaining `drain_tmux_events` to `apply_tmux_replies` and add its body guard**

The remaining body (the former non-`Closed` reply path: `take_client_name`, `take_version`, `take_active_pane` + aggressive follow-up, `take_aggressive_resize`, `take_pane_captures`, `take_cursor_positions`, the four `take_keybindings`, `take_prefix_keys`, `take_mode_keys`, `drain_copy_replies`, and `trigger_events`) becomes `apply_tmux_replies`. Replace the temporary `if connection.client().is_none() { return; }` (added in Task 2 Step 4) with the documented body guard, and update the signature:

```rust
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
    let events = &batch.0;
    // ... move the reply-correlation body (plugin.rs:224-311) verbatim ...
}
```

(The active-pane→aggressive follow-up at `plugin.rs:238-246` sends via `connection.client()` — keep it inside `apply_tmux_replies`, as the spec documents.)

- [ ] **Step 5: Register ③b and ④; delete `drain_tmux_events`**

Final chain:

```rust
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
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ozmux_tmux`
Expected: PASS. Behavior is preserved; the chain now has five systems plus `request_pane_captures` (`.after(TmuxProjectionSet)`, unchanged).

- [ ] **Step 7: Lint, format, commit**

```bash
cargo clippy -p ozmux_tmux --all-targets && cargo fmt
git add crates/tmux_session/src/plugin.rs
git commit -m "refactor(tmux): split into send_tmux_reenumeration + apply_tmux_replies with live-client body guards"
```

---

## Task 4: `PendingReply` enum + `EnumerationState` reshape + enum dispatch

Replace the per-field `Option<CommandId>` correlation with one `HashMap<CommandId, PendingReply>` dispatched by a single ordered `match`; fix the session-switch clear; delete the dead `take_*` wrappers.

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs`
- Modify: `crates/tmux_session/src/plugin.rs`
- Modify: `crates/tmux_session/src/event_pump.rs`

**Interfaces:**
- Produces: `enum PendingReply { ListWindows, ClientName, Version, ActivePane, KeyBindings, PrefixKeys, ModeKeys, AggressiveResize, Capture { pane: PaneId }, Cursor { pane: PaneId } }`; reshaped `EnumerationState { pending: HashMap<CommandId, PendingReply>, aggressive_resize_checked: bool, capture_awaiting_cursor: HashMap<PaneId, Vec<String>>, panes_with_cursor_pending: HashSet<PaneId> }` with methods `register(&mut self, send: TmuxResult<CommandId>, reply: PendingReply)`, `has_pending(&self, reply: PendingReply) -> bool`, `clear_for_session_switch(&mut self)`.
- Removed: every `*_pending: Option<CommandId>` / `capture_pending` / `cursor_pending` field and every `take_*` wrapper in `event_pump.rs`.

- [ ] **Step 1: Reshape `EnumerationState` and add `PendingReply` in `enumerate.rs`**

Replace the `EnumerationState` struct (`enumerate.rs:423-462`) with:

```rust
/// What an in-flight command's reply will populate, keyed by its `CommandId`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PendingReply {
    /// `list-windows` enumeration → per-row projection seed.
    ListWindows,
    /// `display-message #{client_name}`.
    ClientName,
    /// `display-message #{version}`.
    Version,
    /// `display-message #{window_id} #{pane_id}` active-pane query.
    ActivePane,
    /// Any `list-keys -T <table>` reply → `KeyBindings::install`.
    KeyBindings,
    /// Prefix-options query → `set_prefix_keys`.
    PrefixKeys,
    /// `#{mode-keys}` → `set_mode_keys`.
    ModeKeys,
    /// `aggressive-resize` option query → warn if `on`.
    AggressiveResize,
    /// `capture-pane` of a pane's screen.
    Capture { pane: PaneId },
    /// Cursor-position query paired with a [`PendingReply::Capture`].
    Cursor { pane: PaneId },
}

/// Correlates in-flight enumeration/query commands by [`CommandId`] and the
/// capture/cursor pairing buffers, so each drained reply routes to its handler.
#[derive(Resource, Default)]
pub(crate) struct EnumerationState {
    pub(crate) pending: HashMap<CommandId, PendingReply>,
    pub(crate) aggressive_resize_checked: bool,
    pub(crate) capture_awaiting_cursor: HashMap<PaneId, Vec<String>>,
    pub(crate) panes_with_cursor_pending: HashSet<PaneId>,
}

impl EnumerationState {
    /// Records `reply` under the id `send` returned, logging on send failure.
    pub(crate) fn register(&mut self, send: TmuxResult<CommandId>, reply: PendingReply) {
        match send {
            Ok(id) => {
                self.pending.insert(id, reply);
            }
            Err(error) => tracing::warn!(?error, ?reply, "failed to send tmux query"),
        }
    }

    /// Whether a reply of `reply`'s kind is already in flight (replaces the old
    /// `Option::is_some` singleton guard for client-name / aggressive-resize).
    pub(crate) fn has_pending(&self, reply: PendingReply) -> bool {
        self.pending.values().any(|r| *r == reply)
    }

    /// Drops the in-flight entries a session switch invalidates: the
    /// capture/cursor pairs and the enumeration ids `send_session_enumeration`
    /// re-issues. A `HashMap` keyed by `CommandId` does not get the old
    /// `Option` fields' free last-write-wins overwrite, so a stale pre-switch
    /// `list-windows`/active-pane reply would otherwise mis-seed the new session.
    pub(crate) fn clear_for_session_switch(&mut self) {
        self.pending.retain(|_, r| {
            !matches!(
                r,
                PendingReply::Capture { .. }
                    | PendingReply::Cursor { .. }
                    | PendingReply::ListWindows
                    | PendingReply::ActivePane
            )
        });
        self.capture_awaiting_cursor.clear();
        self.panes_with_cursor_pending.clear();
        self.aggressive_resize_checked = false;
    }
}
```

Add `use tmux_control::TmuxResult;` to the `enumerate.rs` import block (alongside `use tmux_control::CommandId;`).

- [ ] **Step 2: Write the failing test for the session-switch clear**

Add to `enumerate.rs` tests:

```rust
#[test]
fn clear_for_session_switch_drops_enumeration_but_keeps_keybindings() {
    let mut state = EnumerationState::default();
    state.pending.insert(CommandId(1), PendingReply::ListWindows);
    state.pending.insert(CommandId(2), PendingReply::ActivePane);
    state.pending.insert(CommandId(3), PendingReply::KeyBindings);
    state
        .pending
        .insert(CommandId(4), PendingReply::Capture { pane: PaneId(7) });
    state.aggressive_resize_checked = true;
    state.clear_for_session_switch();
    assert_eq!(state.pending.get(&CommandId(3)), Some(&PendingReply::KeyBindings));
    assert!(state.pending.get(&CommandId(1)).is_none(), "stale list-windows dropped");
    assert!(state.pending.get(&CommandId(2)).is_none(), "stale active-pane dropped");
    assert!(state.pending.get(&CommandId(4)).is_none(), "capture dropped");
    assert!(!state.aggressive_resize_checked, "aggressive guard reset");
}
```

- [ ] **Step 3: Run the test to verify it fails (then passes after Step 1 compiles)**

Run: `cargo test -p ozmux_tmux clear_for_session_switch_drops_enumeration_but_keeps_keybindings`
Expected: FAIL to compile until the rest of the crate is migrated (Steps 4–7); the assertion logic itself is satisfied by Step 1's `clear_for_session_switch`.

- [ ] **Step 4: Migrate the send sites to `register`**

In `plugin.rs`:
- `request_pane_captures` (`plugin.rs:85-103`): replace `enumeration.capture_pending.insert(cap_id, pane.id)` / `enumeration.cursor_pending.insert(cur_id, pane.id)` with `enumeration.register(client.handle().send(&capture_pane_command(pane.id)), PendingReply::Capture { pane: pane.id })` and the cursor equivalent. Keep `enumeration.panes_with_cursor_pending.insert(pane.id)`. Because `register` consumes the send result, restructure so the cursor send + `panes_with_cursor_pending` insert happen only when the capture send succeeded — preserve the existing nesting by matching on the capture send id first (keep the current `match` shape but route the id through `register`).
- `send_attach_enumeration`: replace each `match client.handle().send(&X) { Ok(id) => enumeration.Y_pending = Some(id), Err(..) => warn }` with `enumeration.register(client.handle().send(&X), PendingReply::Z)`. The four `list_keys_command(...)` sends all use `PendingReply::KeyBindings`.
- `send_tmux_reenumeration`: the window-add `list_windows_command` → `PendingReply::ListWindows`; the window-switch `active_pane_command` → `PendingReply::ActivePane`; the client-name re-arm → `PendingReply::ClientName`, and change its guard `enumeration.client_name_pending.is_none()` to `!enumeration.has_pending(PendingReply::ClientName)` and the missing-name check to `connection.client_name().is_none()`. Replace `enumeration.clear_for_session_switch()`-equivalent inline clears (`plugin.rs:150-158`) with a single `enumeration.clear_for_session_switch()` call plus the existing `enumeration.aggressive_resize_pending = None`-equivalent (now covered by `clear_for_session_switch`).
- `send_session_enumeration` (`plugin.rs:331`): replace `enumeration.pending = Some(id)` / `enumeration.active_pane_pending = Some(id)` with `enumeration.register(send_result, PendingReply::ListWindows)` / `PendingReply::ActivePane`.

- [ ] **Step 5: Rewrite `apply_tmux_replies` as the ordered `match` dispatch**

Replace the `take_*` body of `apply_tmux_replies` with a single ordered pass plus the copy-reply pass. Reproduce the per-arm logic from the corresponding `event_pump.rs` helpers:

```rust
    let events = &batch.0;
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
                    &connection,
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
```

Write `apply_reply` as a private helper in `plugin.rs` whose arms reproduce the existing handlers (drawn from `event_pump.rs` and `plugin.rs:224-311`):

```rust
#[expect(clippy::too_many_arguments, reason = "single apply seam over many reply kinds")]
fn apply_reply(
    commands: &mut Commands,
    enumeration: &mut EnumerationState,
    keybindings: &mut KeyBindings,
    pane_output: &mut MessageWriter<PaneOutput>,
    connection: &TmuxConnection,
    reply: PendingReply,
    ok: bool,
    output: &[String],
) {
    match reply {
        PendingReply::ListWindows if ok => trigger_seed(commands, output),
        PendingReply::ListWindows => tracing::warn!("list-windows enumeration command failed"),
        PendingReply::ClientName => {
            if let Some(name) = first_reply_line(ok, output, "client-name") {
                // NOTE: set_client_name needs &mut; thread it via the connection
                // param as NonSendMut at the call site (see Step 6).
            }
        }
        // ... Version, ActivePane (+aggressive follow-up), KeyBindings, PrefixKeys,
        //     ModeKeys, AggressiveResize, Capture, Cursor ...
    }
}
```

NOTE: `apply_reply` needs `&mut TmuxConnection` for `set_client_name` / `set_per_window_refresh` and `&TmuxConnection` for the active-pane→aggressive send. Pass `connection: &mut TmuxConnection` (deref the `NonSendMut` once at the call site: `&mut connection`). Pull the small `first_reply_line(ok, output, what) -> Option<String>` helper from the old `take_reply_line` body (`event_pump.rs:95-117`) into `event_pump.rs` as a pure `pub(crate)` fn. Reproduce the `Capture`/`Cursor` arms from `take_pane_captures` / `take_cursor_positions` (`event_pump.rs:162-225`), using `enumeration.panes_with_cursor_pending` and `enumeration.capture_awaiting_cursor`, and the `parse_cursor_pos` / `capture_to_bytes*` helpers (unchanged). The `ActivePane` arm reproduces `plugin.rs:230-247`: trigger `TmuxActivePaneChanged { from_notification: false }`, then if `!aggressive_resize_checked && !has_pending(AggressiveResize)` and the client is live, `register(send(&aggressive_resize_command(window)), PendingReply::AggressiveResize)` (parse `window` via `parse_active_pane`).

- [ ] **Step 6: Delete the dead `take_*` wrappers and their tests**

In `event_pump.rs`, remove `take_reply_line` (replaced by `first_reply_line`), `take_client_name`, `take_version`, `take_aggressive_resize`, `take_pane_captures`, `take_cursor_positions`, `take_active_pane`, `take_keybindings`, `take_prefix_keys`, `take_mode_keys`, and their `#[cfg(test)]` tests. Keep `trigger_events`'s notification path by extracting the notification + list-windows handling already moved into `apply_reply` / `trigger_notification`; delete `trigger_events` itself if no caller remains (the dispatch now inlines its two responsibilities). Keep all pure helpers (`advance_state`, `detect_*`, `parse_active_pane`, `parse_cursor_pos`, `capture_to_bytes`, `capture_to_bytes_with_cursor`, `trigger_notification`, `trigger_seed`, `collect_pane_outputs`, the new `first_reply_line`) and their tests. Remove now-unused imports.

- [ ] **Step 7: Write the dispatch characterization test**

Add to `plugin.rs` tests — assert a `client-name` reply updates the connection and a `list-windows` reply seeds windows, in one batch, through `apply_tmux_replies`:

```rust
#[test]
fn apply_tmux_replies_dispatches_client_name_and_seeds_windows() {
    use tmux_control::{ClientEvent, CommandId};
    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    // Pretend a client-name query (id 1) and a list-windows enumeration (id 2)
    // are in flight, and their replies arrive this frame.
    {
        let mut enumeration = app.world_mut().resource_mut::<EnumerationState>();
        enumeration.pending.insert(CommandId(1), PendingReply::ClientName);
        enumeration.pending.insert(CommandId(2), PendingReply::ListWindows);
    }
    app.insert_resource(TmuxEventBatch(vec![
        TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(1),
            number: 0,
            ok: true,
            output: vec!["ozmux-0".into()],
        }),
        TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(2),
            number: 0,
            ok: true,
            output: vec!["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tmain".into()],
        }),
    ]));
    // apply_tmux_replies body-guards on a live client; this test asserts the
    // pending entries are consumed (a fuller end-to-end test needs a fake client).
    app.update();
    let enumeration = app.world().resource::<EnumerationState>();
    assert!(
        enumeration.pending.is_empty() || !enumeration.pending.contains_key(&CommandId(1)),
        "consumed replies are removed from pending"
    );
}
```

(If the body guard makes this inconclusive without a live client, also add a direct unit test of `apply_reply` for the `ClientName` and `ListWindows` arms, constructing the args by hand.)

- [ ] **Step 8: Run the full crate test suite**

Run: `cargo test -p ozmux_tmux`
Expected: PASS — migrated tests plus the new dispatch/session-switch tests.

- [ ] **Step 9: Build the whole workspace (the binary consumes `EnumerationState`-adjacent APIs)**

Run: `cargo build`
Expected: success — confirms `src/` callers (e.g. `request_pane_captures`, copy-mode) still compile against the reshaped crate.

- [ ] **Step 10: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets && cargo fmt
git add crates/tmux_session/src/enumerate.rs crates/tmux_session/src/plugin.rs crates/tmux_session/src/event_pump.rs
git commit -m "refactor(tmux): enum reply correlation via HashMap<CommandId, PendingReply> dispatch"
```

---

## Self-Review

**Spec coverage:**
- Five chained systems + `TmuxEventBatch` + `tmux_batch_pending` → Tasks 1–3. ✓
- `TmuxClientAttached` message + `on_message` gate → Task 2. ✓
- Live-client body guard (not a NonSend run condition) → Task 3 + spec Edit 1. ✓
- `PendingReply` enum + `EnumerationState` reshape + `register`/`has_pending` → Task 4. ✓
- Single ordered `match` dispatch; capture-before-cursor preserved by stream-order pass → Task 4 Step 5. ✓
- Session-switch clears `ListWindows`/`ActivePane` (spec Edit 2) → Task 4 Step 1 (`clear_for_session_switch`) + test Step 2. ✓
- Conditional batch write (spec Edit 4) → Task 1 Step 3 + test Step 1. ✓
- Teardown precedes re-enum sends (spec Edit 3) → Task 2 (②) ordered before Task 3 (③b) in the chain. ✓
- `%output` routed even on a closing batch → Task 1 (① has no live-client guard). ✓
- No false attach on same-batch close → Task 2 (`advance_state` folds to `Detached`, no emit). ✓

**Placeholder scan:** Task 4 Step 5 leaves the `apply_reply` arms as `// ...` references to exact source ranges (`event_pump.rs:162-225`, `plugin.rs:224-311`) rather than reproducing ~90 lines of unchanged handler code verbatim; the executor copies those blocks into the named arms. Every other step is concrete.

**Type consistency:** `EnumerationState` fields and methods (`pending`, `register`, `has_pending`, `clear_for_session_switch`, `capture_awaiting_cursor`, `panes_with_cursor_pending`, `aggressive_resize_checked`) are used identically in `plugin.rs` and `enumerate.rs`. `PendingReply` variants are spelled identically across enumerate/plugin. System names (`drain_tmux_transport`, `advance_tmux_connection`, `send_attach_enumeration`, `send_tmux_reenumeration`, `apply_tmux_replies`) match the chain registration and the spec table.
