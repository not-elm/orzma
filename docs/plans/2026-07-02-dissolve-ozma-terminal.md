# Dissolve `ozma_terminal` into `src/` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete `crates/ozma_terminal` and move its ~1,150 lines into the binary (`src/`), following the input → action architecture, with zero behavior change.

**Architecture:** The crate is emptied leaf-first so every task leaves the whole workspace compiling and tested: first sever the one library consumer (`ozma_webview`), then move the action layer (`paste`, mouse apply events) into a new `src/action/terminal/` domain, then the shared `Clipboard`, then the Default-mode `exit`/`layout` systems, and finally the `OzmaTerminal` marker + spawn types — at which point the crate is deleted. Type and event names are preserved; the bulk of the diff is `use` path rewrites.

**Tech Stack:** Rust edition 2024 (toolchain 1.95), Bevy 0.18 ECS (EntityEvent + observer idiom), `arboard`, `open`, `anyhow`.

**Spec:** `docs/specs/2026-07-02-dissolve-ozma-terminal-design.md` (reviewed and approved).

## Global Constraints

- Zero behavior change. Type names, event names, observer logic, and system scheduling stay exactly as they are.
- Comments: only `// TODO:` / `// NOTE:` / `// SAFETY:` line comments; `//!` file headers required on every new module file; all comments in English (`.claude/rules/rust.md`).
- Imports: one contiguous `use` block at the top of each file, no blank lines between groups, no inline fully-qualified paths in signatures/bodies.
- Visibility: narrowest that compiles — items used only inside their defining module stay private; cross-module items in the binary use `pub(crate)`; nothing in the binary is `pub`.
- Plugin registration: systems/observers are registered by a `Plugin` in the file that defines them; parents aggregate with `add_plugins`; `Plugin::build` bodies are a single method chain.
- No `mod.rs` files — `foo.rs` + `foo/bar.rs` layout only.
- Every task ends with `cargo build` + `cargo test` green and a commit.
- Doc comments from the old crate move verbatim with their items unless a step says otherwise.

---

### Task 1: Re-key `ozma_webview` GC on `TerminalHandle` and drop the `ozma_terminal` dependency

**Files:**
- Modify: `crates/ozma_webview/src/control_plane.rs` (imports at :16, `gc_despawned_surfaces` at :402-424, test at :849-884)
- Modify: `crates/ozma_webview/Cargo.toml` (drop the `ozma_terminal` path dep at :19)
- Modify: `src/mode/default.rs:89-91` (stale `// NOTE:` documenting the gc keying)
- Test: in-file `gc_tests` module of `control_plane.rs`

**Interfaces:**
- Consumes: `TerminalHandle` and `TerminalHandle::detached(cols, rows)` from `ozma_tty_engine` (already a dependency of `ozma_webview`).
- Produces: `ozma_webview` no longer references `ozma_terminal` in any form — later tasks can move/delete the crate without touching `ozma_webview` again.

- [ ] **Step 1: Change the gc test to spawn `TerminalHandle` instead of `OzmaTerminal` (failing test)**

In `crates/ozma_webview/src/control_plane.rs`, inside `mod gc_tests`, replace:

```rust
        let surface = app.world_mut().spawn(OzmaTerminal).id();
```

with:

```rust
        let surface = app.world_mut().spawn(TerminalHandle::detached(4, 2)).id();
```

`gc_tests` starts with `use super::*;`, so `TerminalHandle` resolves once Step 3 adds the import at the file top. Add the import now (Step 3's import change) so the test compiles: in the top-of-file `use` block, replace

```rust
use ozma_terminal::OzmaTerminal;
```

with

```rust
use ozma_tty_engine::TerminalHandle;
```

Leave `gc_despawned_surfaces` itself untouched for now. It still keys on `RemovedComponents<OzmaTerminal>` — which no longer compiles because the import is gone. To keep this a true red/green cycle without a broken build, do the signature change and the test change in the same commit but verify the test-level behavior in Step 2 by reasoning: if you prefer a strictly compiling red state, temporarily keep both imports (`use ozma_terminal::OzmaTerminal;` AND `use ozma_tty_engine::TerminalHandle;`), run Step 2, then finish Step 3.

- [ ] **Step 2: Run the gc test to verify it fails**

Run: `cargo test -p ozma_webview gc_purges_registrations_when_owner_surface_despawns`

Expected: FAIL — the assertion `"despawning the owner surface purges its registrations"` trips, because the spawned surface never carried `OzmaTerminal`, so `RemovedComponents<OzmaTerminal>` never sees the despawn and the registration survives.

- [ ] **Step 3: Re-key `gc_despawned_surfaces` on `TerminalHandle`**

In `crates/ozma_webview/src/control_plane.rs`:

1. Ensure the `use` block has `use ozma_tty_engine::TerminalHandle;` and NOT `use ozma_terminal::OzmaTerminal;` (remove the temporary dual import if you kept one).
2. Replace the doc comment and signature of the gc system:

```rust
/// Purges a despawned surface's dynamic registrations + assets. Keyed on
/// `RemovedComponents<TerminalHandle>` so it fires for every terminal surface
/// (tmux pane or standalone), with no multiplexer dependency.
///
/// # Invariants
/// Must stay ungated and run every frame: `RemovedComponents` buffers clear at
/// end of frame, so a skipped frame leaks registrations + assets. The purge
/// also runs when `ControlPlaneHandle` is absent (token unbinding is then a
/// no-op) — gating it behind the handle would leak in that case.
fn gc_despawned_surfaces(
    mut registry: ResMut<OzmaRegistry>,
    mut closed: RemovedComponents<TerminalHandle>,
    handle: Option<Res<ControlPlaneHandle>>,
    ozma_assets: Res<WebviewAssetRegistryRes>,
) {
```

The body is unchanged. (Every surface carries `TerminalHandle`: tmux panes get it in `attach_tmux_pane_terminal`, standalone shells via `TerminalBundle`; `remove::<TerminalHandle>()` is never called anywhere, so removal ⇔ despawn. Despawns of unregistered entities are no-ops: `remove_by_surface` returns empty and `tokens.remove_entity` retains mismatches.)

- [ ] **Step 4: Run the gc test to verify it passes**

Run: `cargo test -p ozma_webview gc_purges_registrations_when_owner_surface_despawns`
Expected: PASS

- [ ] **Step 5: Drop the `ozma_terminal` dependency from `ozma_webview`**

In `crates/ozma_webview/Cargo.toml`, delete the line:

```toml
ozma_terminal = { path = "../ozma_terminal" }
```

Run: `grep -rn "ozma_terminal" crates/ozma_webview/` — Expected: no matches.

- [ ] **Step 6: Update the stale gc-keying NOTE in `src/mode/default.rs`**

Replace (at ~line 89):

```rust
            // NOTE: bind the token only after a successful spawn. gc keys on
            // RemovedComponents<OzmaTerminal> (never added on the error path),
            // so a pre-spawn bind would leak the token if the spawn failed.
```

with:

```rust
            // NOTE: bind the token only after a successful spawn. gc keys on
            // RemovedComponents<TerminalHandle> (never added on the error path),
            // so a pre-spawn bind would leak the token if the spawn failed.
```

- [ ] **Step 7: Full check and commit**

Run: `cargo build && cargo test -p ozma_webview`
Expected: build OK, all `ozma_webview` tests pass.

```bash
git add crates/ozma_webview/ src/mode/default.rs
git commit -m "refactor(webview): key surface gc on TerminalHandle, drop ozma_terminal dep"
```

---

### Task 2: Move `PasteAction` into `src/action/terminal/paste.rs`

**Files:**
- Create: `src/action/terminal.rs` (domain aggregator)
- Create: `src/action/terminal/paste.rs`
- Modify: `src/action.rs` (register the third domain)
- Modify: `src/input/keyboard.rs:14`, `src/input/default_mode.rs:29` (import rewrites)
- Modify: `crates/ozma_terminal/src/lib.rs` (drop the `action` module)
- Delete: `crates/ozma_terminal/src/action.rs`
- Test: in-file test moves with `paste.rs`

**Interfaces:**
- Consumes (temporarily, until Tasks 4/6): `ozma_terminal::{Clipboard, build_paste_bytes, OzmaTerminal}`; `ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle}`.
- Produces: `crate::action::terminal::PasteAction` (struct with `#[event_target] pub entity: Entity`) — the import target for `src/input/keyboard.rs` and `src/input/default_mode.rs`. `TerminalActionPlugin` (pub(super), aggregated by `ActionPlugin`). `PastePlugin` temporarily owns `init_resource::<Clipboard>()` (Task 4 moves it to `ClipboardPlugin`).

- [ ] **Step 1: Create `src/action/terminal.rs`**

```rust
//! Per-command PTY-level terminal action events: mode-neutral apply observers
//! that write to a terminal surface's handle, backend, or the clipboard. This
//! root aggregates their per-file plugins.

mod paste;

use bevy::prelude::*;

pub(crate) use paste::PasteAction;

/// Aggregates the per-command terminal action plugins.
pub(super) struct TerminalActionPlugin;

impl Plugin for TerminalActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(paste::PastePlugin);
    }
}
```

- [ ] **Step 2: Create `src/action/terminal/paste.rs`**

Move the content of `crates/ozma_terminal/src/action.rs` with visibility demoted and imports adjusted:

```rust
//! Paste action: reads the system clipboard and writes it to the target
//! terminal entity's PTY as (optionally bracketed) paste bytes.

use bevy::prelude::*;
use ozma_terminal::{Clipboard, OzmaTerminal, build_paste_bytes};
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

/// Pastes the system clipboard into the target terminal entity's PTY.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct PasteAction {
    /// The terminal entity to paste into.
    #[event_target]
    pub entity: Entity,
}

/// Registers the paste apply observer and the `Clipboard` resource.
pub(super) struct PastePlugin;

impl Plugin for PastePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Clipboard>().add_observer(on_paste);
    }
}

fn on_paste(
    ev: On<PasteAction>,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer), With<OzmaTerminal>>,
) {
    let Some(text) = clipboard.read() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let Ok((mut handle, mut pty, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    if !handle.is_at_bottom() {
        handle.scroll_to_bottom(&mut coalescer);
    }
    let bracketed = handle.bracketed_paste_enabled();
    let bytes = build_paste_bytes(&text, bracketed);
    if let Err(e) = handle.write(&mut pty, &bytes) {
        tracing::warn!(?e, entity = ?ev.entity, "ozma paste write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_action_on_entity_without_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(PastePlugin);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(PasteAction { entity });
        app.update();
        // Reaching here proves the observer handled the missing-terminal and
        // unavailable/empty-clipboard paths without panicking. Byte correctness
        // is covered by the clipboard `build_paste_bytes_*` tests.
    }
}
```

(The test previously registered `OzmaActionPlugin`; `PastePlugin` is its exact replacement.)

- [ ] **Step 3: Register the domain in `src/action.rs`**

```rust
//! The action layer: per-command `EntityEvent`s and their apply observers,
//! grouped by domain (tmux pane/window ops, shared VI copy-mode ops,
//! PTY-level terminal ops).

pub(crate) mod terminal;
pub(crate) mod tmux;
pub(crate) mod vi;

use bevy::prelude::*;

/// Aggregates the action-layer plugins.
pub(crate) struct ActionPlugin;

impl Plugin for ActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            terminal::TerminalActionPlugin,
            tmux::TmuxActionPlugin,
            vi::ViActionPlugin,
        ));
    }
}
```

- [ ] **Step 4: Rewrite `PasteAction` consumer imports**

| File | Old | New |
|---|---|---|
| `src/input/keyboard.rs:14` | `use ozma_terminal::{OzmaTerminal, PasteAction};` | `use crate::action::terminal::PasteAction;` and `use ozma_terminal::OzmaTerminal;` (two lines, merged into the existing single import block in alphabetical position) |
| `src/input/default_mode.rs:29` | `use ozma_terminal::{OzmaTerminal, PasteAction};` | same split as above |

- [ ] **Step 5: Remove the `action` module from the crate**

In `crates/ozma_terminal/src/lib.rs`:
- delete `mod action;`
- delete `use crate::action::OzmaActionPlugin;`
- delete `pub use action::PasteAction;`
- change the plugin tuple `(ExitPlugin, LayoutPlugin, OzmaActionPlugin, OzmaMousePlugin)` to `(ExitPlugin, LayoutPlugin, OzmaMousePlugin)`

Then: `git rm crates/ozma_terminal/src/action.rs`

- [ ] **Step 6: Build, test, commit**

Run: `cargo build && cargo test`
Expected: all green. In particular `paste_action_on_entity_without_terminal_does_not_panic` now runs from the binary's test set (`cargo test -p ozmux paste_action`).

```bash
git add src/action.rs src/action/terminal.rs src/action/terminal/ src/input/keyboard.rs src/input/default_mode.rs crates/ozma_terminal/
git commit -m "refactor(action): move PasteAction into src/action/terminal/"
```

---

### Task 3: Move the mouse apply events into `src/action/terminal/`

**Files:**
- Create: `src/action/terminal/forward_input.rs`, `src/action/terminal/mouse_write.rs`, `src/action/terminal/selection.rs`, `src/action/terminal/viewport_scroll.rs`, `src/action/terminal/open_uri.rs`
- Modify: `src/action/terminal.rs` (declare modules, re-export events, add the shared `apply_to_terminal` helper, aggregate plugins)
- Modify: `src/input/mouse.rs:17-20`, `src/input/mouse/wheel.rs:19`, `src/input/tmux/forward.rs:6` (import rewrites)
- Modify: `crates/ozma_terminal/src/lib.rs` (drop `mouse` + `hyperlink` modules)
- Delete: `crates/ozma_terminal/src/mouse.rs`, `crates/ozma_terminal/src/hyperlink.rs`
- Test: the three in-file tests from `mouse.rs` move into `mouse_write.rs`, `selection.rs`, `viewport_scroll.rs`

**Interfaces:**
- Consumes (temporarily): `ozma_terminal::{Clipboard, OzmaTerminal}`; `ozma_tty_engine::{Coalescer, Point, PtyHandle, SelectionType, Side, TerminalHandle}`; `ozma_tty_renderer::schema::is_allowed`; the `open` crate.
- Produces: `crate::action::terminal::{TerminalForwardInput, TerminalMouseWrite, TerminalOpenUri, TerminalSelectionClear, TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate, TerminalViewportScroll}` — all `pub(crate)` EntityEvent structs with field sets identical to today's `ozma_terminal` versions (same names, same types). A private `apply_to_terminal` helper in `terminal.rs`, reachable by the child modules as `super::apply_to_terminal`.

- [ ] **Step 1: Extend `src/action/terminal.rs`**

Replace the file with:

```rust
//! Per-command PTY-level terminal action events: mode-neutral apply observers
//! that write to a terminal surface's handle, backend, or the clipboard. This
//! root aggregates their per-file plugins and hosts the shared attached /
//! detached apply helper.

mod forward_input;
mod mouse_write;
mod open_uri;
mod paste;
mod selection;
mod viewport_scroll;

use bevy::prelude::*;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

pub(crate) use forward_input::TerminalForwardInput;
pub(crate) use mouse_write::TerminalMouseWrite;
pub(crate) use open_uri::TerminalOpenUri;
pub(crate) use paste::PasteAction;
pub(crate) use selection::{
    TerminalSelectionClear, TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate,
};
pub(crate) use viewport_scroll::TerminalViewportScroll;

/// Aggregates the per-command terminal action plugins.
pub(super) struct TerminalActionPlugin;

impl Plugin for TerminalActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            mouse_write::MouseWritePlugin,
            open_uri::OpenUriPlugin,
            paste::PastePlugin,
            selection::SelectionPlugin,
            viewport_scroll::ViewportScrollPlugin,
        ));
    }
}

/// Applies one handle-touching mouse op to `entity`, branching on whether
/// the terminal is PTY-attached (apply through the coalescer) or detached
/// (mutate the VT only, then `flush_emit`). `detached` returns whether a
/// frame flush is needed (the write op forwards instead and returns false).
fn apply_to_terminal(
    commands: &mut Commands,
    handle: &mut TerminalHandle,
    pty: Option<Mut<PtyHandle>>,
    coalescer: Option<Mut<Coalescer>>,
    entity: Entity,
    attached: impl FnOnce(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
    detached: impl FnOnce(&mut Commands, &mut TerminalHandle, Entity) -> bool,
) {
    if let (Some(mut pty), Some(mut coalescer)) = (pty, coalescer) {
        attached(handle, &mut pty, &mut coalescer);
    } else if detached(commands, handle, entity) {
        handle.flush_emit(commands, entity);
    }
}
```

(`apply_to_terminal` is private: child modules reach a parent's private items via `super::`, so no wider visibility is needed. `forward_input` has no plugin — it declares an event type whose routing observer is host-owned, in `src/input/tmux/forward.rs`.)

- [ ] **Step 2: Create `src/action/terminal/forward_input.rs`**

```rust
//! Backend-bytes event for PTY-less terminal surfaces; the host owns the
//! observer that routes it to the real backend (`crate::input::tmux::forward`).

use bevy::prelude::*;

/// Terminal input bytes destined for the backend of `entity` (a PTY for a
/// local terminal, or tmux `send-keys` for a control-mode pane). Emitted by the
/// mouse apply observer when the terminal has no `PtyHandle`; the host owns the
/// observer that routes it to the real backend.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalForwardInput {
    /// The terminal entity whose backend should receive `bytes`.
    #[event_target]
    pub entity: Entity,
    /// The raw bytes to deliver to the backend.
    pub bytes: Vec<u8>,
}
```

- [ ] **Step 3: Create `src/action/terminal/mouse_write.rs`**

Move `TerminalMouseWrite` + `on_terminal_mouse_write` + the `detached_write_event_forwards_bytes` test from `crates/ozma_terminal/src/mouse.rs`:

```rust
//! Mouse-report write action: delivers mouse-protocol bytes to a terminal's
//! backend (PTY when attached, `TerminalForwardInput` when detached).

use crate::action::terminal::{TerminalForwardInput, apply_to_terminal};
use bevy::prelude::*;
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

/// Writes mouse-protocol report bytes to `entity`'s backend (PTY when
/// attached, `TerminalForwardInput` when detached).
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalMouseWrite {
    /// The terminal entity whose backend receives `bytes`.
    #[event_target]
    pub entity: Entity,
    /// The report bytes to deliver.
    pub bytes: Vec<u8>,
}

/// Registers the mouse-write apply observer.
pub(super) struct MouseWritePlugin;

impl Plugin for MouseWritePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_mouse_write);
    }
}

/// Applies a `TerminalMouseWrite`: PTY write when attached, otherwise a
/// `TerminalForwardInput` to the host-owned backend router.
fn on_terminal_mouse_write(
    ev: On<TerminalMouseWrite>,
    mut commands: Commands,
    mut terminals: Query<
        (
            &mut TerminalHandle,
            Option<&mut PtyHandle>,
            Option<&mut Coalescer>,
        ),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    apply_to_terminal(
        &mut commands,
        &mut handle,
        pty,
        coalescer,
        ev.entity,
        |handle, pty, _coalescer| {
            if let Err(e) = handle.write(pty, &ev.bytes) {
                tracing::warn!(?e, "ozma mouse pty write failed");
            }
        },
        |commands, _handle, entity| {
            commands.trigger(TerminalForwardInput {
                entity,
                bytes: ev.bytes.clone(),
            });
            false
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_terminal::Clipboard;

    #[test]
    fn detached_write_event_forwards_bytes() {
        #[derive(Resource, Default)]
        struct CapturedForward(Vec<Vec<u8>>);

        let mut app = App::new();
        app.init_resource::<Clipboard>()
            .init_resource::<CapturedForward>()
            .add_observer(on_terminal_mouse_write)
            .add_observer(
                |ev: On<TerminalForwardInput>, mut cap: ResMut<CapturedForward>| {
                    cap.0.push(ev.bytes.clone());
                },
            );

        let handle = TerminalHandle::detached(10, 5);
        let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

        app.world_mut().trigger(TerminalMouseWrite {
            entity,
            bytes: b"\x1b[<0;1;1M".to_vec(),
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<CapturedForward>().0,
            vec![b"\x1b[<0;1;1M".to_vec()],
            "TerminalMouseWrite on a PTY-less OzmaTerminal must emit TerminalForwardInput"
        );
    }
}
```

- [ ] **Step 4: Create `src/action/terminal/selection.rs`**

Move the four selection events, their observers, and the `detached_selection_start_event_sets_selection_and_emits_frame` test. The file follows the exact same pattern as Step 3 — the moved items are, verbatim from `crates/ozma_terminal/src/mouse.rs`: `TerminalSelectionStart` (:41-52), `TerminalSelectionUpdate` (:55-64), `TerminalSelectionClear` (:67-72), `TerminalSelectionCopy` (:75-80), observers `on_terminal_selection_start` (:178-205), `on_terminal_selection_update` (:208-235), `on_terminal_selection_clear` (:238-265), `on_terminal_selection_copy` (:299-310), and the test (:359-400).

File skeleton (bodies verbatim from the source lines above; structs demoted to `pub(crate)`, observers private):

```rust
//! Local-selection actions: start / update / clear a selection on a terminal
//! surface, and copy the current selection to the clipboard.

use crate::action::terminal::apply_to_terminal;
use bevy::prelude::*;
use ozma_terminal::{Clipboard, OzmaTerminal};
use ozma_tty_engine::{Coalescer, Point, PtyHandle, SelectionType, Side, TerminalHandle};

// ... the four pub(crate) event structs, doc comments verbatim ...

/// Registers the selection apply observers.
pub(super) struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_selection_start)
            .add_observer(on_terminal_selection_update)
            .add_observer(on_terminal_selection_clear)
            .add_observer(on_terminal_selection_copy);
    }
}

// ... the four private observers, bodies verbatim ...

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_engine::{Column, Line};

    // detached_selection_start_event_sets_selection_and_emits_frame, verbatim
}
```

- [ ] **Step 5: Create `src/action/terminal/viewport_scroll.rs`**

Move `TerminalViewportScroll` (:83-90), `on_terminal_viewport_scroll` (:268-295), and the `viewport_scroll_event_on_missing_terminal_does_not_panic` test (:402-412). Same pattern:

```rust
//! Viewport scroll action: scrolls a terminal surface's viewport into / out of
//! scrollback.

use crate::action::terminal::apply_to_terminal;
use bevy::prelude::*;
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

// ... pub(crate) TerminalViewportScroll, doc verbatim ...

/// Registers the viewport-scroll apply observer.
pub(super) struct ViewportScrollPlugin;

impl Plugin for ViewportScrollPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_viewport_scroll);
    }
}

// ... private on_terminal_viewport_scroll, body verbatim ...

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_scroll_event_on_missing_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_observer(on_terminal_viewport_scroll);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger(TerminalViewportScroll { entity, lines: 3 });
        app.update();
    }
}
```

(The old test init'd `Clipboard`; this observer never touches it, so the init is dropped here — the resource requirement was an artifact of the old all-in-one module.)

- [ ] **Step 6: Create `src/action/terminal/open_uri.rs`**

Move `TerminalOpenUri` (:94-102), `on_terminal_open_uri` (:315-319), and fold in `try_open_uri` from `crates/ozma_terminal/src/hyperlink.rs` as a private helper:

```rust
//! Hyperlink open action: opens an allowlist-validated URI via the OS default
//! handler, gated on the target terminal still existing.

use bevy::prelude::*;
use ozma_terminal::OzmaTerminal;
use ozma_tty_renderer::schema::is_allowed;

/// Opens `uri` in the host browser / handler, gated on the target terminal
/// still existing.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TerminalOpenUri {
    /// The terminal entity the link belongs to; the open is suppressed if it
    /// no longer exists.
    #[event_target]
    pub entity: Entity,
    /// The URI to open.
    pub uri: String,
}

/// Registers the open-uri apply observer.
pub(super) struct OpenUriPlugin;

impl Plugin for OpenUriPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_terminal_open_uri);
    }
}

/// Applies a `TerminalOpenUri`: opens the link in the host handler, but only
/// while the target terminal still exists — parity with the legacy apply
/// path, which gated every effect behind the target's presence.
fn on_terminal_open_uri(ev: On<TerminalOpenUri>, terminals: Query<(), With<OzmaTerminal>>) {
    if terminals.get(ev.entity).is_ok() {
        try_open_uri(&ev.uri);
    }
}

/// Validates `uri` against the shared allowlist and opens it via the OS default
/// handler. Disallowed URIs are dropped with a debug log.
fn try_open_uri(uri: &str) {
    if !is_allowed(uri) {
        debug!("hyperlink: dropping disallowed uri {}", uri);
        return;
    }
    if let Err(e) = open::that_detached(uri) {
        warn!("hyperlink: failed to open {}: {}", uri, e);
    }
}
```

- [ ] **Step 7: Rewrite consumer imports**

| File | Old | New |
|---|---|---|
| `src/input/mouse.rs:17-20` | `use ozma_terminal::{\n    OzmaTerminal, TerminalMouseWrite, TerminalOpenUri, TerminalSelectionClear,\n    TerminalSelectionCopy, TerminalSelectionStart, TerminalSelectionUpdate,\n};` | `use crate::action::terminal::{\n    TerminalMouseWrite, TerminalOpenUri, TerminalSelectionClear, TerminalSelectionCopy,\n    TerminalSelectionStart, TerminalSelectionUpdate,\n};` plus `use ozma_terminal::OzmaTerminal;` |
| `src/input/mouse/wheel.rs:19` | `use ozma_terminal::{TerminalMouseWrite, TerminalViewportScroll};` | `use crate::action::terminal::{TerminalMouseWrite, TerminalViewportScroll};` |
| `src/input/tmux/forward.rs:6` | `use ozma_terminal::TerminalForwardInput;` | `use crate::action::terminal::TerminalForwardInput;` |

- [ ] **Step 8: Remove the `mouse` and `hyperlink` modules from the crate**

In `crates/ozma_terminal/src/lib.rs`:
- delete `mod hyperlink;` and `mod mouse;`
- delete `use crate::mouse::OzmaMousePlugin;`
- delete the `pub use mouse::{ ... };` re-export block
- change the plugin tuple `(ExitPlugin, LayoutPlugin, OzmaMousePlugin)` to `(ExitPlugin, LayoutPlugin)`

Then: `git rm crates/ozma_terminal/src/mouse.rs crates/ozma_terminal/src/hyperlink.rs`

- [ ] **Step 9: Build, test, commit**

Run: `cargo build && cargo test`
Expected: green; the three moved tests now run in `ozmux` (`cargo test -p ozmux detached_write detached_selection viewport_scroll`).

```bash
git add src/action/terminal.rs src/action/terminal/ src/input/mouse.rs src/input/mouse/wheel.rs src/input/tmux/forward.rs crates/ozma_terminal/
git commit -m "refactor(action): move mouse apply events into src/action/terminal/"
```

---

### Task 4: Move `Clipboard` into `src/clipboard.rs`

**Files:**
- Create: `src/clipboard.rs`
- Modify: `src/main.rs` (declare `mod clipboard;`, register `ClipboardPlugin`)
- Modify: `src/action/terminal/paste.rs` (drop `init_resource`, rewrite import)
- Modify: import rewrites in `src/ui/copy_mode.rs:15`, `src/mode/tmux/copy_mode.rs:20`, `src/input/tmux/input.rs:47`, `src/action/vi/default_mode.rs:10`, `src/action/terminal/selection.rs`, `src/action/terminal/mouse_write.rs` (test), `src/input/mouse/button.rs:491` (test), `src/input/mouse/wheel.rs:273` (test)
- Modify: `crates/ozma_terminal/src/lib.rs` (drop the `clipboard` module)
- Delete: `crates/ozma_terminal/src/clipboard.rs`
- Test: the clipboard unit tests move with the file

**Interfaces:**
- Consumes: `arboard` (already in root `[dependencies]`, Cargo.toml:24), `bevy`, `tracing`.
- Produces: `crate::clipboard::{Clipboard, build_paste_bytes}` — API identical to today (`Clipboard::new/in_memory/write/read`, `build_paste_bytes(text, bracketed) -> Vec<u8>`), demoted to `pub(crate)`. `ClipboardPlugin` (pub(crate)) — sole owner of `init_resource::<Clipboard>()`.

- [ ] **Step 1: Create `src/clipboard.rs`**

Copy `crates/ozma_terminal/src/clipboard.rs` verbatim (261 lines including tests), then apply exactly these changes:

1. Replace the import line `use bevy::ecs::resource::Resource;` with `use bevy::prelude::*;` (the plugin below needs `App`/`Plugin`; `Resource` comes with the prelude).
2. Demote `pub struct Clipboard` → `pub(crate) struct Clipboard`; `pub fn new` / `pub fn in_memory` / `pub fn write` / `pub fn read` → `pub(crate) fn`; `pub fn build_paste_bytes` → `pub(crate) fn build_paste_bytes`.
3. Append after the `Clipboard` impl block (before `build_paste_bytes`):

```rust
/// Registers the shared `Clipboard` resource consumed by the action layer,
/// copy-mode UIs, and tmux paste.
pub(crate) struct ClipboardPlugin;

impl Plugin for ClipboardPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Clipboard>();
    }
}
```

- [ ] **Step 2: Register in `src/main.rs`**

- Add `mod clipboard;` to the module list (alphabetical: after `mod cef_profile;`... exact position between `mod cef_profile;` and `mod configs;`).
- Add `use crate::clipboard::ClipboardPlugin;` to the import block.
- Add `ClipboardPlugin,` to the second `add_plugins` tuple (after `DefaultModePlugin,`).

- [ ] **Step 3: Drop the temporary init from `PastePlugin`**

In `src/action/terminal/paste.rs`:

```rust
impl Plugin for PastePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste);
    }
}
```

Update its doc comment to `/// Registers the paste apply observer.` and its in-file test: replace `app.add_plugins(MinimalPlugins).add_plugins(PastePlugin);` with `app.add_plugins(MinimalPlugins).add_plugins(PastePlugin).init_resource::<Clipboard>();`.

- [ ] **Step 4: Rewrite `Clipboard` imports**

| File | Old | New |
|---|---|---|
| `src/action/terminal/paste.rs` | `use ozma_terminal::{Clipboard, OzmaTerminal, build_paste_bytes};` | `use crate::clipboard::{Clipboard, build_paste_bytes};` + `use ozma_terminal::OzmaTerminal;` |
| `src/action/terminal/selection.rs` | `use ozma_terminal::{Clipboard, OzmaTerminal};` | `use crate::clipboard::Clipboard;` + `use ozma_terminal::OzmaTerminal;` |
| `src/action/terminal/mouse_write.rs` (tests) | `use ozma_terminal::Clipboard;` | `use crate::clipboard::Clipboard;` |
| `src/ui/copy_mode.rs:15` | `use ozma_terminal::Clipboard;` | `use crate::clipboard::Clipboard;` |
| `src/mode/tmux/copy_mode.rs:20` | `use ozma_terminal::Clipboard;` | `use crate::clipboard::Clipboard;` |
| `src/input/tmux/input.rs:47` | `use ozma_terminal::{Clipboard, build_paste_bytes};` | `use crate::clipboard::{Clipboard, build_paste_bytes};` |
| `src/action/vi/default_mode.rs:10` | `use ozma_terminal::Clipboard;` | `use crate::clipboard::Clipboard;` |
| `src/input/mouse/button.rs:491` (test) | `use ozma_terminal::Clipboard;` | `use crate::clipboard::Clipboard;` |
| `src/input/mouse/wheel.rs:273` (test) | `use ozma_terminal::{Clipboard, OzmaTerminal};` | `use crate::clipboard::Clipboard;` + `use ozma_terminal::OzmaTerminal;` |

After the table, run `grep -rn "ozma_terminal::Clipboard\|ozma_terminal::{Clipboard\|Clipboard, build_paste_bytes" src/` and fix any site the table missed the same way. Expected: no `ozma_terminal` clipboard imports remain.

- [ ] **Step 5: Remove the `clipboard` module from the crate**

In `crates/ozma_terminal/src/lib.rs`: delete `mod clipboard;` and `pub use clipboard::{Clipboard, build_paste_bytes};`.
Then: `git rm crates/ozma_terminal/src/clipboard.rs`

- [ ] **Step 6: Build, test, commit**

Run: `cargo build && cargo test`
Expected: green; clipboard tests run in `ozmux` (`cargo test -p ozmux build_paste_bytes`).

```bash
git add src/clipboard.rs src/main.rs src/action/ src/ui/copy_mode.rs src/mode/tmux/copy_mode.rs src/input/ crates/ozma_terminal/
git commit -m "refactor(clipboard): move Clipboard into src/clipboard.rs"
```

---

### Task 5: Move `exit` and `layout` into `src/mode/default/`

**Files:**
- Create: `src/mode/default/exit.rs`, `src/mode/default/layout.rs`
- Modify: `src/mode/default.rs` (declare modules, aggregate plugins)
- Modify: `crates/ozma_terminal/src/lib.rs` (drop both modules)
- Delete: `crates/ozma_terminal/src/exit.rs`, `crates/ozma_terminal/src/layout.rs`
- Test: in-file tests move with each file

**Interfaces:**
- Consumes (temporarily): `ozma_terminal::{OzmaTerminal, cells_for}`; `ozma_tty_engine::{Coalescer, PtyHandle, TerminalChildExit, TerminalHandle}`; `ozma_tty_renderer::TerminalCellMetricsResource`.
- Produces: `DefaultExitPlugin` and `DefaultLayoutPlugin`, both `pub(super)`, aggregated by `DefaultModePlugin`. No public items — these are self-registering leaf plugins.

- [ ] **Step 1: Create `src/mode/default/exit.rs`**

Move `crates/ozma_terminal/src/exit.rs` verbatim (including its test module), with these changes:
- imports: `use ozma_terminal::OzmaTerminal;` replaces `use crate::spawn::OzmaTerminal;` (and the test's `use crate::spawn::OzmaTerminal;` becomes `use ozma_terminal::OzmaTerminal;` — or is covered by `use super::*`).
- rename `ExitPlugin` → `DefaultExitPlugin`, visibility `pub(super)`.
- replace the `//!` header and add the gateway NOTE on the observer:

```rust
//! Child-process exit observer: sends `AppExit` when the shell quits.

use bevy::prelude::*;
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::TerminalChildExit;

/// Registers the shell-exit observer.
pub(super) struct DefaultExitPlugin;

impl Plugin for DefaultExitPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_child_exit);
    }
}

// NOTE: not Default-only despite the module path — detached tmux panes never
// emit TerminalChildExit, but the adopted tmux gateway keeps OzmaTerminal and
// a real PtyHandle, so this observer also fires (alongside
// on_gateway_child_exit) when the gateway shell dies during tmux mode.
fn on_child_exit(
    ev: On<TerminalChildExit>,
    mut exit: MessageWriter<AppExit>,
    terminals: Query<(), With<OzmaTerminal>>,
) {
    if terminals.get(ev.event_target()).is_ok() {
        exit.write(AppExit::Success);
    }
}
```

The `child_exit_sends_app_exit` test moves verbatim below.

- [ ] **Step 2: Create `src/mode/default/layout.rs`**

Move `crates/ozma_terminal/src/layout.rs` verbatim (including tests), with these changes:
- imports: `use crate::spawn::{OzmaTerminal, cells_for};` → `use ozma_terminal::{OzmaTerminal, cells_for};`
- rename `LayoutPlugin` → `DefaultLayoutPlugin`, visibility `pub(super)`.
- add the gateway NOTE directly above `fn resize_to_window(`:

```rust
// NOTE: not Default-only despite the module path — the query needs
// &mut PtyHandle + &mut Coalescer, so detached tmux panes never match, but the
// adopted tmux gateway (OzmaTerminal + real PtyHandle) is then the single
// match: this system keeps resizing the hidden gateway PTY during tmux mode.
// Do not gate it on AppMode::Default without re-examining gateway behavior.
```

Everything else (the `OzmaLastSize` resource, `reset_last_size` observer, the `run_if` chain in `build`, both tests) moves unchanged.

- [ ] **Step 3: Aggregate in `src/mode/default.rs`**

Add module declarations after `mod webview;`:

```rust
mod exit;
mod layout;
mod webview;
```

Extend `DefaultModePlugin::build` (single chain):

```rust
impl Plugin for DefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((exit::DefaultExitPlugin, layout::DefaultLayoutPlugin))
            .add_systems(
                Update,
                ensure_default_mode_ui.run_if(
                    in_state(AppMode::Default).and(not(any_with_component::<DefaultModeUi>)),
                ),
            );
    }
}
```

- [ ] **Step 4: Remove both modules from the crate**

In `crates/ozma_terminal/src/lib.rs`:
- delete `use crate::{exit::ExitPlugin, layout::LayoutPlugin};` and the `mod` declarations for `exit` / `layout`
- the plugin tuple `(ExitPlugin, LayoutPlugin)` is now empty — change the `build` body to:

```rust
    fn build(&self, app: &mut App) {
        app.insert_resource(OzmaTerminalConfig {
            shell: self.config_shell.clone(),
        })
        .add_observer(on_add_inject_render);
    }
```

Then: `git rm crates/ozma_terminal/src/exit.rs crates/ozma_terminal/src/layout.rs`

Check the `DefaultModePlugin` tests in `src/mode/default.rs` still pass — `build_app` registers `DefaultModePlugin`, which now also pulls in the exit/layout plugins; both are inert without terminals/metrics, so no test change is expected.

- [ ] **Step 5: Build, test, commit**

Run: `cargo build && cargo test`
Expected: green; `cargo test -p ozmux child_exit_sends_app_exit ozma_terminal_spawn_resets_last_size` pass in the binary.

```bash
git add src/mode/default.rs src/mode/default/ crates/ozma_terminal/
git commit -m "refactor(mode): move shell exit + window-fill layout into src/mode/default/"
```

---

### Task 6: Move the marker + spawn types; delete `crates/ozma_terminal`

**Files:**
- Create: `src/surface.rs` (marker + render injection), `src/mode/default/spawn.rs` (bundle/options/config)
- Modify: `src/surface_geom.rs` (gains `cells_for`), `src/main.rs`, `src/mode/default.rs`, `src/mode/default/exit.rs`, `src/mode/default/layout.rs`
- Modify (marker import rewrite, `use ozma_terminal::OzmaTerminal;` → `use crate::surface::OzmaTerminal;`): `src/webview_pointer.rs:20`, `src/window_title.rs:9`, `src/input/default_mode.rs:29,321,330,360`, `src/input/hyperlink.rs:27`, `src/input/focus.rs:9`, `src/input/mouse.rs`, `src/input/ime.rs:384`, `src/input/keyboard.rs`, `src/input/mouse/button.rs:492`, `src/input/mouse/wheel.rs:273`, `src/ui/ime_overlay.rs:735,821,897,1018,1165`, `src/mode/default/webview.rs:30`, `src/mode/tmux/render.rs:11`, `src/action/terminal/paste.rs`, `src/action/terminal/selection.rs`, `src/action/terminal/mouse_write.rs`, `src/action/terminal/viewport_scroll.rs`, `src/action/terminal/open_uri.rs`
- Modify (`cells_for` import rewrite): `src/mode/tmux/adopt.rs:21`, `src/mode/tmux/render.rs:11`
- Modify (test plugin swap + stale doc comments): `src/mode/tmux/render.rs` (:177 doc, :723 import, :856/:913/:1648 `add_plugins`), `src/input/hyperlink.rs:12-13` (module doc)
- Modify: root `Cargo.toml` (drop `ozma_terminal` dep, add `anyhow`)
- Delete: `crates/ozma_terminal/` (entire directory; workspace `members` uses the `crates/*` glob, so no members edit is needed)
- Test: `on_add_injects_render_bundle` moves to `src/surface.rs`; `cells_for_divides_and_floors` moves to `src/surface_geom.rs`; shell-resolution tests move to `src/mode/default/spawn.rs`

**Interfaces:**
- Consumes: `ozma_tty_engine::{SpawnOptions, TerminalBundle}`, `ozma_tty_renderer::material::TerminalUiMaterial`, `ozma_tty_renderer::prelude::TerminalRenderBundle`, `anyhow`.
- Produces: `crate::surface::{OzmaTerminal, SurfacePlugin}`; `crate::surface_geom::cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16)`; `crate::mode::default::spawn::{OzmaSpawnOptions, OzmaTerminalBundle, OzmaTerminalConfig}` (pub(crate)); `DefaultSpawnPlugin { shell: Option<String> }` (pub(super)); `DefaultModePlugin` gains a `pub config_shell: Option<String>` field.

- [ ] **Step 1: Create `src/surface.rs`**

```rust
//! Shared terminal-surface identity: the `OzmaTerminal` marker and the
//! render-bundle injection observer, which fire for every surface — tmux
//! panes and the Default-mode shell alike.

use bevy::prelude::*;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;

/// Marker component identifying an Ozma-mode terminal entity.
///
/// One or more entities may carry this marker; mouse input routes to the
/// topmost under the cursor, while keyboard input (raw keys and IME) targets the
/// single entity the host marks `KeyboardFocused`.
#[derive(Component)]
pub(crate) struct OzmaTerminal;

/// Registers the render-bundle injection observer.
pub(crate) struct SurfacePlugin;

impl Plugin for SurfacePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_inject_render);
    }
}

/// Bevy observer that injects a `TerminalRenderBundle` whenever `OzmaTerminal`
/// is added to an entity, allocating the GPU material on demand.
fn on_add_inject_render(
    ev: On<Add, OzmaTerminal>,
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
) {
    let material = materials.add(TerminalUiMaterial::default());
    commands
        .entity(ev.event_target())
        .insert(TerminalRenderBundle::new(material));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_add_injects_render_bundle() {
        use bevy::asset::AssetPlugin;
        use ozma_tty_renderer::schema::TerminalGrid;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_observer(on_add_inject_render);
        let entity = app.world_mut().spawn(OzmaTerminal).id();
        app.update();
        assert!(
            app.world().entity(entity).contains::<TerminalGrid>(),
            "On<Add, OzmaTerminal> must inject TerminalRenderBundle (TerminalGrid)",
        );
    }
}
```

Add `mod surface;` to `src/main.rs`'s module list (between `mod mode;` and `mod surface_geom;`).

- [ ] **Step 2: Move `cells_for` into `src/surface_geom.rs`**

Append to `src/surface_geom.rs` (below `cell_at_local`), and add its unit test to a `#[cfg(test)] mod tests` block (create the block if the file has none):

```rust
/// Computes terminal dimensions in cells from physical pixel size.
///
/// Returns `(cols, rows)`, each clamped to a minimum of 1.
pub(crate) fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}
```

```rust
    #[test]
    fn cells_for_divides_and_floors() {
        assert_eq!(cells_for(800, 600, 8.0, 16.0), (100, 37));
        assert_eq!(cells_for(1, 1, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(0, 0, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(807, 607, 8.0, 16.0), (100, 37));
    }
```

- [ ] **Step 3: Create `src/mode/default/spawn.rs`**

Move the remainder of `crates/ozma_terminal/src/spawn.rs` (bundle, options, config, `resolve_shell`, shell-resolution tests):

```rust
//! Default-mode standalone terminal spawn: the PTY bundle, spawn options, and
//! the shell-override config resource.

use crate::surface::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::{SpawnOptions, TerminalBundle};
use std::path::PathBuf;

/// Shell override resource.
///
/// `None` means fall back to `$SHELL` at spawn time.
#[derive(Resource)]
pub(crate) struct OzmaTerminalConfig {
    /// Optional shell path. When set, overrides `$SHELL` and `/bin/sh`.
    pub shell: Option<String>,
}

/// Options for spawning a standalone Ozma terminal.
#[derive(Default)]
pub(crate) struct OzmaSpawnOptions {
    /// Shell override; `None` falls back to `$SHELL` then `/bin/sh`.
    pub shell: Option<String>,
    /// Working directory for the PTY; `None` inherits the process cwd.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables for the PTY.
    pub env: Vec<(String, String)>,
}

/// Self-contained spawn bundle for a standalone Ozma terminal: the engine PTY
/// bundle, the `OzmaTerminal` marker, and a default full-screen `Node`. The
/// GPU render bundle is injected by `crate::surface`'s add-observer on
/// insertion.
#[derive(Bundle)]
pub(crate) struct OzmaTerminalBundle {
    terminal: TerminalBundle,
    marker: OzmaTerminal,
    node: Node,
}

impl OzmaTerminalBundle {
    /// Spawns the PTY at a provisional 80x24 (the window-fill resize system
    /// corrects it on the first frame) and returns the bundle. Errors when the
    /// PTY fails to spawn.
    pub(crate) fn spawn(opts: OzmaSpawnOptions) -> anyhow::Result<Self> {
        let shell = resolve_shell(
            opts.shell.as_deref(),
            std::env::var("SHELL").ok().as_deref(),
        );
        let terminal = TerminalBundle::spawn_login_shell(SpawnOptions {
            cols: 80,
            rows: 24,
            shell,
            cwd: opts.cwd,
            env: opts.env,
        })?;
        Ok(Self {
            terminal,
            marker: OzmaTerminal,
            node: Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        })
    }
}

/// Inserts the shell-override config resource read by `ensure_default_mode_ui`.
pub(super) struct DefaultSpawnPlugin {
    /// Shell override from the loaded configs; `None` defers to `$SHELL`.
    pub shell: Option<String>,
}

impl Plugin for DefaultSpawnPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(OzmaTerminalConfig {
            shell: self.shell.clone(),
        });
    }
}

/// Resolves the shell path: config → `$SHELL` → `/bin/sh`.
fn resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String {
    config
        .filter(|s| !s.is_empty())
        .or_else(|| env_shell.filter(|s| !s.is_empty()))
        .unwrap_or("/bin/sh")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_resolution_uses_config() {
        assert_eq!(
            resolve_shell(Some("/bin/fish"), Some("/bin/zsh")),
            "/bin/fish"
        );
    }

    #[test]
    fn shell_resolution_falls_back_to_env() {
        assert_eq!(resolve_shell(None, Some("/bin/zsh")), "/bin/zsh");
    }

    #[test]
    fn shell_resolution_falls_back_to_sh() {
        assert_eq!(resolve_shell(None, None), "/bin/sh");
    }
}
```

(`resolve_shell` demotes to private — its only callers are `spawn` and the tests. `cells_for` is NOT here; it went to `surface_geom` in Step 2.)

- [ ] **Step 4: Thread `config_shell` through `DefaultModePlugin`**

In `src/mode/default.rs`:

1. Module list gains `pub(crate) mod spawn;` (`ensure_default_mode_ui` consumes it; `main.rs` does not).
2. Import rewrite: `use ozma_terminal::{OzmaSpawnOptions, OzmaTerminalBundle, OzmaTerminalConfig};` → `use crate::mode::default::spawn::{OzmaSpawnOptions, OzmaTerminalBundle, OzmaTerminalConfig};`
3. Give the plugin its field and register the spawn plugin (also fix the now-stale doc sentence "`OzmaTerminalPlugin` must be added first (it inserts the `OzmaTerminalConfig` this reads)" → "`DefaultSpawnPlugin` (registered below) inserts the `OzmaTerminalConfig` this reads."):

```rust
pub(crate) struct DefaultModePlugin {
    /// Shell override forwarded to `spawn::DefaultSpawnPlugin`.
    pub config_shell: Option<String>,
}

impl Plugin for DefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            spawn::DefaultSpawnPlugin {
                shell: self.config_shell.clone(),
            },
            exit::DefaultExitPlugin,
            layout::DefaultLayoutPlugin,
        ))
        .add_systems(
            Update,
            ensure_default_mode_ui
                .run_if(in_state(AppMode::Default).and(not(any_with_component::<DefaultModeUi>))),
        );
    }
}
```

4. In the in-file tests' `build_app`, `OzmaTerminalConfig` now comes from `spawn`, and `DefaultModePlugin` needs the field. Replace:

```rust
        app.insert_resource(OzmaTerminalConfig { shell: None });
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(DefaultModePlugin);
```

with:

```rust
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(DefaultModePlugin { config_shell: None });
```

(`DefaultSpawnPlugin` inside `DefaultModePlugin` now inserts the config, so the manual `insert_resource` line is dropped.)

- [ ] **Step 5: Update `src/main.rs`**

- Remove `use ozma_terminal::OzmaTerminalPlugin;`.
- Add `mod surface;` (Step 1) and `use crate::surface::SurfacePlugin;`.
- In the plugin list, replace:

```rust
            OzmaTerminalPlugin {
                config_shell: pre_configs.ozma.shell.clone(),
            },
            DefaultModePlugin,
```

with:

```rust
            SurfacePlugin,
            DefaultModePlugin {
                config_shell: pre_configs.ozma.shell.clone(),
            },
```

- [ ] **Step 6: Rewrite all remaining marker / `cells_for` imports**

Mechanical sweep — for every file below, change `use ozma_terminal::OzmaTerminal;` to `use crate::surface::OzmaTerminal;` (both top-of-file and test-module-local `use` statements):

`src/webview_pointer.rs:20`, `src/window_title.rs:9`, `src/input/default_mode.rs` (:29 top + :321/:330/:360 test-local), `src/input/hyperlink.rs:27`, `src/input/focus.rs:9`, `src/input/mouse.rs`, `src/input/ime.rs:384` (test-local), `src/input/keyboard.rs`, `src/input/mouse/button.rs:492` (test-local), `src/input/mouse/wheel.rs:273` (test-local), `src/ui/ime_overlay.rs` (:735/:821/:897/:1018/:1165 test-local), `src/mode/default/webview.rs:30`, and the five `src/action/terminal/*.rs` files from Tasks 2–4.

Special cases:

| File | Old | New |
|---|---|---|
| `src/mode/tmux/render.rs:11` | `use ozma_terminal::{OzmaTerminal, cells_for};` | `use crate::surface::OzmaTerminal;` + `use crate::surface_geom::cells_for;` |
| `src/mode/tmux/adopt.rs:21` | `use ozma_terminal::cells_for;` | `use crate::surface_geom::cells_for;` |
| `src/mode/default/exit.rs` (Task 5) | `use ozma_terminal::OzmaTerminal;` | `use crate::surface::OzmaTerminal;` |
| `src/mode/default/layout.rs` (Task 5) | `use ozma_terminal::{OzmaTerminal, cells_for};` | `use crate::surface::OzmaTerminal;` + `use crate::surface_geom::cells_for;` |

- [ ] **Step 7: Swap the render-test plugin and fix the two stale doc comments**

In `src/mode/tmux/render.rs`:

1. Test import (:723): `use ozma_terminal::OzmaTerminalPlugin;` → `use crate::surface::SurfacePlugin;`
2. Three call sites (:856, :913, :1648): `app.add_plugins(OzmaTerminalPlugin { config_shell: None });` → `app.add_plugins(SurfacePlugin);` (the tests only need the render-injection observer; the config resource and layout/exit systems were never exercised by them — if a test fails on a missing `OzmaTerminalConfig`, add `app.insert_resource(crate::mode::default::spawn::OzmaTerminalConfig { shell: None });` — not expected).
3. Doc comment (:177): "The `On<Add, OzmaTerminal>` observer in `ozma_terminal` injects the" → "The `On<Add, OzmaTerminal>` observer in `crate::surface` injects the"

In `src/input/hyperlink.rs` module doc (:12-13): replace

```rust
//! `crate::input::mouse` (deciders `decide_button`/`resolve_button_event`);
//! `ozma_terminal` keeps only the apply observer (`on_terminal_open_uri` →
//! `try_open_uri`).
```

with

```rust
//! `crate::input::mouse` (deciders `decide_button`/`resolve_button_event`);
//! `crate::action::terminal` keeps only the apply observer
//! (`on_terminal_open_uri` → `try_open_uri`).
```

- [ ] **Step 8: Delete the crate and fix the manifests**

```bash
git rm -r crates/ozma_terminal
```

In the root `Cargo.toml` `[dependencies]`:
- delete `ozma_terminal = { path = "crates/ozma_terminal" }`
- add `anyhow = { workspace = true }` (alphabetical position, right after `ab_glyph`/`arboard`)

(`[workspace] members` uses the `crates/*` glob — no edit needed there.)

- [ ] **Step 9: Full verification**

```bash
grep -rn "ozma_terminal" src/ crates/ Cargo.toml
```
Expected: no matches (docs/ may still mention it historically).

Run: `cargo build && cargo test`
Expected: green across the workspace.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "refactor: dissolve ozma_terminal crate into src/ (surface, spawn, cells_for)"
```

---

### Task 7: Final lint sweep and docs check

**Files:**
- Modify: whatever `just fix-lint` touches; `CLAUDE.md` only if it still references `ozma_terminal` (the current crate map already omits it)

**Interfaces:**
- Consumes: everything above.
- Produces: a clean branch ready for review.

- [ ] **Step 1: Lint + format**

Run: `just fix-lint` (clippy fix + rustfmt + `pnpm lint:fix`)
Expected: exits 0; re-run `cargo test` if it rewrote anything.

- [ ] **Step 2: Docs references**

Run: `grep -rn "ozma_terminal" CLAUDE.md docs/ --include="*.md" | grep -v docs/specs/ | grep -v docs/plans/`
Expected: no live references (historical specs/plans keep theirs). If `CLAUDE.md` matches, update its crate list / module map to drop the crate and mention `src/surface.rs`, `src/clipboard.rs`, `src/action/terminal/`.

- [ ] **Step 3: Verify zero-behavior-change at runtime**

Run: `cargo run` — confirm the shell renders, typing works, paste (Cmd+V) works, selection + copy works, and a `tmux -CC` attach still adopts into tmux mode with panes rendering.

- [ ] **Step 4: Commit any residue**

```bash
git add -A
git commit -m "chore: lint sweep after ozma_terminal dissolution"
```

(Skip the commit if the tree is clean.)
