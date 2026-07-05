# Restore the Default Shell on tmux Detach — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Path note:** this plan's file paths reflect the module layout at the time
> it was written. A later merge of main's #234 ("dissolve `src/mode` into
> feature-first modules") moved them: `src/mode/tmux/adopt.rs` →
> `src/session/tmux/adopt.rs`; `src/mode/default.rs` →
> `src/ui/default_mode.rs`; `src/mode/default/spawn.rs` →
> `src/session/default/spawn.rs`; `src/mode/tmux.rs` →
> `src/session/tmux.rs`; `src/mode/default/layout.rs` →
> `src/session/default/layout.rs`.

**Goal:** When the user detaches from `tmux -CC` (`%exit`), restore the original Default-mode shell terminal — same entity, same still-alive shell process, preserved scrollback — instead of despawning it and spawning a fresh shell.

**Architecture:** Un-adopt in teardown (spec: `docs/specs/2026-07-03-restore-default-shell-on-detach-design.md`). Four work areas: (1) `tmux_control`'s `ProtocolClient` gains an *ended* state with byte-prefix DCS-terminator detection and a residual-byte buffer; (2) `orzma_tty_engine` gains a `ReleaseControlMode` entity event whose observer atomically re-feeds residual + late-captured bytes through the introducer scanner and re-arms `ControlModeWatch`; (3) `tmux_session` observes the same event to strip `TmuxClient`/`TmuxAttached`/`EnumerationState`, and `TmuxClient` gains a `take_residual()` passthrough; (4) the binary's `%exit` teardown becomes a restore path (synthesized detach line, UI restore via a `mode::default` helper, `GatewaySize` reset) while the child-death path keeps today's despawn.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18 ECS (EntityEvent + observer idiom), sans-io protocol client. No new dependencies.

## Global Constraints

- Comments: only `// TODO:` / `// NOTE:` / `// SAFETY:` line comments; `// NOTE:` reserved for critical caveats (`.claude/rules/rust.md`).
- Every externally-`pub` item gets a `///` doc comment; every new module file starts with a `//!` module doc.
- All `use` statements in one contiguous block at the top of the file; no inline fully-qualified paths in signatures/bodies; no glob imports.
- Mutable parameters before immutable ones in every new/changed signature (a fixed leading `On<E>` observer trigger is exempt).
- Visibility: start private, widen only as far as a real caller requires (`pub(super)` → `pub(in path)` → `pub(crate)` → `pub`).
- `Plugin::build` bodies are a single method chain; systems/observers are registered by a `Plugin` defined in the same file that defines them.
- Bevy `Query` params: no `_q` suffix; singular for `.single()`, plural for iteration.
- Commit after every green test cycle. Run `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt` before each commit (or `just fix-lint`).
- Package names for `cargo test -p`: `tmux_control_parser`, `tmux_control`, `orzma_tty_engine`, `orzma_tmux` (crates/tmux_session), `orzma` (root binary).

---

### Task 1: `tmux_control` — ended state + residual buffer

The protocol client currently ignores a DCS terminator only when it arrives as its own newline-delimited line (`crates/tmux_control/src/protocol.rs:160-162`) — which never happens on a real detach: tmux writes `%exit <reason>\n` then `ESC \` with **no trailing newline**, then the client exits and shell-prompt bytes follow on the same stream. This task makes the terminator end the protocol via a byte-prefix check (gated off inside reply blocks) and accumulate everything after it as residual.

**Files:**
- Modify: `crates/tmux_control_parser/src/assembler.rs` (add `is_in_block()`)
- Modify: `crates/tmux_control/src/protocol.rs` (fields, `feed()`, new accessors, tests)

**Interfaces:**
- Consumes: existing `BlockAssembler`, `DCS_TERMINATOR: &[u8] = b"\x1b\\"`.
- Produces (used by Task 3):
  - `ProtocolClient::take_residual(&mut self) -> Vec<u8>`
  - `ProtocolClient::is_ended(&self) -> bool`
  - `feed()` after end: returns `Ok(Vec::new())`, appends all bytes to residual.

- [ ] **Step 1: Add `BlockAssembler::is_in_block`**

In `crates/tmux_control_parser/src/assembler.rs`, after `pub fn feed(...)` (keep `pub` items grouped, before any private items):

```rust
    /// Returns whether a `%begin` block is currently open (its matching
    /// `%end` / `%error` has not yet arrived).
    pub fn is_in_block(&self) -> bool {
        self.open.is_some()
    }
```

Run: `cargo test -p tmux_control_parser`
Expected: PASS (pure addition).

- [ ] **Step 2: Write the failing protocol tests**

In `crates/tmux_control/src/protocol.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn ended_at_terminator_with_glued_residual() {
        let mut c = ProtocolClient::new();
        let events = c
            .feed(b"%exit detached (from session main)\r\n\x1b\\[prompt]$ ")
            .unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::Exit {
                reason: Some("detached (from session main)".into()),
            })]
        );
        assert!(c.is_ended());
        assert_eq!(c.take_residual(), b"[prompt]$ ".to_vec());
    }

    #[test]
    fn terminator_split_across_chunks() {
        let mut c = ProtocolClient::new();
        let events = c.feed(b"%exit\r\n\x1b").unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::Exit { reason: None })]
        );
        assert!(!c.is_ended(), "a lone ESC must not end the stream yet");
        assert!(c.feed(b"\\$ ").unwrap().is_empty());
        assert!(c.is_ended());
        assert_eq!(c.take_residual(), b"$ ".to_vec());
    }

    #[test]
    fn feed_after_end_accumulates_residual_without_events() {
        let mut c = ProtocolClient::new();
        c.feed(b"%exit\r\n\x1b\\").unwrap();
        assert!(c.is_ended());
        assert!(c.feed(b"%window-add @9\r\n").unwrap().is_empty());
        assert_eq!(c.take_residual(), b"%window-add @9\r\n".to_vec());
        assert!(c.take_residual().is_empty(), "take_residual drains once");
    }

    #[test]
    fn terminator_prefixed_block_body_line_does_not_end_stream() {
        let mut c = ProtocolClient::new();
        let events = c
            .feed(b"%begin 1 7 0\n\x1b\\ body line\n%end 1 7 0\n%window-add @1\n")
            .unwrap();
        assert!(!c.is_ended(), "ESC-backslash inside a block is body, not a terminator");
        assert!(matches!(
            events.last(),
            Some(ClientEvent::Notification(ControlEvent::WindowAdd { .. }))
        ));
    }
```

If `ControlEvent`/`WindowAdd` are not already in the test module's scope, add the needed `use` inside `mod tests` (test-local imports are allowed).

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p tmux_control ended_at_terminator terminator_split feed_after_end terminator_prefixed`
Expected: FAIL — `is_ended` / `take_residual` not found.

- [ ] **Step 4: Implement the ended state**

In `crates/tmux_control/src/protocol.rs`:

Add two fields to `ProtocolClient` (the `Default` derive covers them):

```rust
pub struct ProtocolClient {
    assembler: BlockAssembler,
    line_buf: Vec<u8>,
    pending: VecDeque<PendingSlot>,
    next_id: u64,
    next_fence: u64,
    outgoing: Vec<u8>,
    ended: bool,
    residual: Vec<u8>,
}
```

Replace `feed()` (delete the old `if content == DCS_TERMINATOR { continue; }` line check) and update its doc comment:

```rust
    /// Feeds a raw byte chunk; returns the events it produced (possibly empty).
    ///
    /// Splits on `\n` (stripping a trailing `\r`), buffers any incomplete tail,
    /// treats a blank/whitespace-only line outside a block as a no-op (a blank
    /// line inside a block is kept as body), and drives the assembler with each
    /// complete line. Strips the `tmux -CC` DCS introducer from the first line.
    ///
    /// The DCS terminator (`ESC \`) ends the control stream: it is detected as
    /// a byte prefix at a line boundary (tmux writes it with no trailing
    /// newline, directly glued to the post-detach shell bytes), never inside a
    /// reply block. Once ended, `feed` produces no further events and every
    /// byte accumulates as residual (see [`ProtocolClient::take_residual`]).
    pub fn feed(&mut self, bytes: &[u8]) -> TmuxResult<Vec<ClientEvent>> {
        if self.ended {
            self.residual.extend_from_slice(bytes);
            return Ok(Vec::new());
        }
        self.line_buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        loop {
            if !self.assembler.is_in_block() && self.line_buf.starts_with(DCS_TERMINATOR) {
                self.ended = true;
                self.residual
                    .extend_from_slice(&self.line_buf[DCS_TERMINATOR.len()..]);
                self.line_buf.clear();
                break;
            }
            let Some(nl) = self.line_buf.iter().position(|&b| b == b'\n') else {
                break;
            };
            let mut line: Vec<u8> = self.line_buf.drain(..=nl).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let content = line.strip_prefix(DCS_INTRODUCER).unwrap_or(line.as_slice());
            if let Some(event) = self.feed_line(content)? {
                events.push(event);
            }
        }
        Ok(events)
    }
```

Note the split-`ESC` carry needs no extra code: a lone `\x1b` in `line_buf` fails `starts_with(DCS_TERMINATOR)` and contains no `\n`, so it stays buffered until the next chunk completes (or disproves) the terminator.

Add the two accessors after `send_effect` (public items stay grouped above the private helpers):

```rust
    /// Returns whether the control stream has ended (the DCS terminator was
    /// consumed). Once ended, [`ProtocolClient::feed`] produces no further
    /// events and accumulates all bytes as residual.
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Removes and returns the bytes received after the DCS terminator — the
    /// post-detach stream (the shell prompt) that belongs to the terminal,
    /// not the protocol. Empty if the stream has not ended.
    pub fn take_residual(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.residual)
    }
```

- [ ] **Step 5: Update the stale terminator test**

Find `feed_strips_dcs_wrapper` (`crates/tmux_control/src/protocol.rs:485-501`). Its old fixture modeled the terminator as its own CRLF-delimited line (`...%window-add @0\r\n\x1b\\\r\n`), which the spec calls out as not matching the real stream shape. Rewrite the fixture and comment to the glued no-newline form — the terminator directly followed by post-detach bytes with no intervening newline — while keeping the same notification assertion and adding the new ended/residual assertions:

```rust
    #[test]
    fn feed_strips_dcs_wrapper() {
        // Mirrors a real `tmux -CC` startup then detach: the DCS introducer is
        // glued to the first %begin (the launch reply, flags=0, skipped as
        // unsolicited), and the terminator is glued directly to the following
        // bytes with NO newline — the real stream shape, not a CRLF-delimited
        // line.
        let mut c = ProtocolClient::new();
        let events = c
            .feed(b"\x1bP1000p%begin 1 318 0\r\n%end 1 318 0\r\n%window-add @0\r\n\x1b\\$ ")
            .unwrap();
        assert_eq!(
            events,
            vec![ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(0)
            })]
        );
        assert!(c.is_ended(), "the glued terminator ends the stream");
        assert_eq!(c.take_residual(), b"$ ".to_vec());
    }
```

- [ ] **Step 6: Run the full crate's tests**

Run: `cargo test -p tmux_control`
Expected: PASS (all — including previously existing feed tests).

- [ ] **Step 7: Lint + commit**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
git add crates/tmux_control_parser/src/assembler.rs crates/tmux_control/src/protocol.rs
git commit -m "feat(tmux-control): ended-state + residual bytes after the DCS terminator"
```

---

### Task 2: `orzma_tty_engine` — `ReleaseControlMode` event + release observer

The inverse of adoption. A new module defines the event and an observer that, at command-flush time (observers run with exclusive world access, so no PTY bytes can race past): takes the late-captured bytes off `AdoptedControlMode`, routes residual + late bytes through `Handover::scan` (so a fresh `tmux -CC` introducer re-adopts instead of corrupting the VT), ingests VT-bound bytes with the normal flush-or-arm contract, drains control events, removes `AdoptedControlMode`, and re-inserts `ControlModeWatch`.

**Files:**
- Create: `crates/orzma_tty_engine/src/release.rs`
- Modify: `crates/orzma_tty_engine/src/lib.rs` (module decl, export, plugin registration, `ingest_and_flush_or_arm` → `pub(crate)`)

**Interfaces:**
- Consumes: `Handover::scan` (`pub(crate)`), `AdoptedControlMode::take_captured()`, `AdoptedControlMode.captured` (`pub(crate)` field), `ingest_and_flush_or_arm` (lib.rs, currently private), `TerminalHandle::drain_control_events`, `TerminalHandle::detached` + `Coalescer::new()` + `TerminalTitle::default()` (tests).
- Produces (used by Tasks 3 and 5):
  - `pub struct ReleaseControlMode { pub entity: Entity, pub residual: Vec<u8> }` — an `EntityEvent`, exported from `orzma_tty_engine`.
  - Observer behavior: after release, the entity has `ControlModeWatch` and no `AdoptedControlMode` — unless the bytes contained a fresh introducer, in which case it stays adopted (new capture) and `ControlModeDetected` re-fires.

- [ ] **Step 1: Create `release.rs` with the event, plugin, observer, and failing tests**

Create `crates/orzma_tty_engine/src/release.rs`:

```rust
//! Control-mode release: returns an adopted gateway terminal to normal VT
//! feeding when the tmux control stream ends (a detach).

use crate::coalescer::Coalescer;
use crate::control_mode::{AdoptedControlMode, ControlModeDetected, ControlModeWatch, Handover};
use crate::handle::TerminalHandle;
use crate::ingest_and_flush_or_arm;
use crate::title::TerminalTitle;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::prelude::*;

/// Returns an adopted terminal to normal VT feeding.
///
/// `residual` is fed to the terminal ahead of any bytes still buffered on its
/// [`AdoptedControlMode`], both routed through the control-mode introducer
/// scanner — a fresh `tmux -CC` introducer inside the bytes re-adopts the
/// terminal (re-firing [`ControlModeDetected`]) instead of leaking protocol
/// bytes into the VT.
#[derive(EntityEvent)]
pub struct ReleaseControlMode {
    /// The adopted gateway terminal to release.
    #[event_target]
    pub entity: Entity,
    /// Terminal-bound bytes: the caller's synthesized detach line plus
    /// everything the protocol client received after the DCS terminator.
    pub residual: Vec<u8>,
}

/// Registers the release observer.
pub(crate) struct ControlModeReleasePlugin;

impl Plugin for ControlModeReleasePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_release_control_mode);
    }
}

/// Un-adopts the target terminal: feeds residual + late-captured bytes
/// through the introducer scanner into the VT (normal flush-or-arm
/// semantics), drains control events, and swaps `AdoptedControlMode` back to
/// `ControlModeWatch` — unless a fresh introducer keeps it adopted.
fn on_release_control_mode(
    ev: On<ReleaseControlMode>,
    mut commands: Commands,
    mut terminals: Query<(
        &mut TerminalHandle,
        &mut AdoptedControlMode,
        &mut Coalescer,
        &mut TerminalTitle,
    )>,
) {
    let entity = ev.entity;
    let Ok((mut handle, mut adopted, mut coalescer, mut title)) = terminals.get_mut(entity)
    else {
        return;
    };
    let mut bytes = ev.residual.clone();
    bytes.extend_from_slice(&adopted.take_captured());
    let mut watch = ControlModeWatch::default();
    match Handover::scan(&mut watch, &bytes) {
        Handover::NotYet { vt } => {
            ingest_and_flush_or_arm(&mut commands, entity, &mut handle, &mut coalescer, &vt);
            commands
                .entity(entity)
                .remove::<AdoptedControlMode>()
                .insert(watch);
        }
        Handover::Detected { vt, captured } => {
            ingest_and_flush_or_arm(&mut commands, entity, &mut handle, &mut coalescer, &vt);
            // NOTE: stay adopted — the released stream re-entered control mode
            // (a fresh `tmux -CC` inside the residue). Swapping to a watch here
            // would feed the new protocol stream into the VT and corrupt it;
            // keeping the capture and re-firing ControlModeDetected preserves
            // the stream for the next adoption.
            adopted.captured = captured;
            commands.trigger(ControlModeDetected { entity });
        }
    }
    handle.drain_control_events(&mut commands, entity, &mut title);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct DetectedCount(usize);

    fn count_detected(_ev: On<ControlModeDetected>, mut count: ResMut<DetectedCount>) {
        count.0 += 1;
    }

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<DetectedCount>()
            .add_observer(on_release_control_mode)
            .add_observer(count_detected);
        app
    }

    fn spawn_adopted(app: &mut App, captured: &[u8]) -> Entity {
        app.world_mut()
            .spawn((
                TerminalHandle::detached(80, 24),
                AdoptedControlMode::from_captured(captured.to_vec()),
                Coalescer::new(),
                TerminalTitle::default(),
            ))
            .id()
    }

    #[test]
    fn release_swaps_adoption_back_to_watch() {
        let mut app = build_app();
        let entity = spawn_adopted(&mut app, b"late$ ");

        app.world_mut().trigger(ReleaseControlMode {
            entity,
            residual: b"[detached (from session main)]\r\n".to_vec(),
        });
        app.update();

        let world = app.world();
        assert!(
            world.get::<AdoptedControlMode>(entity).is_none(),
            "AdoptedControlMode removed on release"
        );
        assert!(
            world.get::<ControlModeWatch>(entity).is_some(),
            "ControlModeWatch re-armed on release"
        );
        assert_eq!(
            world.resource::<DetectedCount>().0,
            0,
            "no introducer in the bytes, no re-adoption"
        );
    }

    #[test]
    fn release_with_fresh_introducer_stays_adopted_and_refires_detected() {
        let mut app = build_app();
        let entity = spawn_adopted(&mut app, b"");

        app.world_mut().trigger(ReleaseControlMode {
            entity,
            residual: b"[detached]\r\n$ tmux -CC\r\n\x1bP1000p%begin 2\r\n".to_vec(),
        });
        app.update();

        {
            let world = app.world();
            assert!(
                world.get::<AdoptedControlMode>(entity).is_some(),
                "a fresh introducer must keep the terminal adopted"
            );
            assert!(
                world.get::<ControlModeWatch>(entity).is_none(),
                "no watch while adopted"
            );
            assert_eq!(world.resource::<DetectedCount>().0, 1);
        }
        // The new capture must begin at the introducer byte, mirroring the
        // adoption-path contract.
        let captured = app
            .world_mut()
            .get_mut::<AdoptedControlMode>(entity)
            .unwrap()
            .take_captured();
        assert_eq!(captured, b"\x1bP1000p%begin 2\r\n".to_vec());
    }

    #[test]
    fn release_on_non_adopted_entity_is_a_noop() {
        let mut app = build_app();
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(ReleaseControlMode {
            entity,
            residual: b"x".to_vec(),
        });
        app.update();
        assert!(app.world().get::<ControlModeWatch>(entity).is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p orzma_tty_engine release`
Expected: 0 tests run — `test result: ok. 0 passed; 0 failed; ... 0 filtered out` (or similar). `release.rs` exists on disk but is not yet declared as a module, so the crate does not compile it and the name filter matches nothing. This confirms the module isn't wired in yet; Step 3 wires it in.

- [ ] **Step 3: Wire the module into `lib.rs`**

In `crates/orzma_tty_engine/src/lib.rs`:

1. Add `mod release;` to the module list (alphabetical: between `mod raw_write;` and `mod resize;`).
2. Add the export next to the other `pub use` lines: `pub use release::ReleaseControlMode;`.
3. Add the import to the `use` block: `use release::ControlModeReleasePlugin;`.
4. Register the plugin in `TerminalHandlePlugin::build`:

```rust
        app.add_plugins((RawWritePlugin, ResizePlugin, ControlModeReleasePlugin))
```

5. Change `ingest_and_flush_or_arm`'s visibility from private to `pub(crate)`:

```rust
pub(crate) fn ingest_and_flush_or_arm(
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p orzma_tty_engine`
Expected: PASS (the three new `release::tests` plus all existing engine tests).

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
git add crates/orzma_tty_engine/src/release.rs crates/orzma_tty_engine/src/lib.rs
git commit -m "feat(tty-engine): ReleaseControlMode un-adopts a gateway back to VT feeding"
```

---

### Task 3: `tmux_session` — component-strip observer + `take_residual` passthrough

One event, two observers: `tmux_session` registers its own observer on the engine's `ReleaseControlMode` to strip the connection components today's despawn-teardown removed for free. `EnumerationState` is crate-private (auto-required by `TmuxClient`), which is why this lives here, not in the binary. Also adds the `TmuxClient::take_residual()` passthrough the binary needs (its `ProtocolClient` field is private).

**Files:**
- Modify: `crates/tmux_session/src/connection.rs` (passthrough + test)
- Modify: `crates/tmux_session/src/plugin.rs` (observer + registration + tests)

**Interfaces:**
- Consumes: `orzma_tty_engine::ReleaseControlMode` (Task 2), `ProtocolClient::take_residual` (Task 1).
- Produces (used by Task 5):
  - `TmuxClient::take_residual(&mut self) -> Vec<u8>`
  - After a `ReleaseControlMode` flush, the gateway entity has no `TmuxClient`, `TmuxAttached`, or `EnumerationState` (all Update systems gated on `any_with_component::<TmuxClient>` go quiet next frame).

- [ ] **Step 1: Write the failing passthrough test**

In `crates/tmux_session/src/connection.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn take_residual_passes_through_post_exit_bytes() {
        let mut client = TmuxClient::new_adopted();
        client.feed(b"%exit\r\n\x1b\\$ ").expect("feed");
        assert_eq!(client.take_residual(), b"$ ".to_vec());
        assert!(client.take_residual().is_empty(), "drains once");
    }
```

Run: `cargo test -p orzma_tmux take_residual`
Expected: FAIL — no method `take_residual` on `TmuxClient`.

- [ ] **Step 2: Add the passthrough**

In `crates/tmux_session/src/connection.rs`, after `send_effect` (keeping `pub` methods grouped):

```rust
    /// Removes and returns the post-detach bytes the protocol received after
    /// the control stream's DCS terminator — the shell-bound residue to
    /// re-feed into the restored terminal's VT.
    pub fn take_residual(&mut self) -> Vec<u8> {
        self.protocol.take_residual()
    }
```

Run: `cargo test -p orzma_tmux take_residual`
Expected: PASS.

- [ ] **Step 3: Write the failing strip-observer tests**

In `crates/tmux_session/src/plugin.rs`'s `#[cfg(test)] mod tests`, add:

```rust
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

        app.world_mut().trigger(ReleaseControlMode {
            entity: gateway,
            residual: Vec::new(),
        });
        app.update();

        let entity = app.world().entity(gateway);
        assert!(entity.get::<TmuxClient>().is_none(), "TmuxClient stripped");
        assert!(entity.get::<TmuxAttached>().is_none(), "TmuxAttached stripped");
        assert!(
            entity.get::<EnumerationState>().is_none(),
            "EnumerationState stripped"
        );
    }
```

And, next to the existing re-adoption/attach-edge test (the one that despawns `gateway1` and re-adopts `gateway2`, around `crates/tmux_session/src/plugin.rs:985`), add a strip-not-despawn variant that reuses that test's harness (`entry_block()`, `attached_count()`, `enumeration_pending_nonempty()` — copy the setup lines from the existing test verbatim):

```rust
    #[test]
    fn attach_edge_refires_after_component_strip_release() {
        use orzma_tty_engine::ReleaseControlMode;

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

        // First adoption.
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

        // Release-teardown (the detach path — strip components, don't despawn).
        app.world_mut().trigger(ReleaseControlMode {
            entity: gateway,
            residual: Vec::new(),
        });
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

        // Re-adoption on the SAME entity (what on_control_mode_detected does).
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
```

This test uses `app.add_plugins(TmuxSessionPlugin)` (not a bare `App`) so `on_gateway_release` — registered by `TmuxSessionPlugin::build` in Step 5 below — is active without a separate manual registration.

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p orzma_tmux gateway_release attach_edge_refires`
Expected: FAIL — `on_gateway_release` not defined / `ReleaseControlMode` not imported.

- [ ] **Step 5: Implement the observer**

In `crates/tmux_session/src/plugin.rs`:

1. Extend the engine import: `use orzma_tty_engine::{AdoptedControlMode, ReleaseControlMode, TerminalRawWrite};`
2. Import `EnumerationState` is already in scope (line 11).
3. Add the observer (with the private functions, below the exported items):

```rust
/// Strips the connection components from a gateway being released back to a
/// plain terminal on detach: `TmuxClient`, `TmuxAttached`, and the
/// crate-private `EnumerationState`.
///
/// The engine's own `ReleaseControlMode` observer handles the byte-level
/// un-adoption; this observer owns the tmux-side component cleanup the old
/// despawn-teardown got for free. The two observers touch disjoint component
/// sets, so their relative order is irrelevant.
fn on_gateway_release(ev: On<ReleaseControlMode>, mut commands: Commands) {
    commands
        .entity(ev.entity)
        .remove::<(TmuxClient, TmuxAttached, EnumerationState)>();
}
```

4. Register it in `TmuxSessionPlugin::build` by extending the existing chain:

```rust
        app.init_resource::<TmuxProjection>()
            .init_resource::<CopyModeQueries>()
            .init_resource::<TmuxEventBatch>()
            .add_observer(on_gateway_release)
            .add_message::<PaneOutput>()
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p orzma_tmux`
Expected: PASS (both new tests + full crate).

- [ ] **Step 7: Lint + commit**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
git add crates/tmux_session/src/connection.rs crates/tmux_session/src/plugin.rs
git commit -m "feat(tmux-session): strip connection components on ReleaseControlMode"
```

---

### Task 4: `mode::default` — UI restore helper

The canonical full-size terminal `Node` and the `DefaultShell` marker are module-private to Default mode, so the UI-restore surface lives here (not ad hoc in `adopt.rs`). Extract the node shape and the container spawn so `ensure_default_mode_ui` and the new restore helper share them.

**Files:**
- Modify: `src/mode/default/spawn.rs` (extract `full_size_node()`)
- Modify: `src/mode/default.rs` (extract `spawn_default_mode_container`, add `restore_default_shell`, test)

**Interfaces:**
- Consumes: existing `DefaultModeUi`, `DefaultShell`, `KeyboardFocused`, `UiRoot`.
- Produces (used by Task 5):
  - `pub(in crate::mode) fn restore_default_shell(commands: &mut Commands, shell: Entity, ui_root: Entity)` in `crate::mode::default` — spawns a fresh `DefaultModeUi` container under `ui_root`, reparents `shell` into it, and inserts the full-size `Node`, `KeyboardFocused`, and `DefaultShell`.

- [ ] **Step 1: Write the failing test**

In `src/mode/default.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn restore_default_shell_rebuilds_container_focus_and_layout() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let ui_root = app.world_mut().spawn((Node::default(), UiRoot)).id();
        // A released gateway: hidden Node from adoption, no container.
        let shell = app
            .world_mut()
            .spawn(Node {
                display: Display::None,
                ..default()
            })
            .id();

        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                restore_default_shell(&mut commands, shell, ui_root);
            })
            .unwrap();

        let world = app.world_mut();
        let container = world
            .query_filtered::<Entity, With<DefaultModeUi>>()
            .single(world)
            .expect("restore spawns exactly one DefaultModeUi container");
        let shell_ref = world.entity(shell);
        assert_eq!(
            shell_ref.get::<ChildOf>().map(|c| c.parent()),
            Some(container),
            "shell reparented under the fresh container"
        );
        assert!(shell_ref.get::<KeyboardFocused>().is_some(), "focus restored");
        assert!(shell_ref.get::<DefaultShell>().is_some(), "marker present");
        let node = shell_ref.get::<Node>().expect("node restored");
        assert_eq!(node.position_type, PositionType::Absolute);
        assert_eq!(node.width, Val::Percent(100.0));
        assert_ne!(node.display, Display::None, "no longer hidden");
    }
```

Run: `cargo test -p orzma restore_default_shell`
Expected: FAIL — `restore_default_shell` / `full_size_node` not defined.

- [ ] **Step 2: Extract `full_size_node` in `spawn.rs`**

In `src/mode/default/spawn.rs`, replace the inline `node:` construction in `OrzmaTerminalBundle::spawn` and add the helper (after the `impl` block, before `DefaultSpawnPlugin`):

```rust
        Ok(Self {
            terminal,
            marker: OrzmaTerminal,
            node: full_size_node(),
        })
```

```rust
/// Full-window absolute layout for the standalone Default-mode terminal.
/// Shared by the spawn bundle and the detach-restore path, which must undo
/// adoption's `Display::None` overwrite with the identical node shape.
pub(super) fn full_size_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        top: Val::Px(0.0),
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        ..default()
    }
}
```

- [ ] **Step 3: Extract the container spawn and add the restore helper in `default.rs`**

In `src/mode/default.rs`:

1. Add `full_size_node` to the file's existing spawn import (keep its current path style): `use crate::mode::default::spawn::{OrzmaSpawnOptions, OrzmaTerminalBundle, OrzmaTerminalConfig, full_size_node};`.

2. In `ensure_default_mode_ui`, replace the inline container spawn (the `commands.spawn((Name::new("Default Mode UI"), ...)).id()` block) with a call: `let mode_ui = spawn_default_mode_container(&mut commands, ui_root);` — keep the `// NOTE:` about spawning the container before the PTY attempt where it is.

3. Add, after the `DefaultModePlugin` impl (exported item before the private helpers):

```rust
/// Restores a released tmux gateway as the Default-mode shell.
///
/// Spawns a fresh `DefaultModeUi` container under `ui_root` and reparents
/// `shell` into it with keyboard focus and the full-size layout — adoption
/// overwrote the shell's `Node` with `Display::None` + defaults, so the full
/// node is re-inserted, not just `display` flipped back.
pub(in crate::mode) fn restore_default_shell(
    commands: &mut Commands,
    shell: Entity,
    ui_root: Entity,
) {
    let mode_ui = spawn_default_mode_container(commands, ui_root);
    commands.entity(shell).insert((
        full_size_node(),
        KeyboardFocused,
        DefaultShell,
        ChildOf(mode_ui),
    ));
}
```

4. Add the shared container helper (with the private items):

```rust
/// Spawns the `DefaultModeUi` container node under `ui_root` and returns it.
fn spawn_default_mode_container(commands: &mut Commands, ui_root: Entity) -> Entity {
    commands
        .spawn((
            Name::new("Default Mode UI"),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            DefaultModeUi,
            ChildOf(ui_root),
        ))
        .id()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p orzma mode::default`
Expected: PASS — the new test plus all existing `mode::default` tests (`spawns_default_mode_ui_once`, `default_shell_survives_mode_roundtrip`, etc.).

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
git add src/mode/default.rs src/mode/default/spawn.rs
git commit -m "feat(mode): restore_default_shell helper for the detach-restore path"
```

---

### Task 5: `adopt.rs` — teardown split (detach restores, death despawns)

The `%exit` path becomes a restore: take the residual from the client, synthesize the `[detached …]` line from the `%exit` reason (tmux never writes that message to the PTY in control mode), trigger `ReleaseControlMode`, restore the UI, reset `GatewaySize`, and keep the `TmuxConnectionReset`/`TmuxConnectionClosed` triggers. The child-death path keeps the despawn.

**Files:**
- Modify: `src/mode/tmux/adopt.rs`

**Interfaces:**
- Consumes: `ReleaseControlMode` (Task 2), `TmuxClient::take_residual` (Task 3), `restore_default_shell` (Task 4), existing `GatewaySize`, `TmuxConnectionReset`, `TmuxConnectionClosed`, `ControlEvent::Exit { reason }`.
- Produces: no new exports — behavior only.

- [ ] **Step 1: Write the failing tests**

In `src/mode/tmux/adopt.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn synthesized_detach_line_formats_reason() {
        assert_eq!(
            synthesized_detach_line(Some("detached (from session main)".into())),
            "[detached (from session main)]\r\n"
        );
        assert_eq!(synthesized_detach_line(None), "[detached]\r\n");
    }

    #[test]
    fn batch_exit_reason_extracts_the_reason() {
        let exit = TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Exit {
            reason: Some("detached (from session main)".into()),
        }));
        assert_eq!(
            batch_exit_reason(std::slice::from_ref(&exit)),
            Some(Some("detached (from session main)".into()))
        );
        let bare = TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Exit {
            reason: None,
        }));
        assert_eq!(batch_exit_reason(std::slice::from_ref(&bare)), Some(None));
        assert_eq!(batch_exit_reason(&[]), None);
    }

    #[test]
    fn exit_notification_restores_gateway_into_default_container() {
        use bevy::ecs::system::RunSystemOnce;
        use crate::mode::default::DefaultModeUi;
        use orzma_tty_engine::ReleaseControlMode;

        let mut app = build_app();
        // Stand-in for tmux_session's on_gateway_release (that observer lives
        // in the orzma_tmux crate and is not registered by AdoptPlugin).
        app.add_observer(|ev: On<ReleaseControlMode>, mut commands: Commands| {
            commands.entity(ev.entity).remove::<TmuxClient>();
        });
        // Stand-in for src/mode/tmux.rs::on_tmux_connection_closed.
        app.add_observer(
            |_: On<TmuxConnectionClosed>, mut next: ResMut<NextState<AppMode>>| {
                next.set(AppMode::Default);
            },
        );

        let (container, gateway) = spawn_gateway_under_container(&mut app);
        app.world_mut().trigger(ControlModeDetected { entity: gateway });
        for _ in 0..3 {
            app.update();
        }
        assert!(
            app.world().get_entity(container).is_err(),
            "adoption despawned the original container"
        );
        app.world_mut().resource_mut::<GatewaySize>().0 = Some((gateway, 100, 37));

        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      mut last: ResMut<GatewaySize>,
                      mut clients: Query<&mut TmuxClient>,
                      ui_root: Query<Entity, With<UiRoot>>| {
                    let mut client = clients.get_mut(gateway).expect("gateway has a client");
                    restore_gateway(
                        &mut commands,
                        &mut *last,
                        &mut *client,
                        gateway,
                        ui_root.single().ok(),
                        Some("detached (from session main)".into()),
                    );
                },
            )
            .unwrap();
        for _ in 0..3 {
            app.update();
        }

        assert!(
            app.world().get_entity(gateway).is_ok(),
            "detach must NOT despawn the gateway"
        );
        assert!(
            app.world().get::<TmuxClient>(gateway).is_none(),
            "connection component stripped via ReleaseControlMode"
        );
        assert!(
            app.world().get::<KeyboardFocused>(gateway).is_some(),
            "keyboard focus restored"
        );
        assert_eq!(
            app.world().resource::<GatewaySize>().0,
            None,
            "GatewaySize reset so a re-adoption at the same size re-emits"
        );
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Default,
            "detach returns to Default mode"
        );
        let world = app.world_mut();
        let containers: Vec<Entity> = world
            .query_filtered::<Entity, With<DefaultModeUi>>()
            .iter(world)
            .collect();
        assert_eq!(containers.len(), 1, "exactly one fresh DefaultModeUi");
        let gateway_ref = world.entity(gateway);
        assert_eq!(
            gateway_ref.get::<ChildOf>().map(|c| c.parent()),
            Some(containers[0]),
            "gateway reparented under the fresh container"
        );
        assert_ne!(
            gateway_ref.get::<Node>().map(|n| n.display),
            Some(Display::None),
            "gateway visible again"
        );
    }

    #[test]
    fn readoption_after_restore_reenters_tmux_on_the_same_entity() {
        use crate::mode::default::DefaultModeUi;
        use orzma_tty_engine::ReleaseControlMode;

        let mut app = build_app();
        app.add_observer(|ev: On<ReleaseControlMode>, mut commands: Commands| {
            commands.entity(ev.entity).remove::<TmuxClient>();
        });
        app.add_observer(
            |_: On<TmuxConnectionClosed>, mut next: ResMut<NextState<AppMode>>| {
                next.set(AppMode::Default);
            },
        );
        let pump = |app: &mut App| {
            for _ in 0..5 {
                app.update();
            }
        };

        // Adopt, restore (as in the previous test, minus assertions)…
        let (_c1, gateway) = spawn_gateway_under_container(&mut app);
        app.world_mut().trigger(ControlModeDetected { entity: gateway });
        pump(&mut app);
        {
            use bevy::ecs::system::RunSystemOnce;
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          mut last: ResMut<GatewaySize>,
                          mut clients: Query<&mut TmuxClient>,
                          ui_root: Query<Entity, With<UiRoot>>| {
                        let mut client = clients.get_mut(gateway).expect("client");
                        restore_gateway(
                            &mut commands,
                            &mut *last,
                            &mut *client,
                            gateway,
                            ui_root.single().ok(),
                            None,
                        );
                    },
                )
                .unwrap();
        }
        pump(&mut app);
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Default
        );

        // …then the restored shell runs `tmux -CC` again: the SAME entity
        // re-adopts.
        app.world_mut().trigger(ControlModeDetected { entity: gateway });
        pump(&mut app);
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Tmux,
            "re-running tmux -CC from the restored shell re-enters Tmux"
        );
        assert!(
            app.world().get::<TmuxClient>(gateway).is_some(),
            "the same entity is the gateway again"
        );
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<DefaultModeUi>>()
                .iter(world)
                .count(),
            0,
            "re-adoption despawned the restored container again"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p orzma adopt`
Expected: FAIL — `synthesized_detach_line`, `batch_exit_reason`, `restore_gateway` not defined.

- [ ] **Step 3: Implement the teardown split**

In `src/mode/tmux/adopt.rs`:

1. Extend imports: add `ReleaseControlMode` to the `orzma_tty_engine` use; add `use crate::mode::default::restore_default_shell;`. (`DefaultModeUi` is already imported.)

2. Replace `batch_has_exit` with `batch_exit_reason`:

```rust
/// Returns the `%exit` notification's reason if `events` contains one
/// (`Some(None)` for a bare `%exit` with no reason text).
fn batch_exit_reason(events: &[TransportEvent]) -> Option<Option<String>> {
    events.iter().find_map(|event| match event {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Exit { reason })) => {
            Some(reason.clone())
        }
        _ => None,
    })
}
```

3. Add the detach-line synthesizer:

```rust
/// Renders the iTerm2-style detach line fed into the restored shell's VT.
///
/// tmux never writes `[detached …]` to the PTY in control mode (that message
/// is the non-control-mode branch of its client), so it is fabricated here
/// from the `%exit` reason.
fn synthesized_detach_line(reason: Option<String>) -> String {
    format!("[{}]\r\n", reason.as_deref().unwrap_or("detached"))
}
```

4. Rewrite `teardown_on_exit_notification` and add `restore_gateway`; rename the old `teardown` to `teardown_despawn` (still used by `on_gateway_child_exit`):

```rust
/// Restores the adopted connection's gateway to the Default shell when tmux
/// emits `%exit` (a detach — the gateway shell process survives).
///
/// Gated on the presence of a [`TmuxClient`] and ordered after the drive chain
/// so the batch holds this frame's freshly-drained transport events. NOTE: on a
/// detach the gateway shell process SURVIVES, so `TerminalChildExit` never fires
/// for it — this `%exit` scan is the only teardown signal in that path.
fn teardown_on_exit_notification(
    mut commands: Commands,
    mut last: ResMut<GatewaySize>,
    mut clients: Query<(Entity, &mut TmuxClient)>,
    ui_root: Query<Entity, With<UiRoot>>,
    batch: Res<TmuxEventBatch>,
) {
    let Some(reason) = batch_exit_reason(batch.events()) else {
        return;
    };
    let Ok((gateway, mut client)) = clients.single_mut() else {
        return;
    };
    restore_gateway(
        &mut commands,
        &mut *last,
        &mut *client,
        gateway,
        ui_root.single().ok(),
        reason,
    );
}

/// Restores `gateway` to the Default shell: releases control mode (feeding the
/// synthesized detach line + post-exit residual back into the VT and re-arming
/// the introducer watch), strips the connection components (via the
/// tmux-session `ReleaseControlMode` observer), rebuilds the Default view, and
/// closes the connection.
fn restore_gateway(
    commands: &mut Commands,
    last: &mut GatewaySize,
    client: &mut TmuxClient,
    gateway: Entity,
    ui_root: Option<Entity>,
    reason: Option<String>,
) {
    let mut residual = synthesized_detach_line(reason).into_bytes();
    residual.extend_from_slice(&client.take_residual());
    commands.trigger(ReleaseControlMode {
        entity: gateway,
        residual,
    });
    if let Some(ui_root) = ui_root {
        restore_default_shell(commands, gateway, ui_root);
    }
    last.0 = None;
    commands.trigger(TmuxConnectionReset);
    commands.trigger(TmuxConnectionClosed);
}

/// Tears the connection down by despawning the gateway (the death path: the
/// gateway's child process exited, so there is no shell left to restore).
/// Despawning ends its PTY (its `Drop` kills the child); the fresh Default
/// shell appears via `ensure_default_mode_ui` on the return to
/// `AppMode::Default`.
///
/// Idempotency is guaranteed by the callers' `With<TmuxClient>` checks: once
/// the gateway is despawned (or released) neither teardown path finds a
/// `TmuxClient`, so neither fires again.
fn teardown_despawn(commands: &mut Commands, gateway: Entity) {
    commands.entity(gateway).despawn();
    commands.trigger(TmuxConnectionReset);
    commands.trigger(TmuxConnectionClosed);
}
```

5. Update `on_gateway_child_exit` to call `teardown_despawn` (same body, renamed callee), and update the module `//!` doc: teardown now has two paths — `%exit` restores the gateway to the Default shell; gateway child-exit despawns it.

6. Update the two stale existing tests:
   - `batch_has_exit_detects_percent_exit` → rewrite against `batch_exit_reason` (covered by the new `batch_exit_reason_extracts_the_reason`; delete the old test).
   - `exit_notification_runs_teardown` → delete (superseded by `exit_notification_restores_gateway_into_default_container`).
   - `gateway_exit_tears_down_connection`, `non_gateway_exit_does_not_tear_down`, `re_adoption_after_teardown_re_enters_tmux`, `re_adopt_while_live_replaces_and_despawns_old_gateway`, `teardown_is_a_noop_without_a_client`: keep — they exercise the death/replace paths, which are unchanged. If `teardown_is_a_noop_without_a_client` fails to compile due to the new system params, registering the system bare still works (all params resolve on an empty world); adjust only if the compiler demands it.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p orzma adopt`
Expected: PASS — all new and surviving tests.

- [ ] **Step 5: Full binary test run**

Run: `cargo test -p orzma`
Expected: PASS. Pay attention to `mode::default` tests — `default_shell_survives_mode_roundtrip` must still pass (the non-adopted roundtrip is untouched).

- [ ] **Step 6: Lint + commit**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
git add src/mode/tmux/adopt.rs
git commit -m "feat(tmux): restore the Default shell on detach instead of despawning it"
```

---

### Task 6: Workspace verification + manual smoke test

**Files:**
- No new files; fixes only if verification fails.

**Interfaces:**
- Consumes: everything above.
- Produces: a verified, lint-clean branch.

- [ ] **Step 1: Full workspace test run**

Run: `cargo test`
Expected: PASS across all crates. Known coupling points to watch if anything fails:
- `tmux_control` tests that fed a terminator mid-stream now end the stream (Task 1 updated `feed_strips_dcs_wrapper`; any other test doing the same needs the same treatment).
- Engine tests around `drain_pty_chunks` are unaffected (the release observer is additive), but the plugin now registers one more sub-plugin.

- [ ] **Step 2: Lint everything**

Run: `just fix-lint`
Expected: no diffs beyond formatting; commit any that appear with `style: fix-lint`.

- [ ] **Step 3: Manual smoke test (the real detach round-trip)**

Run: `cargo run`, then in the app's shell:

1. Run some commands (e.g. `ls`, `echo marker-before-tmux`) to build scrollback.
2. `tmux -CC` → app enters Tmux mode (window bar, panes).
3. Detach with the detach binding (or run `tmux detach-client` from another terminal attached to the same server).
4. Expected: the ORIGINAL shell reappears — `marker-before-tmux` still in scrollback, a `[detached (from session …)]` line rendered, live prompt, typing works.
5. `tmux -CC` again → re-enters Tmux mode. Detach again → restored again.
6. `exit` in the restored shell → the app quits (Default-mode child-exit behavior).
7. Separately: while in Tmux mode, kill the gateway shell process from outside (`kill <pid>` of the shell that ran `tmux -CC`) → the app quits (death path unchanged).

- [ ] **Step 4: Final commit if the smoke test surfaced fixes**

```bash
git add -A && git commit -m "fix(tmux): detach-restore smoke-test fixes"
```

(Skip if nothing changed.)

---

## Deviations & Notes for the Implementer

- **`ev.residual.clone()` in the engine observer:** `On<E>` provides a shared reference to the event; the residual must be cloned (or the event redesigned) — clone is fine, this is a one-shot detach path.
- **One-frame double-`KeyboardFocused`:** after restore, the tmux active pane still carries `KeyboardFocused` until `DespawnOnExit(AppMode::Tmux)` runs on the next-frame state transition; keyboard dispatch's `.single()` no-ops for that frame. Do NOT assert single-focus on the same frame in tests.
- **No resize on restore is expected:** `src/mode/default/layout.rs` keeps resizing the hidden gateway during tmux mode at the identical full-window size; do not add a resize call, and do not assert a `TerminalResize` on restore.
- **`Commands::entity` vs liveness in `on_gateway_release`:** the restore path only triggers `ReleaseControlMode` on a live gateway in the same flush; if a future caller can race a despawn, switch to `commands.get_entity(...)`.
