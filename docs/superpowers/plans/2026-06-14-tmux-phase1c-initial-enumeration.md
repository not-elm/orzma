# Tmux Migration Phase 1c — Initial Enumeration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On attach, send one `list-windows` command, correlate its reply by `CommandId`, and seed the projection from it — so a session's *pre-existing* windows (and their full pane layouts) populate immediately instead of trickling in only as later `%layout-change` notifications. Also project a flat `TmuxSession` entity from the model's session id.

**Architecture:** Builds on Phase 1b (`ProjectionModel`, `seed_from_rows`, `parse_window_rows`, `LIST_WINDOWS_FORMAT`, the two-phase pump, `TmuxProjection` index). When `drain_tmux_events` advances `ConnectionState` to `Attached`, it sends `list-windows -F "<format>"` via the connection's `TmuxHandle` and records the returned `CommandId` in an `EnumerationState` resource. When the matching `CommandComplete { id, ok, output }` arrives, the pump parses the reply (`parse_window_rows`) and calls `seed_from_rows`. The reconcile gains a `TmuxSession` entity tracked in `TmuxProjection.session`. **No `ChildOf` hierarchy and no rendering** — entities stay flat (the session→window→pane hierarchy moves to Phase 2 with rendering).

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18, in-repo `tmux_control` + `tmux_control_parser`.

---

## Background — verified facts the implementer must rely on

Trust these.

- **`tmux_control` exports** `CommandId(pub u64)` deriving `Debug, Clone, Copy, PartialEq, Eq, Hash`; `ClientEvent::CommandComplete { id: CommandId, number: u32, ok: bool, output: Vec<String> }` | `Notification(ControlEvent)`; `TransportEvent::{Protocol(ClientEvent), Closed{reason}}`. `TmuxHandle::send(&str) -> TmuxResult<CommandId>`. A `&TmuxClient` exposes `handle() -> TmuxHandle` and `events() -> &Receiver<TransportEvent>`.
- **Phase 1b `ozmux_tmux` state** (`crates/tmux_session/src/`):
  - `enumerate.rs`: `pub const LIST_WINDOWS_FORMAT` (tab-separated, `#{window_name}` last), `pub struct WindowRow`, `pub fn parse_window_rows(&[String]) -> Result<Vec<WindowRow>, String>`.
  - `model.rs`: `ProjectionModel` (Resource) with `pub fn seed_from_rows(&mut self, &[WindowRow])` and `apply_event`.
  - `event_pump.rs`: `pub(crate) fn drain_transport`, `advance_state(&mut ConnectionState, &[TransportEvent]) -> bool`, `route_to_model(&mut ProjectionModel, &[TransportEvent]) -> bool`.
  - `reconcile.rs`: `pub struct TmuxProjection { pub windows: HashMap<WindowId, Entity>, pub panes: HashMap<PaneId, Entity> }` (Resource, Default), `pub(crate) fn reconcile_projection`, private `reconcile_windows`.
  - `components.rs`: `TmuxSession { id: SessionId }`, `TmuxWindow`, `TmuxPane` (Bevy `Component`s; `TmuxSession` derives `Component, Debug, Clone, Copy, PartialEq, Eq`).
  - `plugin.rs`: `drain_tmux_events(mut state: ResMut<ConnectionState>, mut model: ResMut<ProjectionModel>, mut connection: NonSendMut<TmuxConnection>)` already drains, advances state via `bypass_change_detection()` + `set_changed()`, routes notifications, and `connection.take()`s on `Closed`.
- **`connection.client() -> Option<&TmuxClient>`** on the `TmuxConnection` NonSend resource.
- **tmux control-mode command tokenizing:** a control-mode command line is re-tokenized by tmux on whitespace, so the `-F` format (which contains literal tab field-separators) MUST be double-quoted in the command string, or tmux will split it. This plan double-quotes it; the gated test verifies the reply parses (see Task 5's NOTE for the fallback).
- **Repo Rust rules:** no `mod.rs`; only `// TODO:`/`// NOTE:`/`// SAFETY:` comments; `//!` per module; `///` on every `pub` item; all `use` at top in one block; mutable params before immutable; private items after public; `std::collections::HashMap`; no `#[allow]`/`#[expect]` without a justified `// NOTE:`.

## File Structure

- Modify `crates/tmux_session/src/enumerate.rs` — add `list_windows_command()` + `EnumerationState` resource.
- Modify `crates/tmux_session/src/event_pump.rs` — add `seed_from_reply`.
- Modify `crates/tmux_session/src/reconcile.rs` — add `session` to `TmuxProjection` + `reconcile_session`.
- Modify `crates/tmux_session/src/plugin.rs` — register `EnumerationState`; rewire `drain_tmux_events` to send on attach + seed on reply.
- Create `crates/tmux_session/tests/real_tmux_enumeration.rs` — gated end-to-end test.

---

### Task 1: `list_windows_command()` + `EnumerationState`

**Files:** Modify `crates/tmux_session/src/enumerate.rs`.

- [ ] **Step 1: Extend the `use` block**

At the top of `crates/tmux_session/src/enumerate.rs`, the existing import is `use tmux_control_parser::{WindowId, WindowLayout};`. Add two imports so the block reads (keep it one contiguous block, ordered):

```rust
use bevy::prelude::Resource;
use tmux_control::CommandId;
use tmux_control_parser::{WindowId, WindowLayout};
```

- [ ] **Step 2: Add the command builder + resource**

After `parse_window_rows` (and its private helpers `parse_row`/`parse_window_id`), but before the `#[cfg(test)] mod tests` block, add:

```rust
/// Builds the `list-windows` command ozmux sends on attach to enumerate the
/// session's existing windows.
///
/// The `-F` format is double-quoted so its embedded tab field-separators
/// survive tmux's control-mode command tokenizer (which otherwise splits the
/// argument on whitespace).
pub(crate) fn list_windows_command() -> String {
    format!("list-windows -F \"{LIST_WINDOWS_FORMAT}\"")
}

/// Tracks the in-flight `list-windows` enumeration command so its reply can
/// be correlated by [`CommandId`] and seeded into the projection.
#[derive(Resource, Default)]
pub(crate) struct EnumerationState {
    /// The id of the in-flight `list-windows` command, if any.
    pub(crate) pending: Option<CommandId>,
}
```

- [ ] **Step 3: Add a test for the command shape**

Inside `enumerate.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn list_windows_command_quotes_the_format() {
        let cmd = list_windows_command();
        assert!(cmd.starts_with("list-windows -F \""));
        assert!(cmd.ends_with('"'));
        assert!(cmd.contains(LIST_WINDOWS_FORMAT));
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ozmux_tmux enumerate::`
Expected: the existing 6 tests plus `list_windows_command_quotes_the_format` pass (7). A `dead_code` warning on `list_windows_command`/`EnumerationState` is expected here (callers arrive in Task 4); do not run `-D warnings` yet and do not add suppressions.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/enumerate.rs
git commit -m "feat(tmux_session): list_windows_command + EnumerationState"
```

---

### Task 2: `seed_from_reply`

**Files:** Modify `crates/tmux_session/src/event_pump.rs`.

- [ ] **Step 1: Add the import**

`event_pump.rs` already imports `use crate::model::ProjectionModel;`. Add `use crate::enumerate::parse_window_rows;` to the top `use` block (keep it contiguous).

- [ ] **Step 2: Add the seed function**

After `route_to_model` (and before the private `log_transport_event`), add:

```rust
/// Parses a `list-windows` reply and seeds it into `model`, returning `true`
/// on success. A malformed reply is logged and leaves the model untouched.
pub(crate) fn seed_from_reply(model: &mut ProjectionModel, output: &[String]) -> bool {
    match parse_window_rows(output) {
        Ok(rows) => {
            model.seed_from_rows(&rows);
            true
        }
        Err(error) => {
            tracing::warn!(error = %error, "failed to parse list-windows reply");
            false
        }
    }
}
```

- [ ] **Step 3: Add tests**

Inside `event_pump.rs`'s `#[cfg(test)] mod tests` (which imports `tmux_control_parser::{PaneId, WindowId, WindowLayout}`), add:

```rust
    #[test]
    fn seed_from_reply_populates_model() {
        let output = vec!["1\t@1\tabcd,80x24,0,0,5\tx\tmain".to_string()];
        let mut model = ProjectionModel::default();
        assert!(seed_from_reply(&mut model, &output));
        assert_eq!(model.windows.len(), 1);
        assert_eq!(model.windows[0].id, WindowId(1));
        assert_eq!(model.windows[0].panes.first().map(|p| p.id), Some(PaneId(5)));
    }

    #[test]
    fn seed_from_reply_rejects_malformed() {
        let output = vec!["garbage".to_string()];
        let mut model = ProjectionModel::default();
        assert!(!seed_from_reply(&mut model, &output));
        assert!(model.windows.is_empty());
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ozmux_tmux event_pump::`
Expected: the existing event_pump tests plus the 2 new ones pass. (`dead_code` on `seed_from_reply` until Task 4 — expected; no `-D warnings` yet.)

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/event_pump.rs
git commit -m "feat(tmux_session): seed_from_reply parses a list-windows reply into the model"
```

---

### Task 3: `TmuxSession` entity reconcile

**Files:** Modify `crates/tmux_session/src/reconcile.rs`.

- [ ] **Step 1: Import `TmuxSession`**

Change reconcile.rs's `use crate::components::{TmuxPane, TmuxWindow};` to `use crate::components::{TmuxPane, TmuxSession, TmuxWindow};`.

- [ ] **Step 2: Add `session` to the index**

In `pub struct TmuxProjection`, add a field after `panes`:

```rust
    /// The session entity, if a session id is known.
    pub session: Option<Entity>,
```

- [ ] **Step 3: Reconcile the session entity**

In `reconcile_projection`, after the `reconcile_windows(&mut commands, &mut index, &model);` call, add `reconcile_session(&mut commands, &mut index, &model);`. Then add the private function (after `reconcile_windows`):

```rust
fn reconcile_session(commands: &mut Commands, index: &mut TmuxProjection, model: &ProjectionModel) {
    match (model.session, index.session) {
        (Some(id), Some(entity)) => {
            commands.entity(entity).insert(TmuxSession { id });
        }
        (Some(id), None) => {
            let entity = commands.spawn(TmuxSession { id }).id();
            index.session = Some(entity);
        }
        (None, Some(entity)) => {
            commands.entity(entity).despawn();
            index.session = None;
        }
        (None, None) => {}
    }
}
```

NOTE: the `TmuxSession` entity is FLAT (no `ChildOf` to windows). The session→window→pane hierarchy is deferred to Phase 2, so despawning the session never recursively touches window/pane entities — `despawn()` here is safe.

- [ ] **Step 4: Add a test**

In reconcile.rs's `#[cfg(test)] mod tests`, add `use tmux_control_parser::SessionId;` to its imports (alongside the existing `use tmux_control_parser::CellDims;`), then add:

```rust
    #[test]
    fn spawns_session_entity_from_model_session() {
        let mut app = app();
        app.world_mut().resource_mut::<ProjectionModel>().session = Some(SessionId(7));
        app.update();
        let entity = app
            .world()
            .resource::<TmuxProjection>()
            .session
            .expect("session entity spawned");
        assert_eq!(
            app.world().get::<TmuxSession>(entity).unwrap().id,
            SessionId(7)
        );
    }

    #[test]
    fn despawns_session_entity_when_session_cleared() {
        let mut app = app();
        app.world_mut().resource_mut::<ProjectionModel>().session = Some(SessionId(7));
        app.update();
        app.world_mut().resource_mut::<ProjectionModel>().session = None;
        app.update();
        assert!(app.world().resource::<TmuxProjection>().session.is_none());
    }
```

- [ ] **Step 5: Run the tests + clippy**

Run: `cargo test -p ozmux_tmux reconcile:: && cargo clippy -p ozmux_tmux -- -D warnings`
Expected: the existing reconcile tests plus the 2 new ones pass. Clippy may still warn `dead_code` on `list_windows_command`/`EnumerationState`/`seed_from_reply` (wired in Task 4) — if `-D warnings` fails on those, that is expected mid-plan; run plain `cargo test -p ozmux_tmux reconcile::` for this task and defer the strict clippy gate to Task 4. Do NOT add suppressions.

- [ ] **Step 6: Commit**

```bash
git add crates/tmux_session/src/reconcile.rs
git commit -m "feat(tmux_session): reconcile a flat TmuxSession entity from the model"
```

---

### Task 4: Wire enumeration into the pump

**Files:** Modify `crates/tmux_session/src/plugin.rs`.

- [ ] **Step 1: Extend imports**

In `plugin.rs`, update the `use` block:
- change `use crate::enumerate::...` — there is none yet; add `use crate::enumerate::{EnumerationState, list_windows_command};`
- change `use crate::event_pump::{advance_state, drain_transport, route_to_model};` to also import `seed_from_reply`: `use crate::event_pump::{advance_state, drain_transport, route_to_model, seed_from_reply};`
- the existing `use tmux_control::TransportEvent;` becomes `use tmux_control::{ClientEvent, TransportEvent};`

(Keep one contiguous, ordered `use` block.)

- [ ] **Step 2: Register the resource**

In `TmuxSessionPlugin::build`, add after the other `init_resource` calls:

```rust
        app.init_resource::<EnumerationState>();
```

- [ ] **Step 3: Rewire the drain system**

Replace the `drain_tmux_events` function body with the version below (it adds the `EnumerationState` param, the send-on-attach, and the reply-seed):

```rust
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
    let mut model_changed = route_to_model(model.bypass_change_detection(), &events);
    if let Some(pending) = enumeration.pending {
        for event in &events {
            if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output }) = event
                && *id == pending
            {
                enumeration.pending = None;
                if *ok {
                    model_changed |= seed_from_reply(model.bypass_change_detection(), output);
                } else {
                    tracing::warn!("list-windows enumeration command failed");
                }
                break;
            }
        }
    }
    if model_changed {
        model.set_changed();
    }
    if events
        .iter()
        .any(|event| matches!(event, TransportEvent::Closed { .. }))
    {
        connection.take();
        enumeration.pending = None;
    }
}
```

Also update the `drain_tmux_events` doc comment to mention it sends the `list-windows` enumeration on attach and seeds the model from the reply.

- [ ] **Step 4: Update the headless plugin test**

The existing `plugin_registers_resources_and_stays_idle_without_connection` test still holds (no connection → early return). Extend its assertions to confirm the new resource is registered and idle:

```rust
        assert!(
            app.world()
                .resource::<EnumerationState>()
                .pending
                .is_none()
        );
```

(Add `EnumerationState` to the test's imports if needed: `use crate::enumerate::EnumerationState;` inside the test module, or rely on `use super::*;`.)

- [ ] **Step 5: Run all crate tests + clippy**

Run: `cargo test -p ozmux_tmux && cargo clippy -p ozmux_tmux -- -D warnings`
Expected: all unit tests pass; clippy clean (the dead_code warnings from Tasks 1–3 are now resolved — `list_windows_command`/`EnumerationState`/`seed_from_reply` all have callers). Fix any failure properly; NO suppressions.

- [ ] **Step 6: Commit**

```bash
git add crates/tmux_session/src/plugin.rs
git commit -m "feat(tmux_session): send list-windows on attach and seed the projection from the reply"
```

---

### Task 5: Gated real-tmux enumeration test

**Files:** Create `crates/tmux_session/tests/real_tmux_enumeration.rs`.

- [ ] **Step 1: Write the gated test**

Create `crates/tmux_session/tests/real_tmux_enumeration.rs`:

```rust
//! Gated end-to-end test: a session with two windows attaches and the
//! projection populates both windows WITH panes — exercising the
//! list-windows enumeration + seed path.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_enumeration -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{ProjectionModel, TmuxConnection, TmuxSessionPlugin};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn enumeration_populates_existing_windows_with_panes() {
    let socket = format!("ozmux-phase1c-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    // Give the session a second window, then let tmux settle.
    client.handle().send("new-window").expect("new-window");
    std::thread::sleep(Duration::from_millis(500));

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while Instant::now() < deadline {
        app.update();
        let model = app.world().resource::<ProjectionModel>();
        if model.windows.len() >= 2 && model.windows.iter().all(|w| !w.panes.is_empty()) {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert!(
        ready,
        "both windows should be projected with panes (via the list-windows seed)"
    );

    if let Some(client) = app
        .world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection present")
        .take()
    {
        client.handle().send("kill-server").ok();
    }
}
```

- [ ] **Step 2: Compile, and run if tmux is present**

Run: `cargo test -p ozmux_tmux --test real_tmux_enumeration --no-run` (must compile).
Then check `command -v tmux`; if present, run with logging so a broken enumeration command is visible:
`RUST_LOG=warn cargo test -p ozmux_tmux --test real_tmux_enumeration -- --ignored --nocapture`
Expected: 1 test passes, and NO `list-windows enumeration command failed` / `failed to parse list-windows reply` warning appears.

NOTE (quoting fallback): if the test passes but you SEE a "list-windows enumeration command failed" or parse warning — or the test times out — tmux's tokenizer is splitting the tab-containing `-F` argument despite the quotes. Fall back to a non-whitespace separator: change `LIST_WINDOWS_FORMAT` in `enumerate.rs` to use the unit-separator byte `\u{1f}` instead of `\t` between fields, change `parse_row`'s `splitn(5, '\t')` to `splitn(5, '\u{1f}')`, drop the quoting in `list_windows_command` (the format then has no whitespace), and update the affected unit tests' fixture strings (`"1\t@1\t..."` → `"1\u{1f}@1\u{1f}..."`) and the `name_with_tabs_is_preserved_as_last_field` test accordingly. Report this if you hit it.

- [ ] **Step 3: Commit**

```bash
git add crates/tmux_session/tests/real_tmux_enumeration.rs
git commit -m "test(tmux_session): gated real-tmux enumeration-seeds-existing-windows test"
```

---

## Done criteria for Phase 1c

- On attach, `drain_tmux_events` sends `list-windows` once and records the `CommandId`; the matching `CommandComplete` reply seeds the model via `seed_from_reply`/`seed_from_rows`.
- `seed_from_reply` and `list_windows_command` are unit-tested; the enumeration is idempotent per attach and cleared on `Closed`.
- `reconcile_projection` projects a flat `TmuxSession` entity tracked in `TmuxProjection.session`.
- Gated real-tmux test confirms a two-window session populates both windows with panes via the seed, with no enumeration-command warning.
- `cargo test -p ozmux_tmux` passes; `cargo clippy -p ozmux_tmux -- -D warnings` clean; binary builds; no rendering added.

## Next: Phase 2 — rendering

PTY-less `ozma_tty_engine` API (detached constructor + external-chunk ingest + grid-only resize); route `%output` into per-pane `TerminalHandle`s; absolute cell-dim layout from `CellDims`; `refresh-client -C` window sizing; and the `ChildOf` session→window→pane hierarchy (deferred from here) wired so the renderer can query panes within windows. Flip `tmux.auto_connect` default to true.
