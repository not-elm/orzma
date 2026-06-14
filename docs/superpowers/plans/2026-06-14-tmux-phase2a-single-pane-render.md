# tmux Phase 2a — Single Pane Render Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a single tmux pane visibly render its `%output` on the GPU grid by attaching a PTY-less `TerminalHandle` + the existing render bundle to each `TmuxPane` entity and routing `%output` into it.

**Architecture:** `ozma_tty_engine` gains a PTY-less `TerminalHandle::detached` constructor and a coalescer-free `flush_emit`. `ozmux_tmux` (crate `ozmux_tmux`, dir `crates/tmux_session`) surfaces `%output` as a Bevy `Message` (`PaneOutput`) and exposes a public `TmuxProjectionSet` ordering label; it stays renderer-free. The binary (`src/`) owns a new `OzmuxTmuxRenderPlugin` that attaches the detached handle + `TerminalRenderBundle` + a full-window `Node` to each `TmuxPane`, then routes `PaneOutput` into the handle (coalesced per pane, one `flush_emit` per pane per frame). tmux always auto-connects; the old multiplexer bootstrap seed is removed.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS, `alacritty_terminal` VT engine, `crossbeam-channel`, tmux control mode (`tmux -CC`).

**Spec:** `docs/superpowers/specs/2026-06-14-tmux-phase2-pane-rendering-design.md` (Phase 2a sections).

**Conventions:** Follow `.claude/rules/rust.md` — no `mod.rs`; comments only `// TODO:` / `// NOTE:` / `// SAFETY:`; `//!` on every module file; doc comments on every `pub` item; all `use` at top in one contiguous block; mutable params before immutable; private items last in a block; minimize visibility.

---

## File Structure

- `crates/ozma_tty_engine/src/handle.rs` — add `detached`, `take_replies`, `flush_emit`; refactor `emit` / `finalize_emit` / `abort_emit_with_no_damage` to drop the `Coalescer` parameter (Tasks 1, 2).
- `crates/ozma_tty_engine/src/plugin.rs` — update the 3 `emit` call sites to disarm the coalescer at the call site (Task 2).
- `crates/tmux_session/src/output.rs` *(new)* — `PaneOutput` message + the pure `collect_pane_outputs` helper (Task 3).
- `crates/tmux_session/src/plugin.rs` — `TmuxProjectionSet`, register message, emit `PaneOutput`, wrap the chain in the set (Task 3).
- `crates/tmux_session/src/lib.rs` — export `PaneOutput`, `TmuxProjectionSet` (Task 3).
- `crates/configs/src/tmux.rs`, `crates/configs/src/raw.rs`, `src/tmux_boot.rs` — remove `auto_connect`; always connect (Task 4).
- `src/bootstrap.rs` — stop seeding the old workspace (Task 5).
- `src/tmux_render.rs` *(new)* — `OzmuxTmuxRenderPlugin`: `attach_tmux_pane_terminal` + `route_tmux_output` (Tasks 6, 7).
- `src/main.rs` — add `mod tmux_render;` and the plugin (Task 8).

---

## Task 1: PTY-less `detached` constructor + `take_replies`

**Files:**
- Modify: `crates/ozma_tty_engine/src/handle.rs`

- [ ] **Step 1: Add `unbounded` to the crossbeam import**

In `crates/ozma_tty_engine/src/handle.rs`, change the existing import line:

```rust
use crossbeam_channel::{Receiver, Sender};
```

to:

```rust
use crossbeam_channel::{Receiver, Sender, unbounded};
```

- [ ] **Step 2: Write the failing test**

Append to the `#[cfg(test)] mod tests { ... }` block at the bottom of `handle.rs`:

```rust
#[test]
fn detached_handle_advances_without_pty() {
    let mut h = TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false)));
    h.advance(b"hi");
    let (cols, rows, _cursor) = h.read_geometry();
    assert_eq!((cols, rows), (20, 5));
    assert!(h.take_replies().is_empty());
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ozma_tty_engine detached_handle_advances_without_pty`
Expected: FAIL — `no function or associated item named 'detached'` / `no method named 'take_replies'`.

- [ ] **Step 4: Add the `detached` constructor and `take_replies`**

In `handle.rs`, inside `impl TerminalHandle { ... }`, immediately AFTER the existing `pub(crate) fn new(...) { ... }` method (keep `detached`/`take_replies` with the other public methods, above the private helpers), add:

```rust
/// Constructs a PTY-less handle: same VT bridge as [`TerminalBundle::spawn`]
/// minus the PTY, child process, and reader thread. Used for terminals
/// whose bytes arrive from an external source (e.g. tmux `%output`).
pub fn detached(cols: u16, rows: u16, gate: Arc<AtomicBool>) -> Self {
    let (reply_tx, reply_rx) = unbounded::<Vec<u8>>();
    let (control_tx, control_rx) = unbounded::<ControlFrame>();
    let listener = TermListener {
        reply_tx,
        control_tx: control_tx.clone(),
    };
    Self::new(cols, rows, listener, reply_rx, control_rx, control_tx, gate)
}

/// Drains and returns any pending alacritty `PtyWrite` reply bytes
/// (DSR / DA answers). A detached handle has no PTY to write them to;
/// the caller forwards them to the external program (tmux input) or
/// discards them.
pub fn take_replies(&self) -> Vec<u8> {
    let mut buf = Vec::new();
    self.drain_replies_into(&mut buf);
    buf
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p ozma_tty_engine detached_handle_advances_without_pty`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/ozma_tty_engine/src/handle.rs
git commit -m "feat(ozma_tty_engine): PTY-less TerminalHandle::detached + take_replies"
```

---

## Task 2: Coalescer-free `flush_emit`

This decouples the emit core from `Coalescer`: `emit` / `finalize_emit` / `abort_emit_with_no_damage` stop taking a `Coalescer`, and the PTY callers disarm the coalescer at the call site. `flush_emit` stages damage and calls `emit` — no coalescer needed (tmux panes carry none).

**Files:**
- Modify: `crates/ozma_tty_engine/src/handle.rs`
- Modify: `crates/ozma_tty_engine/src/plugin.rs`

- [ ] **Step 1: Write the failing test**

Append to `handle.rs`'s `#[cfg(test)] mod tests`:

```rust
#[test]
fn flush_emit_triggers_snapshot_for_detached_handle() {
    use bevy::ecs::system::RunSystemOnce;
    use ozma_tty_renderer::schema::FrameSnapshot;

    #[derive(Resource, Default)]
    struct Hits {
        count: u32,
        cols: u16,
        rows: u16,
    }

    let mut app = App::new();
    app.init_resource::<Hits>();
    app.add_observer(|snap: On<FrameSnapshot>, mut hits: ResMut<Hits>| {
        hits.count += 1;
        hits.cols = snap.cols;
        hits.rows = snap.rows;
    });
    app.world_mut()
        .spawn(TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))));
    app.world_mut()
        .run_system_once(
            |mut commands: Commands, mut q: Query<(Entity, &mut TerminalHandle)>| {
                for (entity, mut handle) in &mut q {
                    handle.advance(b"hello");
                    handle.flush_emit(&mut commands, entity);
                }
            },
        )
        .unwrap();
    app.update();

    let hits = app.world().resource::<Hits>();
    assert_eq!(hits.count, 1, "exactly one snapshot emitted");
    assert_eq!((hits.cols, hits.rows), (20, 5));
}
```

`App`, `Resource`, `ResMut`, `Commands`, `Query`, `Entity`, `On` come from `bevy::prelude::*`; add `use bevy::prelude::*;` at the top of the `mod tests` block if not already present (the existing tests use specific imports — add the glob inside `mod tests`, which is permitted by the import rule's test exception).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozma_tty_engine flush_emit_triggers_snapshot_for_detached_handle`
Expected: FAIL — `no method named 'flush_emit'`.

- [ ] **Step 3: Drop the `Coalescer` parameter from `emit`**

In `handle.rs`, change the `emit` signature and its two internal cleanup-call sites. Current:

```rust
    pub(crate) fn emit(
        &mut self,
        commands: &mut Commands,
        entity: Entity,
        coalescer: &mut Coalescer,
    ) {
        let Some(mut dirty) = self.pending_damage.take() else {
            self.abort_emit_with_no_damage(coalescer);
            return;
        };
```

Replace the signature and the abort call:

```rust
    pub(crate) fn emit(&mut self, commands: &mut Commands, entity: Entity) {
        let Some(mut dirty) = self.pending_damage.take() else {
            self.abort_emit_with_no_damage();
            return;
        };
```

Then, further down in the same `emit` body, change the two `self.finalize_emit(coalescer);` calls (the no-op-skip path and the normal-end path) to `self.finalize_emit();`.

- [ ] **Step 4: Make `finalize_emit` and `abort_emit_with_no_damage` coalescer-free**

Current:

```rust
    fn abort_emit_with_no_damage(&mut self, coalescer: &mut Coalescer) {
        coalescer.disarm();
        self.window_open_mode = None;
    }
```

Replace with:

```rust
    fn abort_emit_with_no_damage(&mut self) {
        self.window_open_mode = None;
    }
```

Current:

```rust
    fn finalize_emit(&mut self, coalescer: &mut Coalescer) {
        self.term.reset_damage();
        coalescer.disarm();
    }
```

Replace with:

```rust
    fn finalize_emit(&mut self) {
        self.term.reset_damage();
    }
```

- [ ] **Step 5: Add `flush_emit`**

In `handle.rs`, inside `impl TerminalHandle`, next to the other public methods (e.g. right after `take_replies` from Task 1), add:

```rust
/// Stages damage from the current `Term` state and emits a frame
/// immediately, with no coalescer. The PTY path coalesces via
/// `Coalescer`; tmux panes (whose `%output` tmux has already batched)
/// call this once per pane per frame after `advance`. Handles the
/// first-emit bootstrap (a blank Initial snapshot on a fresh pane).
pub fn flush_emit(&mut self, commands: &mut Commands, entity: Entity) {
    let mut scratch = std::mem::take(&mut self.scratch_dirty);
    self.pending_damage = Some(DirtyRows::collect(&mut self.term, &mut scratch));
    self.scratch_dirty = scratch;
    self.emit(commands, entity);
}
```

- [ ] **Step 6: Update the 3 `emit` call sites in `plugin.rs`**

In `crates/ozma_tty_engine/src/plugin.rs`, the coalescer is now disarmed at the call site. In `process_pty_chunks`, change:

```rust
        let should_flush = handle.ingest_chunk(&chunk, coalescer);
        if should_flush {
            par_commands.command_scope(|mut commands| {
                handle.emit(&mut commands, entity, coalescer);
            });
        } else {
            coalescer.arm_or_extend(Instant::now());
        }
```

to:

```rust
        let should_flush = handle.ingest_chunk(&chunk, coalescer);
        if should_flush {
            par_commands.command_scope(|mut commands| {
                handle.emit(&mut commands, entity);
            });
            coalescer.disarm();
        } else {
            coalescer.arm_or_extend(Instant::now());
        }
```

In `check_deadline_flush`, change the bootstrap path:

```rust
            if handle.needs_bootstrap_emit() {
                handle.force_bootstrap_damage();
                par_commands.command_scope(|mut commands| {
                    handle.emit(&mut commands, entity, &mut coalescer);
                });
                return;
            }
```

to:

```rust
            if handle.needs_bootstrap_emit() {
                handle.force_bootstrap_damage();
                par_commands.command_scope(|mut commands| {
                    handle.emit(&mut commands, entity);
                });
                coalescer.disarm();
                return;
            }
```

and the deadline path:

```rust
            if let Some(deadline) = coalescer.next_deadline()
                && now >= deadline
            {
                par_commands.command_scope(|mut commands| {
                    handle.emit(&mut commands, entity, &mut coalescer);
                });
            }
```

to:

```rust
            if let Some(deadline) = coalescer.next_deadline()
                && now >= deadline
            {
                par_commands.command_scope(|mut commands| {
                    handle.emit(&mut commands, entity);
                });
                coalescer.disarm();
            }
```

- [ ] **Step 7: Run the engine test suite**

Run: `cargo test -p ozma_tty_engine`
Expected: PASS — including `flush_emit_triggers_snapshot_for_detached_handle` and all existing tests (the PTY path still disarms, now at the call site).

- [ ] **Step 8: Commit**

```bash
git add crates/ozma_tty_engine/src/handle.rs crates/ozma_tty_engine/src/plugin.rs
git commit -m "feat(ozma_tty_engine): coalescer-free flush_emit for PTY-less panes"
```

---

## Task 3: `PaneOutput` message + `TmuxProjectionSet`

**Files:**
- Create: `crates/tmux_session/src/output.rs`
- Modify: `crates/tmux_session/src/plugin.rs`
- Modify: `crates/tmux_session/src/lib.rs`

- [ ] **Step 1: Write the failing test (pure helper)**

Create `crates/tmux_session/src/output.rs`:

```rust
//! The `%output` projection seam: the `PaneOutput` message and the pure
//! helper that extracts pane output from a drained transport batch.

use bevy::prelude::Message;
use tmux_control::{ClientEvent, ControlEvent, TransportEvent};
use tmux_control_parser::PaneId;

/// One batch of bytes tmux emitted for a pane (`%output`). Written by the
/// drain system and consumed by the binary's render layer, which maps
/// `pane` to its `TmuxPane` entity.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct PaneOutput {
    /// tmux pane id (`%N`) the bytes belong to.
    pub pane: PaneId,
    /// Raw VT bytes from `%output`.
    pub data: Vec<u8>,
}

/// Extracts a [`PaneOutput`] for every `%output` notification in a drained
/// transport batch, preserving stream order.
pub(crate) fn collect_pane_outputs(events: &[TransportEvent]) -> Vec<PaneOutput> {
    events
        .iter()
        .filter_map(|event| match event {
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Output {
                pane,
                data,
            })) => Some(PaneOutput {
                pane: *pane,
                data: data.clone(),
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::WindowId;

    #[test]
    fn collects_output_events_in_order_and_skips_others() {
        let events = vec![
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(1),
            })),
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Output {
                pane: PaneId(1),
                data: vec![b'a'],
            })),
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Output {
                pane: PaneId(2),
                data: vec![b'b', b'c'],
            })),
        ];
        let out = collect_pane_outputs(&events);
        assert_eq!(
            out,
            vec![
                PaneOutput {
                    pane: PaneId(1),
                    data: vec![b'a'],
                },
                PaneOutput {
                    pane: PaneId(2),
                    data: vec![b'b', b'c'],
                },
            ]
        );
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/tmux_session/src/lib.rs`, add `mod output;` to the module list (keep alphabetical order — after `mod model;`):

```rust
mod model;
mod output;
mod plugin;
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ozmux_tmux collects_output_events_in_order`
Expected: FAIL — `collect_pane_outputs` exists but `plugin.rs` does not yet compile against it / or PASS for the pure test but the crate won't build until exports are added. If it fails to compile due to unused warnings-as-errors, proceed to the next steps which wire it in.

- [ ] **Step 4: Add `TmuxProjectionSet`, register the message, emit `PaneOutput`**

In `crates/tmux_session/src/plugin.rs`, add one import to the existing contiguous `use` block (leave the existing `use tmux_control::TransportEvent;` as-is — `collect_pane_outputs` encapsulates the `ClientEvent`/`ControlEvent` matching, so no new tmux_control imports are needed):

```rust
use crate::output::{PaneOutput, collect_pane_outputs};
```

Add the `SystemSet` definition (after the `TmuxSessionPlugin` struct doc, before `impl Plugin`):

```rust
/// Ordering label for the tmux drain + reconcile chain. The binary's render
/// systems run `.after(TmuxProjectionSet)` so a freshly-projected pane is
/// attached and its output routed in the same frame the projection spawns it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TmuxProjectionSet;
```

In `impl Plugin for TmuxSessionPlugin`, register the message and wrap the chain in the set. Change:

```rust
            .add_systems(
                Update,
                (
                    drain_tmux_events,
                    reconcile_projection.run_if(resource_exists_and_changed::<ProjectionModel>),
                )
                    .chain(),
            );
```

to:

```rust
            .add_message::<PaneOutput>()
            .add_systems(
                Update,
                (
                    drain_tmux_events,
                    reconcile_projection.run_if(resource_exists_and_changed::<ProjectionModel>),
                )
                    .chain()
                    .in_set(TmuxProjectionSet),
            );
```

- [ ] **Step 5: Emit `PaneOutput` from `drain_tmux_events`**

In `plugin.rs`, add a `MessageWriter<PaneOutput>` parameter to `drain_tmux_events` (mutable params first, before the immutable ones — and after the existing `ResMut`/`NonSendMut` params, which are all mutable, so append it among them):

```rust
fn drain_tmux_events(
    mut state: ResMut<ConnectionState>,
    mut model: ResMut<ProjectionModel>,
    mut enumeration: ResMut<EnumerationState>,
    mut connection: NonSendMut<TmuxConnection>,
    mut pane_output: MessageWriter<PaneOutput>,
) {
```

Then, right after the early `if events.is_empty() { return; }` guard, write the pane outputs:

```rust
    if events.is_empty() {
        return;
    }
    for output in collect_pane_outputs(&events) {
        pane_output.write(output);
    }
```

(The rest of the function — `advance_state`, the close/teardown branch, `apply_events` — is unchanged.)

- [ ] **Step 6: Export from `lib.rs`**

In `crates/tmux_session/src/lib.rs`, add to the `pub use` block:

```rust
pub use output::PaneOutput;
pub use plugin::{TmuxProjectionSet, TmuxSessionPlugin};
```

(Replace the existing `pub use plugin::TmuxSessionPlugin;` line with the combined one above.)

- [ ] **Step 7: Run the tmux crate tests**

Run: `cargo test -p ozmux_tmux`
Expected: PASS — including `collects_output_events_in_order_and_skips_others` and all existing tests.

- [ ] **Step 8: Commit**

```bash
git add crates/tmux_session/src/output.rs crates/tmux_session/src/plugin.rs crates/tmux_session/src/lib.rs
git commit -m "feat(ozmux_tmux): PaneOutput message + TmuxProjectionSet ordering label"
```

---

## Task 4: Always auto-connect (remove `auto_connect`)

**Files:**
- Modify: `crates/configs/src/tmux.rs`
- Modify: `crates/configs/src/raw.rs`
- Modify: `src/tmux_boot.rs`

- [ ] **Step 1: Remove the field from `TmuxConfig` and `TmuxPatch`**

In `crates/configs/src/tmux.rs`:

Remove the `auto_connect` field + doc from `TmuxConfig`:

```rust
pub struct TmuxConfig {
    /// tmux binary to run (looked up on `PATH` unless absolute).
    pub program: String,
    /// Optional named server socket (`tmux -L <name>`); `None` targets the
    /// default server, which is what a normal CLI `tmux` uses.
    pub socket_name: Option<String>,
}
```

Update `Default`:

```rust
impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            program: "tmux".to_string(),
            socket_name: None,
        }
    }
}
```

Remove the field from `TmuxPatch`:

```rust
pub(crate) struct TmuxPatch {
    /// Optional `[tmux].program` override.
    pub program: Option<String>,
    /// Optional `[tmux].socket_name` override.
    pub socket_name: Option<String>,
}
```

Update `apply_to`:

```rust
    pub fn apply_to(self, base: TmuxConfig) -> TmuxConfig {
        TmuxConfig {
            program: self.program.unwrap_or(base.program),
            socket_name: self.socket_name.or(base.socket_name),
        }
    }
```

- [ ] **Step 2: Update the `tmux.rs` tests**

Replace the three tests in `tmux.rs`'s `mod tests` with versions that no longer reference `auto_connect`:

```rust
    #[test]
    fn default_targets_path_tmux_default_socket() {
        let c = TmuxConfig::default();
        assert_eq!(c.program, "tmux");
        assert_eq!(c.socket_name, None);
    }

    #[test]
    fn patch_overrides_set_fields_only() {
        let patched = TmuxPatch {
            program: Some("/opt/tmux".to_string()),
            socket_name: None,
        }
        .apply_to(TmuxConfig::default());
        assert_eq!(patched.program, "/opt/tmux");
        assert_eq!(patched.socket_name, None);
    }

    #[test]
    fn empty_patch_keeps_base() {
        let patched = TmuxPatch::default().apply_to(TmuxConfig::default());
        assert_eq!(patched, TmuxConfig::default());
    }
```

- [ ] **Step 3: Update the `raw.rs` test**

In `crates/configs/src/raw.rs`, change the `tmux_section_merges_from_toml` test (remove the `auto_connect` line and assert):

```rust
    #[test]
    fn tmux_section_merges_from_toml() {
        let toml_str = r#"
[tmux]
program = "/usr/local/bin/tmux"
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.tmux.program, "/usr/local/bin/tmux");
        assert_eq!(merged.tmux.socket_name, None);
    }
```

- [ ] **Step 4: Always connect in `tmux_boot.rs`**

In `src/tmux_boot.rs`, remove the `auto_connect` gate. Change:

```rust
    let cfg = &configs.tmux;
    if !cfg.auto_connect {
        return;
    }
    let mut server = TmuxServer::new().program(&cfg.program);
```

to:

```rust
    let cfg = &configs.tmux;
    let mut server = TmuxServer::new().program(&cfg.program);
```

- [ ] **Step 5: Fix the `tmux_boot.rs` test**

The `stays_idle_when_auto_connect_disabled` test (in `src/tmux_boot.rs`) is now invalid — without a real tmux/config it would attempt to connect. Replace it with a test that asserts the boot system runs without panicking when tmux is unavailable (it should land in `Connecting` or `Error`, never `Idle`, since we always attempt):

```rust
    #[test]
    fn boot_attempts_connection_and_leaves_idle_state() {
        let mut app = App::new();
        app.add_plugins((TmuxSessionPlugin, TmuxBootPlugin));
        app.insert_resource(OzmuxConfigsResource::default());
        app.update();
        assert_ne!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Idle,
            "boot always attempts a connection, so it must leave Idle",
        );
    }
```

(`ConnectionState` is already imported in that module via `ozmux_tmux`.)

- [ ] **Step 6: Run the affected tests**

Run: `cargo test -p ozmux_configs && cargo test -p ozmux-gui --lib tmux_boot`
Expected: PASS. (If the binary's test target name differs, run `cargo test` at the workspace root and confirm `tmux_boot` tests pass.)

- [ ] **Step 7: Commit**

```bash
git add crates/configs/src/tmux.rs crates/configs/src/raw.rs src/tmux_boot.rs
git commit -m "feat(tmux): always auto-connect; remove auto_connect config option"
```

---

## Task 5: Remove the old multiplexer bootstrap seed

tmux always owns the window now. The old workspace seed must not render. Keep the cursor-icon system.

**Files:**
- Modify: `src/bootstrap.rs`

- [ ] **Step 1: Stop seeding the workspace**

In `src/bootstrap.rs`, remove the `bootstrap` seeding system and its registration; keep `insert_initial_cursor_icon`. Change the plugin:

```rust
impl Plugin for OzmuxBootstrapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, insert_initial_cursor_icon);
    }
}
```

Delete the `bootstrap` function:

```rust
pub(crate) fn bootstrap(mut mux: MultiplexerCommands) {
    let _ = mux.spawn_attached_workspace();
}
```

Remove the now-unused import `use ozmux_multiplexer::MultiplexerCommands;`.

- [ ] **Step 2: Remove the obsolete bootstrap tests**

In `src/bootstrap.rs`'s `#[cfg(test)] mod tests`, delete the three tests that assert a workspace is seeded: `bootstrap_spawns_workspace_entity_with_attached_marker`, `bootstrap_names_the_initial_workspace_workspace1`, and `bootstrap_attaches_subtree_pointer`. If that leaves `mod tests` empty, delete the entire `#[cfg(test)] mod tests { ... }` block and its `use` lines.

- [ ] **Step 3: Verify the binary builds and the multiplexer still compiles**

Run: `cargo build`
Expected: SUCCESS — no references to `bootstrap` remain. (The old `MultiplexerPlugin` stays added in `main.rs`; it is now dormant with nothing seeded.)

- [ ] **Step 4: Commit**

```bash
git add src/bootstrap.rs
git commit -m "feat(tmux): stop seeding the old multiplexer workspace (tmux owns the window)"
```

---

## Task 6: `attach_tmux_pane_terminal`

Attach a PTY-less handle + render bundle + full-window `Node` to each `TmuxPane`, parented under `WorkspaceUiRoot`.

**Files:**
- Create: `src/tmux_render.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create the render module with the attach system**

Create `src/tmux_render.rs`:

```rust
//! Render layer for tmux panes: attaches a PTY-less `TerminalHandle` plus the
//! GPU render bundle to each projected `TmuxPane`, then routes tmux `%output`
//! into the handle. Lives in the binary so `ozmux_tmux` stays renderer-free.

use crate::ui::WorkspaceUiRoot;
use bevy::prelude::*;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use ozmux_tmux::{PaneOutput, TmuxPane, TmuxProjection, TmuxProjectionSet};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Wires the tmux pane render systems after the projection chain.
pub struct OzmuxTmuxRenderPlugin;

impl Plugin for OzmuxTmuxRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (attach_tmux_pane_terminal, route_tmux_output)
                .chain()
                .after(TmuxProjectionSet),
        );
    }
}

/// Attaches a detached `TerminalHandle`, a `TerminalRenderBundle`, and a
/// full-window absolute `Node` (under `WorkspaceUiRoot`) to each `TmuxPane`
/// that lacks a `TerminalHandle`. Runs every frame but targets each pane
/// exactly once. The grid is sized from the pane's projected `dims`.
fn attach_tmux_pane_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    panes: Query<(Entity, &TmuxPane), Without<TerminalHandle>>,
    ui_root: Query<Entity, With<WorkspaceUiRoot>>,
) {
    let Ok(root) = ui_root.single() else {
        return;
    };
    for (entity, pane) in panes.iter() {
        let cols = pane.dims.width.max(1) as u16;
        let rows = pane.dims.height.max(1) as u16;
        let handle = TerminalHandle::detached(cols, rows, Arc::new(AtomicBool::new(false)));
        let material = materials.add(TerminalUiMaterial::default());
        commands.entity(entity).insert((
            handle,
            TerminalRenderBundle::new(material),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ChildOf(root),
        ));
    }
}

/// Routes tmux `%output` into each pane's handle. Groups a frame's
/// `PaneOutput` messages by pane, advances all of a pane's bytes, then emits
/// once per pane (decision 2: immediate emit, coalesced per pane).
fn route_tmux_output(
    mut commands: Commands,
    mut reader: MessageReader<PaneOutput>,
    mut handles: Query<&mut TerminalHandle>,
    index: Res<TmuxProjection>,
) {
    let mut by_pane: HashMap<_, Vec<u8>> = HashMap::new();
    for msg in reader.read() {
        by_pane
            .entry(msg.pane)
            .or_default()
            .extend_from_slice(&msg.data);
    }
    for (pane, data) in by_pane {
        let Some(&entity) = index.panes.get(&pane) else {
            continue;
        };
        let Ok(mut handle) = handles.get_mut(entity) else {
            continue;
        };
        handle.advance(&data);
        handle.flush_emit(&mut commands, entity);
    }
}
```

- [ ] **Step 2: Declare the module in `main.rs`**

In `src/main.rs`, add `mod tmux_render;` to the module declarations (near the other `mod` lines, e.g. after `mod tmux_boot;` if present, else among the `mod` block).

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: SUCCESS. (`route_tmux_output` is referenced by the plugin even though it's exercised in Task 8; it compiles now.)

- [ ] **Step 4: Commit**

```bash
git add src/tmux_render.rs src/main.rs
git commit -m "feat(tmux): attach PTY-less handle + render bundle to TmuxPane entities"
```

---

## Task 7: Wire the render plugin + integration test

**Files:**
- Modify: `src/main.rs`
- Modify: `src/tmux_render.rs` (test)

- [ ] **Step 1: Add the plugin to the app**

In `src/main.rs`, add `OzmuxTmuxRenderPlugin` to the second `.add_plugins((...))` tuple, right after `TmuxBootPlugin`:

```rust
            TmuxSessionPlugin,
            TmuxBootPlugin,
            OzmuxTmuxRenderPlugin,
```

Add the import near the other plugin imports:

```rust
use crate::tmux_render::OzmuxTmuxRenderPlugin;
```

- [ ] **Step 2: Write the failing integration test**

Append a `#[cfg(test)] mod tests` block to `src/tmux_render.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_renderer::prelude::TerminalGridPlugin;
    use ozma_tty_renderer::schema::TerminalGrid;
    use ozmux_tmux::PaneOutput;
    use tmux_control_parser::{CellDims, PaneId};

    fn dims() -> CellDims {
        CellDims {
            width: 20,
            height: 5,
            xoff: 0,
            yoff: 0,
        }
    }

    #[test]
    fn output_routed_into_pane_grid_renders_text() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.init_resource::<TmuxProjection>();
        app.add_message::<PaneOutput>();

        // A stand-in WorkspaceUiRoot so `attach`'s `ui_root.single()` resolves
        // (kept separate from the pane so the pane is not self-parented).
        app.world_mut().spawn((Node::default(), WorkspaceUiRoot));

        // A projected pane entity + its index mapping.
        let pane_id = PaneId(1);
        let pane_entity = app
            .world_mut()
            .spawn(TmuxPane {
                id: pane_id,
                dims: dims(),
            })
            .id();
        app.world_mut()
            .resource_mut::<TmuxProjection>()
            .panes
            .insert(pane_id, pane_entity);

        app.add_systems(Update, (attach_tmux_pane_terminal, route_tmux_output).chain());

        // Frame 1: attach the handle (no output yet).
        app.update();
        assert!(
            app.world().get::<TerminalHandle>(pane_entity).is_some(),
            "handle attached on first frame",
        );

        // Frame 2: deliver output and route it.
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"hi".to_vec(),
            });
        app.update();

        let grid = app
            .world()
            .get::<TerminalGrid>(pane_entity)
            .expect("pane has a TerminalGrid");
        let row0: String = grid.cells[0].iter().map(|c| c.text.as_str()).collect();
        assert!(
            row0.starts_with("hi"),
            "rendered grid row 0 should start with 'hi', got {row0:?}",
        );
    }
}
```

NOTE: this test spawns the pane with a `WorkspaceUiRoot` marker directly on a stand-in entity so `attach_tmux_pane_terminal`'s `ui_root.single()` resolves; it exercises attach + route + the renderer's snapshot observer end-to-end without a real tmux connection. `TerminalGridPlugin` is re-exported from `ozma_tty_renderer::prelude` (the `grid` module itself is private), and it registers exactly the `apply_snapshot` / `apply_delta` observers needed to mirror frames into `TerminalGrid`.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ozmux-gui output_routed_into_pane_grid_renders_text`
Expected: FAIL if anything is mis-wired (e.g. handle not attached, or grid empty). If it fails to compile because `TerminalGridPlugin`/`grid` module is private, apply the fallback in the NOTE above, then re-run.

- [ ] **Step 4: Make it pass**

The production code from Tasks 1–6 should already satisfy the test. If the grid is empty because the snapshot fired before the grid existed, confirm the two systems are `.chain()`ed (attach before route) so the grid is inserted before the route system's `flush_emit` triggers the deferred snapshot. Adjust only if red.

- [ ] **Step 5: Run the full workspace test + lint**

Run: `cargo test && cargo clippy --workspace --all-targets`
Expected: PASS, no clippy warnings.

- [ ] **Step 6: Format**

Run: `cargo fmt`
Expected: no diff after commit.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/tmux_render.rs
git commit -m "feat(tmux): route %output into pane grids; wire OzmuxTmuxRenderPlugin"
```

---

## Task 8: Manual verification + final polish

**Files:** none (verification only).

- [ ] **Step 1: Run the app against a real tmux**

Ensure a tmux server is running (`tmux new-session -d`), then:

Run: `cargo run`
Expected: the ozmux window opens and a tmux pane's shell prompt / output renders on the grid. Type in the underlying tmux session (from another client) and confirm output appears. (Input from ozmux is Phase 3 — not expected to work yet.)

- [ ] **Step 2: Confirm tmux-unavailable shows the error dialog**

Run: `OZMUX_CONFIG=/dev/null PATH=/nonexistent cargo run` (or temporarily point `[tmux].program` at a missing binary).
Expected: the existing tmux error dialog appears; no panic; no old-bootstrap terminal (per decision 4).

- [ ] **Step 3: Final full check**

Run: `cargo test && cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: all green.

- [ ] **Step 4: Commit any formatting fixes**

```bash
git add -A
git commit -m "chore(tmux): phase 2a polish" || echo "nothing to commit"
```

---

## Out of scope (Phase 2b / later)

- Multi-pane absolute cell-dim layout + `resize_grid_only` + `refresh-client -C` window sizing — Phase 2b (separate plan).
- Input / reply routing to tmux, focus + dim, click-to-focus — Phase 3.
- OSC 5379 inline webviews on tmux panes — deferred (gate stays off).
