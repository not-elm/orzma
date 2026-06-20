# Reuse ozma_terminal for Ozmux-mode Panes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make each tmux pane a first-class `OzmaTerminal` so selection / copy / hover / hyperlink / mouse-report forwarding are driven by the shared `ozma_terminal` systems, deleting the duplicated arbiter logic in `src/tmux/mouse.rs`.

**Architecture:** Panes gain the `OzmaTerminal` marker, so the crate's already-`AppMode`-independent mouse systems hit-test and drive them. A PTY-less terminal can have no `PtyHandle`/`Coalescer`, so the mouse apply observer becomes optional over them: local effects use the `*_vt_only` handle methods + `flush_emit`, and `MouseEffect::Write` is re-emitted as a new `TerminalForwardInput` `EntityEvent`. A host observer routes that event to tmux via `send_bytes_command` (`send-keys -H`). A host gate maintainer marks panes `KeyboardDisabled` (always) and `MouseDisabled` (modal / copy-mode / webview), and a pre-gate suppresses `ozma_terminal` where tmux-specific gestures (divider, inline-webview) claim a press.

**Tech Stack:** Rust 2024 (toolchain 1.95), Bevy 0.18 ECS (`EntityEvent` + observers), `alacritty_terminal` 0.26 VT, tmux `-CC` control mode.

**Design doc:** `docs/superpowers/specs/2026-06-20-ozmux-ozma-terminal-reuse-design.md` (read it first).

## Global Constraints

- Rust edition 2024, toolchain pinned `1.95`. No `mod.rs` module roots.
- Comments only `// TODO:` / `// NOTE:` / `// SAFETY:`, English only.
- Every externally-`pub` item has a `///` doc; every module file has a `//!`.
- All `use` in one contiguous top block; no inline fully-qualified paths.
- Bevy: mutable `SystemParam`s before immutable; `Plugin::build` one method chain; whole-system change gates via `run_if`; `Query` params descriptive nouns (no `_q`); no manual `set_changed()` / `bypass_change_detection()`.
- Visibility minimized (private unless a cross-module caller forces wider); private items last in a block; `#[expect(reason=…)]` over `#[allow]`.
- Lint/format gate for every "Commit" step: `cargo clippy --workspace --all-targets && cargo fmt --check`.

## File Structure

| File | Responsibility | Tasks |
| --- | --- | --- |
| `crates/ozma_terminal/src/mouse.rs` | Add `TerminalForwardInput`; PtyHandle/Coalescer-optional apply observer; `apply_effect_detached` | 1 |
| `crates/ozma_terminal/src/lib.rs` | Export `TerminalForwardInput` | 1 |
| `src/tmux/forward.rs` (new) | `forward_pane_input` observer: `TerminalForwardInput` → `send-keys -H`; `ForwardPlugin` | 2 |
| `src/tmux/gate.rs` (new) | `maintain_tmux_input_gates` (Keyboard/MouseDisabled) + pre-gate claim; `GatePlugin` | 3 |
| `src/tmux/render.rs` | Attach `OzmaTerminal`; drop the tmux-side `TerminalRenderBundle` insert | 4 |
| `src/tmux/mouse.rs` | Delete local-selection / multi-click copy / hover / hyperlink; keep select-pane / divider / copy-mode / inline-webview | 5 |
| `src/tmux/input.rs` | Narrow `forward_wheel_to_tmux` (copy-mode + alt-screen-`!1007` + inline only) | 6 |
| `src/tmux.rs` | Register `ForwardPlugin`, `GatePlugin` | 2, 3 |
| `crates/tmux_session/tests/real_tmux_*.rs` | DECSET-in-`%output` integration test | 7 |

---

### Task 1: Crate sink seam — `TerminalForwardInput` + PtyHandle-optional apply observer

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs` (event def near `TerminalMouseEffects` ~257; observer `on_terminal_mouse_effects` ~776; `apply_effect` ~789)
- Modify: `crates/ozma_terminal/src/lib.rs:21-24` (exports)
- Test: `crates/ozma_terminal/src/mouse.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `pub struct TerminalForwardInput { #[event_target] pub entity: Entity, pub bytes: Vec<u8> }` (EntityEvent) — Task 2 observes it.
- Produces: `on_terminal_mouse_effects` now also applies to `OzmaTerminal` entities with **no** `PtyHandle`/`Coalescer` (selection/copy/scroll via `*_vt_only` + `flush_emit`; `Write` → `trigger(TerminalForwardInput)`).
- Consumes (existing): `TerminalHandle::{selection_start_at_vt_only(point,side,ty), selection_update_to_vt_only(point,side), selection_clear_vt_only(), scroll_vt_only(delta), selection_to_string(), flush_emit(commands,entity)}` (`crates/ozma_tty_engine/src/handle.rs:570,593,609,348,~615,198`).

- [ ] **Step 1: Write the failing test**

Add to `crates/ozma_terminal/src/mouse.rs` `mod tests` (it already has `use super::*;`):

```rust
#[test]
fn detached_terminal_forwards_write_and_selects_via_vt_only() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use ozma_tty_engine::TerminalHandle;

    #[derive(Resource, Default)]
    struct CapturedForward(Vec<Vec<u8>>);

    let mut app = App::new();
    app.init_resource::<Clipboard>()
        .init_resource::<CapturedForward>()
        .add_observer(on_terminal_mouse_effects)
        .add_observer(|ev: On<TerminalForwardInput>, mut cap: ResMut<CapturedForward>| {
            cap.0.push(ev.bytes.clone());
        });

    // A pane-like terminal: OzmaTerminal with a detached handle, NO PtyHandle/Coalescer.
    let handle = TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false)));
    let entity = app.world_mut().spawn((OzmaTerminal, handle)).id();

    // A Write effect must be forwarded as TerminalForwardInput (not dropped).
    app.world_mut().trigger(TerminalMouseEffects {
        entity,
        effects: vec![MouseEffect::Write(b"\x1b[<0;1;1M".to_vec())],
    });
    // A local SelStart must set a selection through the vt_only path (no panic, no Coalescer).
    app.world_mut().trigger(TerminalMouseEffects {
        entity,
        effects: vec![MouseEffect::SelStart {
            point: APoint::new(Line(0), Column(0)),
            side: ASide::Left,
            ty: SelectionType::Simple,
        }],
    });

    assert_eq!(
        app.world().resource::<CapturedForward>().0,
        vec![b"\x1b[<0;1;1M".to_vec()],
        "Write on a PTY-less OzmaTerminal must emit TerminalForwardInput"
    );
    let handle = app.world().entity(entity).get::<TerminalHandle>().unwrap();
    assert!(
        handle.selection_to_string().is_some(),
        "SelStart on a PTY-less OzmaTerminal must set a selection via vt_only"
    );
}
```

> NOTE: confirm the `Point`/`Side`/`SelectionType` aliases in scope. `mouse.rs` already imports them (used by `MouseEffect`); reuse the same aliases the file uses (`APoint`, `ASide`, `Line`, `Column`, `SelectionType`). If an alias name differs, match the existing `use` block — do not add fully-qualified paths.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozma_terminal detached_terminal_forwards_write_and_selects_via_vt_only`
Expected: FAIL — `TerminalForwardInput` is not defined / observer drops PTY-less entities.

- [ ] **Step 3: Define `TerminalForwardInput`**

In `crates/ozma_terminal/src/mouse.rs`, directly after the `TerminalMouseEffects` struct (~265), add:

```rust
/// Terminal input bytes destined for the backend of `entity` (a PTY for a
/// local terminal, or tmux `send-keys` for a control-mode pane). Emitted by the
/// mouse apply observer when the terminal has no `PtyHandle`; the host owns the
/// observer that routes it to the real backend.
#[derive(EntityEvent, Debug, Clone)]
pub struct TerminalForwardInput {
    /// The terminal entity whose backend should receive `bytes`.
    #[event_target]
    pub entity: Entity,
    /// The raw bytes to deliver to the backend.
    pub bytes: Vec<u8>,
}
```

- [ ] **Step 4: Make the apply observer PtyHandle/Coalescer-optional**

Replace `on_terminal_mouse_effects` (~776) with:

```rust
fn on_terminal_mouse_effects(
    ev: On<TerminalMouseEffects>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: Query<
        (&mut TerminalHandle, Option<&mut PtyHandle>, Option<&mut Coalescer>),
        With<OzmaTerminal>,
    >,
) {
    let Ok((mut handle, pty, coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    if let (Some(mut pty), Some(mut coalescer)) = (pty, coalescer) {
        for effect in &ev.effects {
            apply_effect(&mut handle, &mut pty, &mut coalescer, &mut clipboard, effect);
        }
        return;
    }
    // PTY-less (tmux pane): local effects via *_vt_only, Write forwarded out.
    let mut dirty = false;
    for effect in &ev.effects {
        dirty |= apply_effect_detached(&mut handle, &mut clipboard, &mut commands, ev.entity, effect);
    }
    if dirty {
        handle.flush_emit(&mut commands, ev.entity);
    }
}

fn apply_effect_detached(
    handle: &mut TerminalHandle,
    clipboard: &mut Clipboard,
    commands: &mut Commands,
    entity: Entity,
    effect: &MouseEffect,
) -> bool {
    match effect {
        MouseEffect::Write(b) => {
            commands.trigger(TerminalForwardInput { entity, bytes: b.clone() });
            false
        }
        MouseEffect::SelStart { point, side, ty } => {
            handle.selection_start_at_vt_only(*point, *side, *ty);
            true
        }
        MouseEffect::SelUpdate { point, side } => {
            handle.selection_update_to_vt_only(*point, *side);
            true
        }
        MouseEffect::SelClear => {
            handle.selection_clear_vt_only();
            true
        }
        MouseEffect::Copy => {
            if let Some(text) = handle.selection_to_string() {
                clipboard.write(text);
            }
            false
        }
        MouseEffect::Scroll(lines) => {
            handle.scroll_vt_only(*lines);
            true
        }
        MouseEffect::OpenUri(uri) => {
            try_open_uri(uri);
            false
        }
    }
}
```

> NOTE: `apply_effect` (the PtyHandle path) is unchanged. `apply_effect_detached` is a sibling private helper; place it directly after `apply_effect` (private items stay grouped at the bottom of the file). Confirm `Coalescer` and `PtyHandle` are imported in `mouse.rs`'s top `use` block (the old query already named both — keep those imports).

- [ ] **Step 5: Export the event**

In `crates/ozma_terminal/src/lib.rs`, add `TerminalForwardInput` to the `pub use mouse::{…}` re-export (the line near `:24` exporting `MouseDisabled`, `OzmaMouseConfig`, …):

```rust
pub use mouse::{
    FineModifier, MouseDisabled, OzmaMouseConfig, OzmaTerminalMouseSet, TerminalForwardInput,
};
```

> NOTE: match the existing export line's exact item list; only add `TerminalForwardInput`. Keep alphabetical-ish ordering consistent with the file.

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p ozma_terminal detached_terminal_forwards_write_and_selects_via_vt_only`
Expected: PASS.

- [ ] **Step 7: Verify the existing Ozma-mode path is untouched**

Run: `cargo test -p ozma_terminal`
Expected: PASS (all existing mouse/selection tests still green — the PtyHandle branch is unchanged).

- [ ] **Step 8: Lint, format, commit**

```bash
cargo clippy -p ozma_terminal --all-targets && cargo fmt
git add crates/ozma_terminal/src/mouse.rs crates/ozma_terminal/src/lib.rs
git commit -m "feat(ozma_terminal): TerminalForwardInput seam for PTY-less terminals"
```

---

### Task 2: Host — `forward_pane_input` observer routes `TerminalForwardInput` to tmux

**Files:**
- Create: `src/tmux/forward.rs`
- Modify: `src/tmux.rs` (declare `mod forward;`, add `ForwardPlugin`)
- Test: `src/tmux/forward.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `ozma_terminal::TerminalForwardInput` (Task 1); `ozmux_tmux::{TmuxConnection, TmuxPane, PaneId}`; `ozmux_tmux::send_bytes_command(pane: &str, bytes: &[u8]) -> String` (`crates/tmux_session/src/input.rs:107`).
- Produces: `pub(crate) struct ForwardPlugin` registered in `src/tmux.rs`.

- [ ] **Step 1: Write the failing test (pure target builder)**

Create `src/tmux/forward.rs` with the module doc, a pure `pane_target` helper, and its test:

```rust
//! Routes `ozma_terminal`'s `TerminalForwardInput` (backend-bound bytes from the
//! shared mouse apply observer) to the owning tmux pane via `send-keys -H`.

use bevy::prelude::*;
use ozma_terminal::TerminalForwardInput;
use ozmux_tmux::{PaneId, TmuxConnection, TmuxPane, send_bytes_command};

/// The tmux target string (`%<id>`) for a pane id.
fn pane_target(pane: PaneId) -> String {
    format!("%{}", pane.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_builds_send_keys_hex_for_pane() {
        let cmd = send_bytes_command(&pane_target(PaneId(3)), b"\x1b[<0;1;1M");
        assert_eq!(cmd, "send-keys -H -t %3 1b 5b 3c 30 3b 31 3b 31 4d");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux-gui --lib forward::tests::forward_builds_send_keys_hex_for_pane`
Expected: FAIL to compile — module not declared yet.

- [ ] **Step 3: Declare the module**

In `src/tmux.rs`, add `mod forward;` to the module list (alongside `mod input;`, `mod mouse;`, …) and `use forward::ForwardPlugin;`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux-gui --lib forward::tests::forward_builds_send_keys_hex_for_pane`
Expected: PASS.

- [ ] **Step 5: Add the observer and plugin**

Append to `src/tmux/forward.rs` (before the `#[cfg(test)]` block):

```rust
/// Registers the `TerminalForwardInput` → tmux `send-keys -H` observer.
pub(crate) struct ForwardPlugin;

impl Plugin for ForwardPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(forward_pane_input);
    }
}

fn forward_pane_input(
    ev: On<TerminalForwardInput>,
    panes: Query<&TmuxPane>,
    connection: NonSend<TmuxConnection>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return; // not a tmux pane (e.g. an Ozma-mode terminal): leave to the PTY path
    };
    let Some(client) = connection.client() else {
        return;
    };
    if let Err(e) = client.handle().send(&send_bytes_command(&pane_target(pane.id), &ev.bytes)) {
        tracing::warn!(?e, "tmux mouse-report forward failed");
    }
}
```

> NOTE: confirm `TmuxPane.id` is `PaneId` (it is — `src/tmux/render.rs` reads `p.id`). `connection.client()` returns `Option<&TmuxClient>` (`crates/tmux_session/src/connection.rs:25`); `client.handle().send(&str)` is the existing send path used by `src/tmux/mouse.rs:744`.

- [ ] **Step 6: Register the plugin**

In `src/tmux.rs` `TmuxPlugin::build` (or the aggregate `add_plugins(...)` chain), add `ForwardPlugin` to the registered tmux plugins.

- [ ] **Step 7: Build + lint + commit**

```bash
cargo build -p ozmux-gui && cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux/forward.rs src/tmux.rs
git commit -m "feat(tmux): forward TerminalForwardInput to panes via send-keys -H"
```

---

### Task 3: Host — `maintain_tmux_input_gates` + arbitration pre-gate

**Files:**
- Create: `src/tmux/gate.rs`
- Modify: `src/tmux.rs` (`mod gate;`, register `GatePlugin`)
- Test: `src/tmux/gate.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `ozma_terminal::{KeyboardDisabled, MouseDisabled, OzmaTerminalInputSet, OzmaTerminalMouseSet}`; `ozmux_tmux::TmuxPane`; `crate::ui::copy_mode::CopyModeState`; `crate::picker::SessionPicker`; `crate::input::ime::ImeState`; `bevy_cef::prelude::FocusedWebview`; `crate::ozma::AppMode`.
- Produces: `pub(crate) struct GatePlugin`. Panes carry `KeyboardDisabled` always and `MouseDisabled` when suppressed.

- [ ] **Step 1: Write the failing test (pure predicate)**

Create `src/tmux/gate.rs`:

```rust
//! Per-pane input gating for `AppMode::Ozmux`: every pane is `KeyboardDisabled`
//! (keys pass through to tmux), and `MouseDisabled` whenever a modal owns input,
//! the pane is in copy mode, or a webview is interacting — so `ozma_terminal`'s
//! shared mouse systems yield to the tmux-specific gestures.

use bevy::prelude::*;

/// Whether a pane's `ozma_terminal` mouse handling must be suppressed.
fn should_disable_pane_mouse(modal: bool, in_copy_mode: bool, webview_active: bool) -> bool {
    modal || in_copy_mode || webview_active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_mouse_on_any_guard() {
        assert!(!should_disable_pane_mouse(false, false, false));
        assert!(should_disable_pane_mouse(true, false, false));
        assert!(should_disable_pane_mouse(false, true, false));
        assert!(should_disable_pane_mouse(false, false, true));
    }
}
```

- [ ] **Step 2: Run test to verify it fails, then declare the module**

Run: `cargo test -p ozmux-gui --lib gate::tests::suppresses_mouse_on_any_guard`
Expected: FAIL (module not declared). Then add `mod gate;` + `use gate::GatePlugin;` to `src/tmux.rs`. Re-run: PASS.

- [ ] **Step 3: Add the maintainer system + plugin**

Append to `src/tmux/gate.rs` (before `#[cfg(test)]`):

```rust
use crate::input::ime::ImeState;
use crate::ozma::AppMode;
use crate::picker::SessionPicker;
use crate::ui::copy_mode::CopyModeState;
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{KeyboardDisabled, MouseDisabled, OzmaTerminalInputSet, OzmaTerminalMouseSet};
use ozmux_tmux::TmuxPane;

/// Registers the Ozmux-mode per-pane input gate maintainer.
pub(crate) struct GatePlugin;

impl Plugin for GatePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            maintain_tmux_input_gates
                .before(OzmaTerminalInputSet)
                .before(OzmaTerminalMouseSet)
                .run_if(in_state(AppMode::Ozmux)),
        );
    }
}

fn maintain_tmux_input_gates(
    mut commands: Commands,
    picker: Res<SessionPicker>,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    panes: Query<(Entity, Has<KeyboardDisabled>, Has<MouseDisabled>, Has<CopyModeState>), With<TmuxPane>>,
) {
    let window_focused = windows.single().map(|w| w.focused).unwrap_or(false);
    let modal = picker.open || ime.is_composing() || !window_focused || focused_webview.0.is_some();
    for (entity, has_keyboard, has_mouse, in_copy_mode) in panes.iter() {
        if !has_keyboard {
            commands.entity(entity).insert(KeyboardDisabled);
        }
        let disable_mouse = should_disable_pane_mouse(modal, in_copy_mode, focused_webview.0.is_some());
        if disable_mouse && !has_mouse {
            commands.entity(entity).insert(MouseDisabled);
        } else if !disable_mouse && has_mouse {
            commands.entity(entity).remove::<MouseDisabled>();
        }
    }
}
```

> NOTE: `KeyboardDisabled` is inserted once and never removed for panes (keys always pass through to tmux). The change-guarded `Has<…>` checks avoid re-inserting every frame (satisfies the "let mutation drive change detection" rule). Divider/inline-webview per-press claims are added in Step 4.

- [ ] **Step 4: Add the divider/webview pre-gate claim**

The pre-gate must mark a pane `MouseDisabled` for the frame when the cursor is within the divider grab band or over an interactive inline webview, so `ozma_terminal` does not arm a selection there. Reuse the existing hit-tests:
- divider band: `src/tmux/mouse.rs::divider_at` / `DividerPixelRect` (already `pub(crate)`).
- inline webview: `src/tmux/mouse.rs::inline_hit_at` / `crate::inline_webview` helpers.

Add to `maintain_tmux_input_gates` (or a sibling system in the same `.before(OzmaTerminalMouseSet)` set) a term that ORs into `disable_mouse` for the specific pane under the cursor when a divider/webview claim applies. Concretely, extend the `panes` loop: compute `claimed = cursor_over_divider_band || cursor_over_interactive_inline(entity)` and fold it into `disable_mouse` for that entity.

> NOTE: keep the claim logic in `gate.rs` calling the existing `pub(crate)` hit-test helpers; do NOT duplicate the geometry. If a helper is currently private to `mouse.rs`, widen it to `pub(super)` (not `pub`) in Task 5 when that file is edited. Write a focused test for the pure OR-fold (e.g. `should_disable_pane_mouse(false,false,false)` with a `claimed=true` fourth argument) — extend `should_disable_pane_mouse` to take a `claimed: bool` and update Step 1's test accordingly.

- [ ] **Step 5: Register, build, test, commit**

Add `GatePlugin` to the tmux plugin chain in `src/tmux.rs`.

```bash
cargo test -p ozmux-gui --lib gate:: && cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux/gate.rs src/tmux.rs
git commit -m "feat(tmux): per-pane KeyboardDisabled/MouseDisabled gate for Ozmux"
```

---

### Task 4: Host — attach `OzmaTerminal` to panes; drop the duplicate render bundle

**Files:**
- Modify: `src/tmux/render.rs::attach_tmux_pane_terminal` (~122-148)
- Test: `crates/tmux_session/tests/real_tmux_pane.rs` or a render unit test (see Step 4)

**Interfaces:**
- Consumes: `ozma_terminal::OzmaTerminal`; the existing `On<Add, OzmaTerminal>` render-bundle observer (`crates/ozma_terminal/src/spawn.rs:99-109`) which inserts `TerminalRenderBundle::new(TerminalUiMaterial::default())`.
- Produces: every projected `TmuxPane` now carries `OzmaTerminal` and exactly one `TerminalRenderBundle` (injected by the `Add` observer).

- [ ] **Step 1: Modify `attach_tmux_pane_terminal` — add `OzmaTerminal`, drop the local bundle**

Replace the body's `material`/insert section so the pane no longer creates its own material or `TerminalRenderBundle` (the `On<Add, OzmaTerminal>` observer owns that — design doc §"Render-bundle reconciliation", option a):

```rust
for (entity, pane) in panes.iter() {
    let (cols, rows) = grid_dims(pane.dims.width, pane.dims.height);
    let handle = TerminalHandle::detached(cols, rows, gate.clone());

    commands.entity(entity).insert((
        handle,
        TerminalTitle::default(),
        OzmaTerminal,
        Node {
            position_type: PositionType::Absolute,
            ..default()
        },
    ));
}
```

Remove the now-unused `materials: ResMut<Assets<TerminalUiMaterial>>` parameter and the `let material = materials.add(...)` line (the `Add` observer creates the material). Drop the `TerminalUiMaterial` / `TerminalRenderBundle` imports if they become unused. Add `use ozma_terminal::OzmaTerminal;` to the top `use` block.

> NOTE: `OzmaTerminalPlugin` is already added in `src/main.rs:75`, so the `On<Add, OzmaTerminal>` observer is live in Ozmux mode too. Confirm `OzmaTerminal` is a plain unit marker with no `#[require(...)]` that would pull conflicting components (it is — `crates/ozma_terminal/src/spawn.rs:17`).

- [ ] **Step 2: Build to verify the render path compiles**

Run: `cargo build -p ozmux-gui`
Expected: PASS (no unused-import or missing-param errors).

- [ ] **Step 3: Manual smoke check**

Run: `cargo run` in a tmux-capable environment; attach an Ozmux session.
Expected: panes still render text (the `Add` observer supplied the render bundle); text selection by drag now works via `ozma_terminal` (a transient double-selection with the not-yet-trimmed arbiter is expected and removed in Task 5).

- [ ] **Step 4: Add/extend a test asserting panes are OzmaTerminal**

In the existing tmux render/projection test, after a pane is projected and `attach_tmux_pane_terminal` runs, assert the pane entity has both `OzmaTerminal` and a `TerminalGrid` (proving the `Add` observer fired and there is exactly one render bundle):

```rust
assert!(world.entity(pane_entity).contains::<ozma_terminal::OzmaTerminal>());
assert!(world.entity(pane_entity).contains::<ozma_tty_renderer::schema::TerminalGrid>());
```

> NOTE: pick the closest existing test harness (`crates/tmux_session/tests/real_tmux_pane.rs` runs a real tmux; a pure Bevy `App` test that inserts a `TmuxPane` and runs `attach_tmux_pane_terminal` + an `update()` to flush the `Add` observer is lighter — prefer the latter if a fixture exists).

- [ ] **Step 5: Lint + commit**

```bash
cargo test -p ozmux-gui --lib && cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux/render.rs
git commit -m "feat(tmux): make panes first-class OzmaTerminal entities"
```

---

### Task 5: Host — trim `src/tmux/mouse.rs` to tmux-specific gestures only

**Files:**
- Modify: `src/tmux/mouse.rs` (`arbiter` ~268-407 and its helpers)
- Test: `src/tmux/mouse.rs` `#[cfg(test)] mod tests` (adjust deleted-behavior tests)

**Interfaces:**
- Keep: `select-pane` on press, divider drag → `resize-pane`, copy-mode mouse (`send-keys -X`), inline-webview (CEF) mouse, and the `pub(crate)` hit-tests (`divider_at`, `tmux_pane_at_phys`, `inline_hit_at`) that Task 3's pre-gate reuses.
- Delete: local VT selection state machine (`SelectingVt`), multi-click word/line select+copy, hover-underline, hyperlink-open — all now owned by `ozma_terminal`.

- [ ] **Step 1: Identify the deletions**

In `src/tmux/mouse.rs`, the `arbiter` system and `TmuxMouseGesture` handle several concerns. Remove only the terminal-interaction concerns that `ozma_terminal` now owns:
- the drag-to-VT-selection branch (`selection_start_at`/`selection_update_to` on `TerminalHandle`, the `SelectingVt` state),
- multi-click `select-word`/`select-line` + clipboard copy when NOT in copy mode (`multi_select_commands` local path),
- hyperlink-open branch (`link_modifier_held` / `try_open_uri`),
- hover handling that sets the link/hover cursor for panes (now `ozma_terminal::hyperlink_hover_cursor`).

Keep: the `select-pane`-on-press branch, the divider-resize branch, the copy-mode `send-keys -X` relays, and the inline-webview click/move forwarding.

> NOTE: this is a surgical removal, not a rewrite. Work branch-by-branch inside `arbiter`; after each removal run `cargo build -p ozmux-gui` to catch a now-unused helper, then delete that helper too (e.g. `multi_select_commands`, `cell_and_side` if only the deleted path used them). Keep helpers still used by the divider/copy-mode/inline paths.

- [ ] **Step 2: Delete the corresponding tests**

Remove unit tests that assert the deleted local-selection/multi-click behavior (e.g. `multi_select_word_commands`, the `arbiter` VT-selection state assertions). Keep divider hit-test, click-tracker, and `target_copy_cmd` tests.

- [ ] **Step 3: Widen hit-test helper visibility for the pre-gate (if needed)**

If Task 3's pre-gate needs `divider_at` / `inline_hit_at` and they are currently private, change them to `pub(super)` (narrowest that lets `gate.rs` call them). Do NOT make them `pub`.

- [ ] **Step 4: Build + test**

Run: `cargo test -p ozmux-gui --lib mouse::`
Expected: PASS (remaining divider/copy-mode/inline tests green; deleted-behavior tests gone).

- [ ] **Step 5: Manual verification — no double handling**

Run: `cargo run`, attach Ozmux.
Expected: single clean selection on drag (no double), word/line double-click selects and copies (via `ozma_terminal`), Cmd-click opens a hyperlink, pane focus on click still works, divider drag resizes, copy-mode mouse still works.

- [ ] **Step 6: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux/mouse.rs
git commit -m "refactor(tmux): drop duplicated mouse-interaction; keep tmux-specific gestures"
```

---

### Task 6: Host — wheel reconciliation (gate on `ALTERNATE_SCROLL`)

**Files:**
- Modify: `src/tmux/input.rs::forward_wheel_to_tmux` (~597) and its plugin registration (~50)
- Test: `src/tmux/input.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Keep in `forward_wheel_to_tmux`: copy-mode scroll (`send-keys -X scroll-up|down`), inline-webview wheel forwarding, and the narrow alt-screen-`!ALTERNATE_SCROLL`-`!MOUSE_MODE` cursor-key fallback (`alt_screen_scroll_command`).
- Delegate to `ozma_terminal`'s `dispatch_mouse_wheel`: normal local-VT scrollback and mouse-mode / alt-screen-`ALTERNATE_SCROLL` wheel (the latter emits SS3 arrows via `WheelAction::route` → `MouseEffect::Write` → `TerminalForwardInput`).

- [ ] **Step 1: Narrow `forward_wheel_to_tmux`**

Restrict the system to the cases `ozma_terminal` does NOT own:
- if the pane under the cursor is in `CopyModeState` → keep the `scroll_command` (`send-keys -X scroll`) path.
- else if an inline webview claims the wheel → keep forwarding to CEF.
- else if `handle.is_in_alt_screen()` AND the pane's `current_modes()` has neither `ALTERNATE_SCROLL` nor a `MOUSE_MODE` bit → keep `alt_screen_scroll_command` (cursor keys).
- else → **do nothing** (drop the wheel here; `ozma_terminal::dispatch_mouse_wheel` handles local scrollback and the SGR/SS3 forward via the new sink).

Use `TerminalHandle::current_modes()` (`crates/ozma_tty_engine/src/handle.rs:377`) to read the bits; `is_in_alt_screen()` (`:387`). The `ALTERNATE_SCROLL` / `MOUSE_MODE` flags come from `current_modes()` (`alacritty_terminal::term::TermMode`).

- [ ] **Step 2: Write/adjust the boundary test**

```rust
#[test]
fn wheel_owned_by_ozma_outside_copymode_altscreen_inline() {
    // A normal pane (not copy-mode, not alt-screen, no mouse mode) produces NO
    // tmux send-keys from forward_wheel_to_tmux — ozma_terminal owns it now.
    // (Assert the system emits no `send-keys` command for that case; keep the
    // existing copy-mode scroll_command test as the positive case.)
}
```

> NOTE: keep the existing `scroll_command` copy-mode test (`src/tmux/input.rs:793,801`) as the still-true positive path. Add the negative case for the normal pane. If the current test harness drives `forward_wheel_to_tmux` end-to-end, assert the connection received no command; otherwise unit-test the decision helper you extract.

- [ ] **Step 3: Build + test**

Run: `cargo test -p ozmux-gui --lib input::`
Expected: PASS.

- [ ] **Step 4: Manual verification**

Run: `cargo run`, attach Ozmux. Expected: wheel scrolls local scrollback on a shell pane; wheel drives mouse-aware apps (e.g. `less`/`vim` with mouse on); copy-mode wheel still scrolls; alt-screen app without mouse still responds (cursor keys). No double-scroll.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux/input.rs
git commit -m "refactor(tmux): wheel owned by ozma_terminal except copy-mode/alt-screen-no-1007"
```

---

### Task 7: Integration test (DECSET-in-`%output`) + final verification

**Files:**
- Create/modify: `crates/tmux_session/tests/real_tmux_pane.rs` (or a new `real_tmux_mouse.rs`)

**Interfaces:**
- Consumes: a real tmux `-CC` session fixture (existing `real_tmux_*` tests show the harness), `TerminalHandle::current_modes()`.

- [ ] **Step 1: Write the DECSET integration test**

Closes the one `[unverified]` premise (design doc §Testing): assert tmux `%output` carries a pane app's DECSET mouse-mode and that the pane's detached `TerminalHandle::current_modes()` reflects it.

```rust
// In a real tmux -CC session: run a command in a pane that enables SGR mouse
// (e.g. `printf '\e[?1000h\e[?1006h'`), pump %output into the pane handle, then:
let modes = pane_handle.current_modes();
assert!(modes.intersects(alacritty_terminal::term::TermMode::MOUSE_REPORT_CLICK));
assert!(modes.intersects(alacritty_terminal::term::TermMode::SGR_MOUSE));
```

> NOTE: model the fixture on `crates/tmux_session/tests/real_tmux_resize.rs` (it already sends `send-keys -t … -l -- '…'` + `Enter` and pumps events). These tests are gated to environments with tmux installed — follow the existing `#[ignore]`/feature gating convention in that file.

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p ozmux_tmux --test real_tmux_pane decset` (or the new test name)
Expected: PASS where tmux is available.

- [ ] **Step 3: Full workspace verification**

Run:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --check
```
Expected: all PASS / clean.

- [ ] **Step 4: Manual end-to-end checklist (design doc §Testing)**

- shell pane: drag selects, double-click word + copy, Cmd-click opens link — all via `ozma_terminal`.
- `vim`/`htop` pane (mouse on): clicks/drag/wheel reach the app (forwarded `send-keys -H`).
- Shift-drag over a mouse-on pane: forces local selection.
- copy-mode pane: mouse drives tmux copy-mode (no ozma double).
- divider drag resizes; pane focus on click; inline webview mouse works.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/tests/
git commit -m "test(tmux): DECSET mouse-mode reaches detached pane handle via %output"
```

---

## Self-Review

**Spec coverage:**
- Panes become `OzmaTerminal` → Task 4. ✓
- Sink seam (`TerminalForwardInput`, optional PtyHandle, `*_vt_only`+`flush_emit`) → Task 1. ✓
- Host forward observer → Task 2. ✓
- Gate maintainer (KeyboardDisabled always + MouseDisabled modal/copy-mode/webview) → Task 3. ✓
- Arbitration pre-gate (divider/webview claim before `OzmaTerminalMouseSet`) → Task 3 Step 4 + Task 5 Step 3. ✓
- Delete duplicated mouse code → Task 5. ✓
- Wheel `ALTERNATE_SCROLL` reconciliation → Task 6. ✓
- Render-bundle double-insert fix → Task 4 Step 1. ✓
- `#[event_target]` on the event → Task 1 Step 3. ✓
- Event defined in `ozma_terminal`, no engine re-export → Task 1 (mouse.rs + lib.rs). ✓
- DECSET-in-`%output` integration test → Task 7. ✓
- Control-channel batching (design doc §Testing) → NOT yet a task; it is an optimization, not correctness. Deferred — add a follow-up only if profiling shows control-channel pressure. (Flagged so it is not silently dropped.)

**Type consistency:** `TerminalForwardInput { entity, bytes }` is produced in Task 1 and consumed in Task 2 with the same field names. `pane_target(PaneId) -> String` and `send_bytes_command(&str, &[u8]) -> String` match `crates/tmux_session/src/input.rs:107`. `current_modes() -> TermMode` used consistently in Tasks 3/6. `should_disable_pane_mouse` gains a `claimed` arg in Task 3 Step 4 — Step 1's test is updated there.

**Placeholder scan:** No "TBD"/"implement later". The subtractive Tasks 5/6 give branch-level instructions with exact systems/line refs rather than full file rewrites (a refactor cannot inline 1000 deleted lines); each has concrete keep/delete lists, real test code, and verification commands.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-20-ozmux-ozma-terminal-reuse.md`.
