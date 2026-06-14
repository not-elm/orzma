# Tmux Migration Phase 0 — Bevy Integration Scaffold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a new `ozmux_tmux` Bevy plugin crate that owns a `tmux -CC` control connection, drains its transport events each frame, logs them, and tracks a `ConnectionState` — proving the `tmux_control` transport works end-to-end inside the Bevy app, with the old multiplexer still running untouched.

**Architecture:** A new crate `crates/tmux_session` (package `ozmux_tmux`) wraps the existing `tmux_control` library. Because `tmux_control::TmuxClient` owns a `Box<dyn MasterPty + Send>` and is therefore `Send` but **not `Sync`**, it is held in a Bevy **`NonSend` resource** (`TmuxConnection`), not a normal `Resource`. A normal `Resource` (`ConnectionState`) tracks the lifecycle. The per-frame drain is a `NonSend` system that delegates to a pure, channel-based `drain_events` function so the core logic is unit-testable without spawning tmux. Phase 0 does **not** auto-connect (that is Phase 1) and does **not** project any entities; it only wires the plumbing and logs.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18, `crossbeam-channel`, `tracing`, the in-repo `tmux_control` crate.

---

## Background — verified facts the implementer must rely on

These were confirmed by reading the codebase. Do not re-derive; trust them.

- `tmux_control` public API (`crates/tmux_control/src/lib.rs`):
  - `TmuxServer::new()` → `.program(&str)` / `.socket_name(&str)` / `.socket_path(&str)` builders.
  - `TmuxServer::new_session() -> TmuxResult<TmuxClient>` (spawns `tmux -CC new-session`).
  - `TmuxServer::attach(&str) -> TmuxResult<TmuxClient>`.
  - `TmuxClient::events(&self) -> &crossbeam_channel::Receiver<TransportEvent>`.
  - `TmuxClient::handle(&self) -> TmuxHandle`, `TmuxClient::kill(&mut self)`.
  - `TransportEvent` enum: `Protocol(ClientEvent)` | `Closed { reason: String }`.
  - `ClientEvent` enum: `CommandComplete { id: CommandId, number: u32, ok: bool, output: Vec<String> }` | `Notification(ControlEvent)`.
  - `ControlEvent`, `SessionId` re-exported from `tmux_control`.
- `TmuxClient` is `Send` but **not `Sync`** (holds `Box<dyn MasterPty + Send>`). It MUST be a `NonSend` resource. Do not add `#[derive(Resource)]` to a wrapper that stores it; insert it via `app.insert_non_send_resource(...)`.
- The root `Cargo.toml` workspace uses `members = ["crates/*"]`, so a new `crates/tmux_session` directory is auto-included as a workspace member. The binary still needs an explicit path dependency added under `[dependencies]`.
- Repo Rust rules (enforced at review): no `mod.rs`; only `// TODO:` / `// NOTE:` / `// SAFETY:` comments; `//!` on every module file; `///` on every `pub` item; all `use` at the top in one block; mutable params before immutable in signatures; private items last in a module; `[lints] workspace = true` in each crate `Cargo.toml`.

## File Structure

- Create `crates/tmux_session/Cargo.toml` — crate manifest (`ozmux_tmux`), deps on `tmux_control`, `bevy`, `tracing`, `crossbeam-channel`.
- Create `crates/tmux_session/src/lib.rs` — module declarations + re-exports (`TmuxSessionPlugin`, `ConnectionState`, `TmuxConnection`).
- Create `crates/tmux_session/src/state.rs` — `ConnectionState` resource enum + the pure `next_state` transition function.
- Create `crates/tmux_session/src/event_pump.rs` — `drain_events` (channel-based core) + `log_transport_event`.
- Create `crates/tmux_session/src/connection.rs` — `TmuxConnection` NonSend resource wrapping `Option<TmuxClient>`.
- Create `crates/tmux_session/src/plugin.rs` — `TmuxSessionPlugin` + the `drain_tmux_events` NonSend system.
- Create `crates/tmux_session/tests/real_tmux.rs` — gated (`#[ignore]`) end-to-end test against a real tmux.
- Modify `Cargo.toml` (root) — add `ozmux_tmux = { path = "crates/tmux_session" }` under `[dependencies]`.
- Modify `src/main.rs` — register `TmuxSessionPlugin`.

---

### Task 1: Create the crate skeleton

**Files:**
- Create: `crates/tmux_session/Cargo.toml`
- Create: `crates/tmux_session/src/lib.rs`

- [ ] **Step 1: Write the crate manifest**

Create `crates/tmux_session/Cargo.toml`:

```toml
[package]
name = "ozmux_tmux"
version.workspace = true
edition.workspace = true
license.workspace = true
readme.workspace = true
authors.workspace = true
publish.workspace = true

[dependencies]
tmux_control = { path = "../tmux_control" }
bevy = { workspace = true }
crossbeam-channel = "0.5"
tracing = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Write a minimal `lib.rs` so the crate compiles**

Create `crates/tmux_session/src/lib.rs`:

```rust
//! ozmux ⇄ tmux control-mode integration: owns a `tmux -CC` connection,
//! drains its transport events into the Bevy world, and tracks the
//! connection lifecycle. Phase 0 wires the plumbing and logs events; it
//! does not project entities or auto-connect.

mod connection;
mod event_pump;
mod plugin;
mod state;

pub use connection::TmuxConnection;
pub use plugin::TmuxSessionPlugin;
pub use state::ConnectionState;
```

- [ ] **Step 3: Create empty module files so the crate resolves**

Create `crates/tmux_session/src/state.rs`:

```rust
//! Connection lifecycle state and its transition rules.
```

Create `crates/tmux_session/src/event_pump.rs`:

```rust
//! Draining and logging of tmux transport events.
```

Create `crates/tmux_session/src/connection.rs`:

```rust
//! The `NonSend` resource that owns the live `tmux -CC` client.
```

Create `crates/tmux_session/src/plugin.rs`:

```rust
//! The `TmuxSessionPlugin` and its per-frame event-drain system.
```

- [ ] **Step 4: Verify the crate compiles**

Run: `cargo build -p ozmux_tmux`
Expected: builds. Warnings about unused empty modules are acceptable at this step (the next tasks fill them); there must be **no errors**.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/Cargo.toml crates/tmux_session/src
git commit -m "feat(tmux_session): crate skeleton for the tmux Bevy integration"
```

---

### Task 2: `ConnectionState` + pure `next_state` transitions

**Files:**
- Modify: `crates/tmux_session/src/state.rs`

- [ ] **Step 1: Write the failing tests**

Replace the contents of `crates/tmux_session/src/state.rs` with:

```rust
//! Connection lifecycle state and its transition rules.

use bevy::prelude::Resource;
use tmux_control::{ClientEvent, TransportEvent};

/// The tmux connection lifecycle, surfaced to the rest of the app.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    /// No connection attempt has been made yet.
    #[default]
    Idle,
    /// A `tmux -CC` process has been spawned but no event has arrived yet.
    Connecting,
    /// The transport is live (at least one event has been received).
    Attached,
    /// The transport closed after having been attached.
    Detached,
    /// The transport closed before attaching, or closed abnormally.
    Error {
        /// Human-readable close reason.
        reason: String,
    },
}

/// Returns the next [`ConnectionState`] for `current` given `event`.
///
/// Any protocol event proves the transport is live, so it moves to
/// `Attached`. A close moves to `Detached` if previously attached, or to
/// `Error` otherwise (e.g. a close during `Connecting`).
pub fn next_state(current: &ConnectionState, event: &TransportEvent) -> ConnectionState {
    match event {
        TransportEvent::Protocol(_) => ConnectionState::Attached,
        TransportEvent::Closed { reason } => match current {
            ConnectionState::Attached => ConnectionState::Detached,
            _ => ConnectionState::Error {
                reason: reason.clone(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control::ControlEvent;
    use tmux_control_parser::WindowId;

    fn notification() -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
            window: WindowId(1),
        }))
    }

    #[test]
    fn protocol_event_attaches_from_connecting() {
        assert_eq!(
            next_state(&ConnectionState::Connecting, &notification()),
            ConnectionState::Attached
        );
    }

    #[test]
    fn protocol_event_keeps_attached() {
        assert_eq!(
            next_state(&ConnectionState::Attached, &notification()),
            ConnectionState::Attached
        );
    }

    #[test]
    fn close_after_attached_is_detached() {
        let close = TransportEvent::Closed {
            reason: "eof".to_string(),
        };
        assert_eq!(
            next_state(&ConnectionState::Attached, &close),
            ConnectionState::Detached
        );
    }

    #[test]
    fn close_while_connecting_is_error() {
        let close = TransportEvent::Closed {
            reason: "boom".to_string(),
        };
        assert_eq!(
            next_state(&ConnectionState::Connecting, &close),
            ConnectionState::Error {
                reason: "boom".to_string()
            }
        );
    }
}
```

- [ ] **Step 2: Add the dev-dependency the test needs**

The test references `tmux_control_parser::WindowId`. Add it as a dev-dependency. In `crates/tmux_session/Cargo.toml`, add this block after the `[dependencies]` block:

```toml
[dev-dependencies]
tmux_control_parser = { path = "../tmux_control_parser" }
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p ozmux_tmux state::`
Expected: 4 tests pass (`protocol_event_attaches_from_connecting`, `protocol_event_keeps_attached`, `close_after_attached_is_detached`, `close_while_connecting_is_error`).

(There is no separate "fails first" step here because the implementation and tests are written together in one file; the meaningful verification is that the transition table behaves exactly as the four tests assert. If any fails, fix `next_state` — not the test.)

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/src/state.rs crates/tmux_session/Cargo.toml
git commit -m "feat(tmux_session): ConnectionState and next_state transitions"
```

---

### Task 3: `drain_events` + `log_transport_event`

**Files:**
- Modify: `crates/tmux_session/src/event_pump.rs`

- [ ] **Step 1: Write the failing test (channel-based core)**

Replace the contents of `crates/tmux_session/src/event_pump.rs` with:

```rust
//! Draining and logging of tmux transport events.

use crate::state::{ConnectionState, next_state};
use crossbeam_channel::Receiver;
use tmux_control::{ClientEvent, TransportEvent};

/// Drains every currently-available transport event from `events`, logs
/// each one, and advances `state` through [`next_state`]. Non-blocking:
/// returns once the channel is empty for now.
pub fn drain_events(state: &mut ConnectionState, events: &Receiver<TransportEvent>) {
    while let Ok(event) = events.try_recv() {
        log_transport_event(&event);
        let next = next_state(state, &event);
        if *state != next {
            *state = next;
        }
    }
}

/// Emits a `tracing` line describing a single transport event.
fn log_transport_event(event: &TransportEvent) {
    match event {
        TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, .. }) => {
            tracing::debug!(?id, ok, "tmux command complete");
        }
        TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
            tracing::debug!(?notification, "tmux notification");
        }
        TransportEvent::Closed { reason } => {
            tracing::info!(reason, "tmux transport closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use tmux_control::ControlEvent;
    use tmux_control_parser::WindowId;

    fn notification() -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
            window: WindowId(1),
        }))
    }

    #[test]
    fn drains_until_empty_and_attaches() {
        let (tx, rx) = unbounded();
        tx.send(notification()).unwrap();
        tx.send(notification()).unwrap();
        let mut state = ConnectionState::Connecting;
        drain_events(&mut state, &rx);
        assert_eq!(state, ConnectionState::Attached);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn close_after_attach_transitions_to_detached() {
        let (tx, rx) = unbounded();
        tx.send(notification()).unwrap();
        tx.send(TransportEvent::Closed {
            reason: "eof".to_string(),
        })
        .unwrap();
        let mut state = ConnectionState::Connecting;
        drain_events(&mut state, &rx);
        assert_eq!(state, ConnectionState::Detached);
    }

    #[test]
    fn empty_channel_leaves_state_untouched() {
        let (_tx, rx) = unbounded::<TransportEvent>();
        let mut state = ConnectionState::Idle;
        drain_events(&mut state, &rx);
        assert_eq!(state, ConnectionState::Idle);
    }
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test -p ozmux_tmux event_pump::`
Expected: 3 tests pass (`drains_until_empty_and_attaches`, `close_after_attach_transitions_to_detached`, `empty_channel_leaves_state_untouched`).

- [ ] **Step 3: Commit**

```bash
git add crates/tmux_session/src/event_pump.rs
git commit -m "feat(tmux_session): channel-based drain_events with state transitions"
```

---

### Task 4: `TmuxConnection` NonSend resource

**Files:**
- Modify: `crates/tmux_session/src/connection.rs`

- [ ] **Step 1: Write the resource**

Replace the contents of `crates/tmux_session/src/connection.rs` with:

```rust
//! The `NonSend` resource that owns the live `tmux -CC` client.

use tmux_control::TmuxClient;

/// Owns the live `tmux -CC` connection, if any.
///
/// Held as a Bevy **`NonSend`** resource because [`TmuxClient`] is `Send`
/// but not `Sync` (it owns a `Box<dyn MasterPty + Send>`). Insert it with
/// `app.insert_non_send_resource(TmuxConnection::default())` and access it
/// via `NonSend<TmuxConnection>` / `NonSendMut<TmuxConnection>`.
#[derive(Default)]
pub struct TmuxConnection {
    client: Option<TmuxClient>,
}

impl TmuxConnection {
    /// Installs `client` as the live connection, replacing any prior one.
    pub fn set(&mut self, client: TmuxClient) {
        self.client = Some(client);
    }

    /// Returns the live client, or `None` when disconnected.
    pub fn client(&self) -> Option<&TmuxClient> {
        self.client.as_ref()
    }

    /// Removes and returns the live client, leaving the connection empty.
    pub fn take(&mut self) -> Option<TmuxClient> {
        self.client.take()
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p ozmux_tmux`
Expected: builds with no errors.

(No unit test here: `TmuxClient` cannot be constructed without spawning a real tmux, so `TmuxConnection`'s behavior is exercised by the gated integration test in Task 6. The accessors are trivial wrappers around `Option`.)

- [ ] **Step 3: Commit**

```bash
git add crates/tmux_session/src/connection.rs
git commit -m "feat(tmux_session): TmuxConnection NonSend resource"
```

---

### Task 5: `TmuxSessionPlugin` + drain system + headless app test

**Files:**
- Modify: `crates/tmux_session/src/plugin.rs`

- [ ] **Step 1: Write the plugin, system, and a headless Bevy test**

Replace the contents of `crates/tmux_session/src/plugin.rs` with:

```rust
//! The `TmuxSessionPlugin` and its per-frame event-drain system.

use crate::connection::TmuxConnection;
use crate::event_pump::drain_events;
use crate::state::ConnectionState;
use bevy::prelude::*;

/// Wires the tmux integration into the Bevy app: registers the
/// [`ConnectionState`] resource, the [`TmuxConnection`] `NonSend` resource,
/// and the per-frame drain system. Phase 0 does not auto-connect.
pub struct TmuxSessionPlugin;

impl Plugin for TmuxSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectionState>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, drain_tmux_events);
    }
}

/// Drains the live tmux connection's transport events each frame and
/// advances [`ConnectionState`]. A no-op while disconnected.
fn drain_tmux_events(mut state: ResMut<ConnectionState>, connection: NonSend<TmuxConnection>) {
    if let Some(client) = connection.client() {
        drain_events(&mut state, client.events());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_state_and_stays_idle_without_connection() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.update();
        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Idle
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p ozmux_tmux plugin::`
Expected: 1 test passes (`plugin_registers_state_and_stays_idle_without_connection`). This confirms the plugin builds, registers the resource, and the drain system is a safe no-op with no connection.

- [ ] **Step 3: Run the whole crate's tests + clippy**

Run: `cargo test -p ozmux_tmux && cargo clippy -p ozmux_tmux -- -D warnings`
Expected: all tests pass; clippy reports no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/src/plugin.rs
git commit -m "feat(tmux_session): TmuxSessionPlugin and per-frame drain system"
```

---

### Task 6: Gated real-tmux integration test

**Files:**
- Create: `crates/tmux_session/tests/real_tmux.rs`

- [ ] **Step 1: Write the gated end-to-end test**

Create `crates/tmux_session/tests/real_tmux.rs`:

```rust
//! End-to-end test against a real tmux binary. Gated with `#[ignore]`
//! because it requires `tmux` on `PATH` and spawns a server on a private
//! socket. Run with: `cargo test -p ozmux_tmux --test real_tmux -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{ConnectionState, TmuxConnection, TmuxSessionPlugin};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn attaches_and_drains_events_from_real_tmux() {
    let socket = format!("ozmux-phase0-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.update();
        if *app.world().resource::<ConnectionState>() == ConnectionState::Attached {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert_eq!(
        *app.world().resource::<ConnectionState>(),
        ConnectionState::Attached,
        "should reach Attached after draining real tmux events"
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

- [ ] **Step 2: Verify the test compiles (without running it)**

Run: `cargo test -p ozmux_tmux --test real_tmux --no-run`
Expected: compiles. The test is not executed (it is `#[ignore]`).

- [ ] **Step 3 (optional, requires tmux installed): run the gated test**

Run: `cargo test -p ozmux_tmux --test real_tmux -- --ignored`
Expected: 1 test passes — the app reaches `ConnectionState::Attached`. If `tmux` is not installed, skip this step and note it; do not treat a missing binary as a failure.

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/tests/real_tmux.rs
git commit -m "test(tmux_session): gated real-tmux attach-and-drain integration test"
```

---

### Task 7: Wire the plugin into the binary

**Files:**
- Modify: `Cargo.toml` (root)
- Modify: `src/main.rs`

- [ ] **Step 1: Add the path dependency**

In the root `Cargo.toml`, under `[dependencies]` (alphabetical neighbors are `ozmux_multiplexer` and `serde`), add:

```toml
ozmux_tmux = { path = "crates/tmux_session" }
```

- [ ] **Step 2: Import the plugin in `src/main.rs`**

In `src/main.rs`, in the `use` block, add (next to the other crate plugin imports such as `ozmux_multiplexer::MultiplexerPlugin`):

```rust
use ozmux_tmux::TmuxSessionPlugin;
```

- [ ] **Step 3: Register the plugin**

In `src/main.rs`, add `TmuxSessionPlugin` to the second `.add_plugins((...))` tuple (the one starting with `TerminalHandlePlugin`). Insert it after `MultiplexerPlugin,`:

```rust
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            MultiplexerPlugin,
            TmuxSessionPlugin,
```

- [ ] **Step 4: Verify the whole workspace builds**

Run: `cargo build`
Expected: the `ozmux-gui` binary builds with the new plugin registered. No errors.

- [ ] **Step 5: Verify the full test suite still passes**

Run: `cargo test`
Expected: all existing tests plus the new `ozmux_tmux` unit tests pass. The gated `real_tmux` test is skipped (it is `#[ignore]`).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/main.rs
git commit -m "feat: register TmuxSessionPlugin in the ozmux binary"
```

---

## Done criteria for Phase 0

- `cargo build` builds the binary with `TmuxSessionPlugin` registered.
- `cargo test -p ozmux_tmux` passes (8 unit tests across `state`, `event_pump`, `plugin`).
- `cargo clippy -p ozmux_tmux -- -D warnings` is clean.
- The gated `real_tmux` integration test compiles, and (where tmux is installed) drives a real `tmux -CC` session to `ConnectionState::Attached`.
- The old multiplexer is untouched and the app runs exactly as before; the tmux scaffold is dormant (no auto-connect) and only logs when a connection is installed.

## Subsequent phases (each gets its own plan after this one lands)

This plan covers **Phase 0 only**. The remaining phases from the design spec
(`docs/superpowers/specs/2026-06-14-tmux-multiplexer-migration-design.md`)
will each be planned separately, in order, once the prior phase merges:

- **Phase 1** — Connection lifecycle (auto-connect at boot, tmux-missing error dialog, MRU attach / create-if-none using `TmuxServer::list_sessions`) + projection skeleton (`TmuxSession`/`TmuxWindow`/`TmuxPane` entities from the initial layout; indexed reducer state).
- **Phase 2** — Pane rendering: the new PTY-less `ozma_tty_engine` API (detached constructor + external-chunk ingest/emit + grid-only resize), absolute cell-dim layout, `refresh-client -C` sizing (tmux ≥ 3.2).
- **Phase 3** — Input via `send-keys` (`-K -c` for key-table routing, `-t` for pane input, `-H` for high bytes), GUI-chord interception, click-to-focus, `%layout-change` reconciliation, focus/dim, `%pause`/`%continue` flow control.
- **Phase 4** — Session-picker popup + detach/reconnect overlay + `list-keys -F` keybind mirror.
- **Phase 5** — Remove `crates/multiplexer`, `src/multiplexer`, the Surface/tab layer; rewire/remove action observers; flip tmux to the only boot path.
