# Unified Shortcut Dispatch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the two near-duplicate keyboard-shortcut dispatchers (`app_shortcut_handler`, `forward_keys_to_tmux`) plus `dispatch_input` into one pure decider `classify_key_batch` + two thin `run_if(in_state)` mode appliers that trigger existing per-action `EntityEvent`s — behaviour-preserving.

**Architecture:** A new pure host-private module `src/input/resolve.rs` owns the decision IR (`KeyEffect`) and `classify_key_batch`, absorbing every per-key rule currently duplicated across the two dispatchers and `dispatch_input`. Each mode keeps a thin applier that resolves its own target entities, feeds the decider its context, and translates the returned `Vec<KeyEffect>` into `commands.trigger(...)` of existing (and two new) `EntityEvent`s. Typing, paste, detach, and plain-key forwarding are applied by observers, not by the appliers.

**Tech Stack:** Rust 2024, Bevy 0.18.1 ECS (`EntityEvent` + observers, `run_if(in_state)`, `MessageReader`), `alacritty_terminal`, tmux control-mode client (`orzma_tmux`).

## Global Constraints

- Rust edition 2024, toolchain 1.95. No `mod.rs`. Comments English-only; only `// TODO:` / `// NOTE:` / `// SAFETY:` line comments (NOTE = critical caveat only).
- Every externally-`pub` item gets a `///` doc; every file-level module gets a `//!`. All `use` at top of file, one contiguous block, no glob in consumer code, no inline fully-qualified paths.
- Visibility minimization is MANDATORY: any item with no caller outside its defining module MUST be private. New host items are `pub(crate)` at most unless a caller forces wider.
- Mutable params first in signatures. `Query` params use descriptive nouns (no `_q`). Items ordered `pub` → `pub(crate)` → private within a block.
- Systems/observers are registered by the `Plugin` defined in the SAME file; aggregators only `add_plugins`. Whole-system change guards use `run_if`, not in-body early returns.
- System bodies stay within ~150 lines; split gather/decide/apply via a pure helper + `EntityEvent`/observer handoff — never inline sequencing.
- Behaviour-preserving refactor: NO user-visible behaviour change. Default keeps pane/window shortcuts as no-ops; the tmux path keeps its exact prompt guards, batch-forward, and paste/detach semantics.
- Full gate for every task's final step: `cargo test` (or the task's crate subset) then `cargo clippy --workspace --all-targets` and `cargo fmt --check` clean.

---

## File Structure

- **Create** `src/input/resolve.rs` — `KeyEffect`, `BatchContext`, `classify_key_batch`, `step_with_repeat`; exhaustive pure unit tests. No systems, no plugin.
- **Create** `src/action/tmux/detach_session.rs` — `DetachSessionRequest` event, `on_detach_session` observer, `DetachSessionPlugin`.
- **Modify** `src/action/tmux.rs` — declare `mod detach_session;`, add `DetachSessionPlugin` to `TmuxActionPlugin`.
- **Create** `src/action/tmux/paste.rs` — `on_paste_tmux` observer (`With<TmuxPane>`) + `TmuxPastePlugin`, applying `PasteAction` for tmux panes via chunked `SendBytes`.
- **Modify** `src/action/terminal/paste.rs` — add `Without<TmuxPane>` to `on_paste`.
- **Modify** `src/input/tmux/forward.rs` — add `ForwardPaneKeysRequest` event, `on_forward_pane_keys` observer, register in `ForwardPlugin`.
- **Modify** `src/input/default_mode.rs` — replace `app_shortcut_handler` with `apply_default_shortcuts`; keep `maintain_input_gates`, `apply_ime_commit_to_terminal`.
- **Modify** `src/input/tmux/input.rs` — replace the keyboard body of `forward_keys_to_tmux` with `apply_tmux_shortcuts`; keep `forward_wheel_to_tmux`, `ActionTargets`, `TmuxWheelAccumulator`.
- **Modify** `src/input/keyboard.rs` — delete `dispatch_input` + its tests; keep `bevy_key_to_terminal_key`, `current_terminal_modifiers`, and a trimmed `KeyboardInputPlugin` that only owns `add_message::<KeyboardInput>()`.
- **Modify** `src/input/shortcuts.rs` — remove `LeaderGate::Read`; delete `Shortcuts::input_bindings`, `Shortcuts::opens_repeat_window`, `populate_input_bindings`; drop the `TerminalInputBindings` init from `ShortcutsPlugin` startup chain.
- **Modify** `src/input/bindings.rs` — delete `TerminalInputBindings` + `ReservedChord` (keyboard policy only); KEEP `OrzmaMouseConfig`, `FineModifier` (mouse policy).
- **Modify** `src/input.rs` — declare `mod resolve;`.

---

### Task 1: Pure decider `classify_key_batch` (unwired)

Builds the single source of truth. Pure, no ECS handles, fully unit-tested. Nothing is wired to the schedule yet — this task only adds the module and its tests.

**Files:**
- Create: `src/input/resolve.rs`
- Modify: `src/input.rs` (add `mod resolve;` with the other submodule declarations)

**Interfaces:**
- Consumes: `LeaderPhase`, `Shortcuts`, `step_leader`, `LeaderStep`, `is_modifier_key` (from `crate::input::shortcuts`); `Modifiers`, `Shortcut` (from `orzma_configs::shortcuts`); `ResolvedCopyModeKeys` (from `crate::action::vi`); `CopyModeAction` (from `orzma_configs::copy_mode` — NOT re-exported by `crate::action::vi`); `bevy::input::keyboard::{Key, KeyCode, KeyboardInput}`; `orzma_webview::NormalizedChord` (the `ForwardKeys.0` element, fields `code/ctrl/shift/alt/logo`) for `forward_chords`.
- Produces:
  ```rust
  pub(crate) enum KeyEffect {
      Action { action: Shortcut, via_leader: bool },
      CopyMode(CopyModeAction),
      Type { logical: Key, key_code: KeyCode },
      WebviewForward { logical: Key, key_code: KeyCode },
      ReleaseWebviewFocus,
  }
  pub(crate) struct BatchContext<'a> {
      pub(crate) mods: Modifiers,
      pub(crate) now: Duration,
      pub(crate) in_copy_mode: bool,
      pub(crate) webview_focused: bool,
      pub(crate) forward_chords: &'a [orzma_webview::NormalizedChord],
  }
  pub(crate) fn classify_key_batch<'a>(
      leader_phase: &mut LeaderPhase,
      shortcuts: &Shortcuts,
      resolved_copy: &ResolvedCopyModeKeys,
      events: impl Iterator<Item = &'a KeyboardInput>,
      ctx: BatchContext<'a>,
  ) -> Vec<KeyEffect>;
  ```

- [ ] **Step 1: Add the module declaration**

In `src/input.rs`, add `mod resolve;` in the submodule block (alongside `mod keyboard;`, `mod shortcuts;`, etc.). Keep declarations in the existing order/style.

- [ ] **Step 2: Write the failing tests**

Create `src/input/resolve.rs` with the `//!` module doc, the `KeyEffect`/`BatchContext`/`classify_key_batch` signatures with a `todo!()` body, and a `#[cfg(test)] mod tests`. Port every existing dispatch/withhold test as a pure assertion over `Vec<KeyEffect>`. Concrete cases to include (names → intent):

```rust
// Leader / repeat (port from shortcuts.rs + the two dispatchers)
leader_press_swallows_and_no_type            // Ctrl+A leader -> [] (Swallow, no Type)
leader_then_bound_key_emits_action           // <Leader>s -> [Action{EnterCopyMode, via_leader:true}]
direct_gui_chord_emits_action_not_leader     // Cmd+Q -> [Action{Quit, via_leader:false}]
plain_key_emits_type                         // 'a' -> [Type{KeyA,..}]
repeat_window_refires_on_os_repeat           // Repeat{..} + repeat 'h' -> [Action{.., via_leader:true}]
repeat_outside_window_passthrough_no_step    // Idle + repeat 'h' -> [Type] (machine not stepped)
pending_skips_bare_modifier_then_second_key  // Ctrl then D -> the real second key resolves
// Withhold parity (port from keyboard.rs tests)
pending_suppresses_type_for_second_key       // Pending + 'a' -> [] (no Type)
pending_types_trailing_same_frame_key        // Pending + 'a' + 'b' -> [Type{b}]
repeat_window_withholds_matching_key         // Repeat + repeat-marked 'h' -> [Action{EnterCopyMode, via_leader:true}] (assert: NO Type variant, not empty Vec)
repeat_window_types_non_matching_key         // Repeat + 'b' -> [Type{b}]
window_closing_key_stops_withholding_same_frame // Repeat + 'b' + 'h' -> [Type{b}, Type{h}]
// New invariants surfaced in spec review
release_webview_chord_emits_type_no_webview  // Ctrl+Shift+Escape, webview_focused:false -> [Type{Escape,..}] (decider emits Type; Default applier drops it, tmux forwards it)
no_type_while_in_copy_mode                    // in_copy_mode + unmatched 'x' -> [] or [CopyMode(..)]
copy_key_shadowed_by_gui                      // in_copy_mode + a GUI chord -> [Action{..}], not CopyMode
meta_unmatched_dropped                         // Cmd+J unmatched -> [] (no Type)
direct_paste_suppressed_in_copy_mode          // in_copy_mode + Cmd+V -> [Action{Paste, via_leader:false}] (applier suppresses)
leader_paste_fires_in_copy_mode               // in_copy_mode + <Leader>p -> [Action{Paste, via_leader:true}]
webview_focus_clears_leader_and_forwards      // webview_focused + forward chord -> [WebviewForward{..}] + leader Idle
webview_release_chord_emits_release           // webview_focused + release chord -> [ReleaseWebviewFocus]
```

Use the existing test constructors: `test_shortcuts_with_repeat_prefix` (already `pub(crate)` in `shortcuts.rs`) and a local `KeyboardInput` builder mirroring `keyboard.rs`'s `press()`/`send_key_repeat()`. Assert on `Vec<KeyEffect>` and on the resulting `LeaderPhase`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p orzma --lib input::resolve`
Expected: FAIL — `todo!()` panics / assertions unmet.

- [ ] **Step 4: Implement `classify_key_batch` + `step_with_repeat`**

Extract the shared per-key loop. `step_with_repeat` wraps the `ev.repeat` handling (port verbatim from `src/input/default_mode.rs:221-231` == `src/input/tmux/input.rs:258-268`):

```rust
/// Advances the leader machine for one pressed event, honoring OS auto-repeat:
/// outside the repeat window a repeat event does NOT step the machine.
fn step_with_repeat(
    leader_phase: &mut LeaderPhase,
    shortcuts: &Shortcuts,
    ev: &KeyboardInput,
    mods: Modifiers,
    now: Duration,
) -> LeaderStep {
    if ev.repeat {
        match *leader_phase {
            LeaderPhase::Pending => LeaderStep::Swallow, // caller `continue`s
            LeaderPhase::Repeat { .. } => step_leader(leader_phase, shortcuts, ev.key_code, mods, now),
            LeaderPhase::Idle => LeaderStep::Passthrough,
        }
    } else {
        step_leader(leader_phase, shortcuts, ev.key_code, mods, now)
    }
}
```
NOTE: the current code `continue`s on a repeat-while-Pending (does not call `step_leader`); model that here by returning `Swallow` for that arm and having `classify_key_batch` treat `Swallow` as "emit nothing, next event".

`classify_key_batch` per pressed event (skip `state != Pressed`):
1. If `ctx.webview_focused`: clear `*leader_phase` to `Idle`; if the key matches `shortcuts.is_release_webview_focus(key_code, ctx.mods)` push `ReleaseWebviewFocus`; else if it matches a `ctx.forward_chords` chord push `WebviewForward { logical, key_code }`; else nothing. Continue. (Port the branch shape from `input.rs:184-233` and `default_mode.rs:202-213`.)
2. Else `let step = step_with_repeat(...)`. On `Swallow` → nothing. On `RunAction(a)` → `Action { action: a, via_leader: true }`. On `Passthrough` → `shortcuts.match_gui_action(key_code, ctx.mods)`; if `Some(a)` → `Action { action: a, via_leader: false }`.
3. If no `Action` produced: if `ctx.in_copy_mode` and `resolved_copy.resolve(&ev.logical_key, ev.key_code, ctx.mods)` is `Some(a)` → `CopyMode(a)`; then continue (never fall to `Type`).
4. Else (not in copy mode): if `is_modifier_key(ev.key_code)` → nothing (a bare modifier is never a `Type`; keeps the IR faithful and the withhold suppression-slot logic honest); else if `ctx.mods.meta` → nothing (meta-drop parity); else → `Type { logical: ev.logical_key.clone(), key_code: ev.key_code }`. NOTE: the decider does NOT special-case `is_release_webview_focus` here. Emitting `Type` for it is correct — the reserved-chord withhold is DEFAULT-ONLY (it lived in `dispatch_input`'s `bindings.reserved`, built by `input_bindings()`), and in tmux the very same Ctrl+Shift+Escape must FORWARD to the pane today. So the Default applier (Task 5) drops the release chord on `Type`; the tmux applier (Task 6) forwards it. Putting the swallow in the shared decider would regress tmux.

Before the loop, port the "close a stale repeat window when in copy mode" guard (`default_mode.rs:195-197` / `input.rs:242-244`): `if ctx.in_copy_mode && matches!(*leader_phase, LeaderPhase::Repeat { .. }) { *leader_phase = LeaderPhase::Idle; }`.

`step_leader` already returns `Passthrough` for a bare modifier without touching `LeaderPhase`; rule #4's explicit `is_modifier_key` check then drops it so no `Type` is emitted. Verify the ported tests cover `pending_skips_bare_modifier_then_second_key` (the leading Ctrl of a second chord must not consume the suppression slot nor emit `Type`).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p orzma --lib input::resolve`
Expected: PASS (all ported + new cases).

- [ ] **Step 6: Lint + commit**

```bash
cargo clippy -p orzma --all-targets && cargo fmt
git add src/input/resolve.rs src/input.rs
git commit -m "feat(input): add pure classify_key_batch decider (unwired)"
```

---

### Task 2: `DetachSessionRequest` event + observer

**Files:**
- Create: `src/action/tmux/detach_session.rs`
- Modify: `src/action/tmux.rs` (declare `mod detach_session;`, add `DetachSessionPlugin` to `TmuxActionPlugin`'s `add_plugins`)
- Reference: `src/session/tmux.rs:51-55` (`request_detach`), `src/action/tmux/next_window.rs` (nearest observer shape that looks up `TmuxSession`)

**Interfaces:**
- Produces:
  ```rust
  #[derive(EntityEvent, Debug, Clone)]
  pub(crate) struct DetachSessionRequest { #[event_target] pub(crate) entity: Entity }
  pub(crate) struct DetachSessionPlugin;
  ```

- [ ] **Step 1: Write the failing test**

In `detach_session.rs` `#[cfg(test)] mod tests`, assert the observer runs without panicking when the target has a `TmuxSession` but no `TmuxClient` (mirrors `next_window.rs`'s test harness for a client-less trigger):

```rust
#[test]
fn detach_request_without_client_is_noop() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins).add_plugins(DetachSessionPlugin);
    let e = app.world_mut().spawn(/* TmuxSession fixture per next_window.rs */).id();
    app.world_mut().trigger(DetachSessionRequest { entity: e });
    app.update(); // must not panic
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p orzma --lib action::tmux::detach_session`
Expected: FAIL — module/type not found.

- [ ] **Step 3: Implement the event, observer, plugin**

```rust
//! Detach-session shortcut action: triggers `detach-client` on the session.
// imports (single block): bevy prelude, Single<&mut TmuxClient>, Query<&TmuxSession>, request_detach
pub(crate) struct DetachSessionPlugin;
impl Plugin for DetachSessionPlugin {
    fn build(&self, app: &mut App) { app.add_observer(on_detach_session); }
}
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct DetachSessionRequest { #[event_target] pub(crate) entity: Entity }
fn on_detach_session(
    ev: On<DetachSessionRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    sessions: Query<&TmuxSession>,
) {
    if sessions.get(ev.entity).is_err() { return; }
    if let Some(client) = client.as_deref_mut() { request_detach(client); }
}
```
`request_detach` stays in `src/session/tmux.rs` (already `pub(crate)`); this observer calls it. In `src/action/tmux.rs`: `mod detach_session;`, add `DetachSessionPlugin` to `TmuxActionPlugin`'s tuple, AND re-export the event — `pub(crate) use detach_session::DetachSessionRequest;` — alongside the existing `*Request` re-exports (`src/action/tmux.rs:19`), so the tmux applier (Task 6) can import it via `crate::action::tmux`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p orzma --lib action::tmux::detach_session`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p orzma --all-targets && cargo fmt
git add src/action/tmux/detach_session.rs src/action/tmux.rs
git commit -m "feat(action): add DetachSessionRequest event + observer"
```

---

### Task 3: `ForwardPaneKeysRequest` batch event + observer

**Files:**
- Modify: `src/input/tmux/forward.rs` (add the event, observer, and register it in `ForwardPlugin`)
- Reference: `src/input/tmux/input.rs:411-431` (the batch send + `snap_to_bottom_vt_only`/`flush_emit` to relocate); `src/input/tmux/mouse/effect.rs:54-59` + `mouse/apply.rs` (batch-event precedent)

**Interfaces:**
- Produces:
  ```rust
  #[derive(EntityEvent, Debug, Clone)]
  pub(crate) struct ForwardPaneKeysRequest {
      #[event_target] pub(crate) entity: Entity,   // the active pane surface
      pub(crate) names: Vec<String>,
  }
  ```

- [ ] **Step 1: Write the failing test**

Assert the observer, given a pane entity with a `TerminalHandle` and a `TmuxClient` present, issues exactly one `SendPaneKeys` (use the existing forward.rs / input.rs test scaffolding for a `TmuxClient` capture; if none exists, assert no-panic with a client-less trigger and cover the send in the Task 6 integration test).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p orzma --lib input::tmux::forward`
Expected: FAIL — type not found.

- [ ] **Step 3: Implement the event + observer**

```rust
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ForwardPaneKeysRequest { #[event_target] pub(crate) entity: Entity, pub(crate) names: Vec<String> }

fn on_forward_pane_keys(
    ev: On<ForwardPaneKeysRequest>,
    mut commands: Commands,
    mut handles: Query<&mut TerminalHandle>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    if ev.names.is_empty() { return; }
    let Ok(pane) = panes.get(ev.entity) else { return; };
    let target = format!("%{}", pane.id.0);
    if let Ok(mut handle) = handles.get_mut(ev.entity)
        && handle.snap_to_bottom_vt_only() { handle.flush_emit(&mut commands, ev.entity); }
    if let Some(client) = client.as_deref_mut()
        && let Err(e) = client.send(SendPaneKeys { pane: &target, names: &ev.names }) {
        tracing::warn!(?e, "tmux key forward failed");
    }
}
```
Register `.add_observer(on_forward_pane_keys)` in `ForwardPlugin::build`. NOTE: this observer snap/flushes BEFORE the send, matching the plain-key path (`input.rs:411-431`). Task 6 also routes the webview-forward batch through this observer; today the webview path sends FIRST then snap/flushes (`input.rs:213-228`), so that one path's snap/flush-vs-send order flips. Accepted as user-invisible (snap-to-bottom + a frame emit are idempotent w.r.t. the pane bytes already queued); do not add a separate webview send path to preserve the old order.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p orzma --lib input::tmux::forward`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p orzma --all-targets && cargo fmt
git add src/input/tmux/forward.rs
git commit -m "feat(input): add ForwardPaneKeysRequest batch event + observer"
```

---

### Task 4: Tmux paste observer + `Without<TmuxPane>` guard

**Files:**
- Create: `src/action/tmux/paste.rs` (`on_paste_tmux` + `TmuxPastePlugin`)
- Modify: `src/action/tmux.rs` (declare `mod paste;`, add `TmuxPastePlugin`)
- Modify: `src/action/terminal/paste.rs` (add `Without<TmuxPane>` to `on_paste`'s query)
- Reference: `src/input/tmux/input.rs:281-308` (clipboard → chunked `SendBytes`, `PASTE_CHUNK_BYTES`); `src/action/terminal/paste.rs:26` (existing `on_paste`); `src/clipboard.rs` (`build_paste_bytes`)

**Interfaces:**
- Consumes: `PasteAction` (existing, `src/action/terminal/paste.rs`).

- [ ] **Step 1: Write the failing test**

In `src/action/terminal/paste.rs` tests, add `on_paste_is_noop_for_tmux_pane` — trigger `PasteAction` on an entity with `TmuxPane` (no `PtyHandle`) and assert the PTY-write path is not taken (mirrors `ime_commit_is_noop_for_tmux_pane_target`). In `src/action/tmux/paste.rs`, add a test that `on_paste_tmux` chunks a >`PASTE_CHUNK_BYTES` clipboard into multiple `SendBytes` (or no-panic without a client if a client capture harness is unavailable).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p orzma --lib action` (single filter — cargo test takes ONE testname; two positional filters is invalid)
Expected: FAIL — `on_paste_tmux` missing; the `Without<TmuxPane>` assertion may already pass because `on_paste` needs `PtyHandle` (document that the filter is defensive, not the sole guard).

- [ ] **Step 3: Implement `on_paste_tmux` + the filter**

Add `Without<TmuxPane>` to `on_paste`'s `Query` filter in `terminal/paste.rs`. Create `src/action/tmux/paste.rs`:

```rust
//! Tmux paste: applies `PasteAction` for a tmux pane by chunking the clipboard into `SendBytes`.
const PASTE_CHUNK_BYTES: usize = 256;
pub(crate) struct TmuxPastePlugin;
impl Plugin for TmuxPastePlugin { fn build(&self, app: &mut App) { app.add_observer(on_paste_tmux); } }
fn on_paste_tmux(
    ev: On<PasteAction>,
    mut commands: Commands,
    mut clipboard: ResMut<Clipboard>,
    mut handles: Query<&mut TerminalHandle>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else { return; };   // plain terminal -> on_paste handles it
    let Some(text) = clipboard.read() else { return; };
    if text.is_empty() { return; }
    let target = format!("%{}", pane.id.0);
    if let Ok(mut handle) = handles.get_mut(ev.entity)
        && handle.snap_to_bottom_vt_only() { handle.flush_emit(&mut commands, ev.entity); }
    let bytes = build_paste_bytes(&text, false);
    if let Some(client) = client.as_deref_mut() {
        for chunk in bytes.chunks(PASTE_CHUNK_BYTES) {
            if let Err(e) = client.send(SendBytes { pane: &target, bytes: chunk }) { tracing::warn!(?e, "paste send failed"); break; }
        }
    }
}
```
Move `PASTE_CHUNK_BYTES` here (delete the copy in `input.rs` in Task 6). Add `TmuxPastePlugin` to `TmuxActionPlugin`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p orzma --lib action` (single filter — cargo test takes ONE testname; two positional filters is invalid)
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p orzma --all-targets && cargo fmt
git add src/action/tmux/paste.rs src/action/tmux.rs src/action/terminal/paste.rs
git commit -m "feat(action): tmux paste observer + Without<TmuxPane> guard on on_paste"
```

---

### Task 5: Swap Default — `apply_default_shortcuts`, delete `dispatch_input`

The atomic Default cutover: delete the two old Default keyboard systems, add the new applier, and remove the now-dead keyboard-binding machinery. Default mode must work after this task; tmux still runs on the old `forward_keys_to_tmux`.

**Files:**
- Modify: `src/input/default_mode.rs` (replace `app_shortcut_handler` with `apply_default_shortcuts`; update `DefaultHostInputPlugin`)
- Modify: `src/input/keyboard.rs` (delete `dispatch_input` + its tests; keep `bevy_key_to_terminal_key`, `current_terminal_modifiers`; trim `KeyboardInputPlugin` to `add_message::<KeyboardInput>()`)
- Modify: `src/input/shortcuts.rs` (remove `LeaderGate::Read`; delete `input_bindings`, `opens_repeat_window`, `populate_input_bindings`; drop `TerminalInputBindings` init from the startup chain)
- Modify: `src/input/bindings.rs` (delete `TerminalInputBindings` + `ReservedChord`; keep mouse policy)
- Reference: `src/input/default_mode.rs:163-287` (old `app_shortcut_handler` — the source of the applier's match arms)

**Interfaces:**
- Consumes: `classify_key_batch`, `KeyEffect`, `BatchContext` (Task 1); `PasteAction`, `EnterCopyModeActionEvent`, `TerminalKeyInput`, `trigger_copy_mode_action`; `bevy_key_to_terminal_key`, `current_terminal_modifiers`.
- Produces: `apply_default_shortcuts` (system, registered `run_if(in_state(AppMode::Default)).run_if(on_message::<KeyboardInput>)` in `InputPhase::FocusedKey` + `LeaderGate::Advance`).

- [ ] **Step 1: Update the Default integration tests (failing)**

Rework `repeat_dispatch_app` in `default_mode.rs` to register `apply_default_shortcuts` (not `app_shortcut_handler`) with a `KeyboardFocused OrzmaTerminal`, capturing `EnterCopyModeActionEvent`, `PasteAction`, `TerminalKeyInput`, and `AppExit`. Keep the ported `os_key_repeat_*` assertions and add: `plain_key_triggers_terminal_key_input`, `pane_action_is_noop` (a `<Leader>h` fires no event), `direct_paste_outside_copy_mode_pastes`, `direct_paste_in_copy_mode_suppressed`, `leader_paste_in_copy_mode_pastes`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p orzma --lib input::default_mode`
Expected: FAIL — `apply_default_shortcuts` not defined.

- [ ] **Step 3: Implement `apply_default_shortcuts`; delete `app_shortcut_handler`**

Replace the `app_shortcut_handler` fn with `apply_default_shortcuts`. Body ≤150 lines: guard (IME/focus → `clear_leader_phase` + `events.clear()` + return) → resolve `entity` (the `KeyboardFocused OrzmaTerminal`), `in_copy_mode`, `webview_focused` → build `BatchContext` (`forward_chords: &[]`) → `let effects = classify_key_batch(&mut leader_phase, &shortcuts, &resolved_copy, events.read(), ctx)` → for each effect:

```rust
match effect {
    KeyEffect::Action { action: Shortcut::Quit, .. } => exit.write(AppExit::Success),
    KeyEffect::Action { action: Shortcut::EnterCopyMode, .. } =>
        commands.trigger(EnterCopyModeActionEvent { entity }),
    KeyEffect::Action { action: Shortcut::Paste, via_leader } =>
        if via_leader || !in_copy_mode { commands.trigger(PasteAction { entity }); },
    KeyEffect::Action { .. } => {} // pane/window/detach/release: Default no-op
    KeyEffect::CopyMode(a) => trigger_copy_mode_action(&mut commands, entity, a),
    KeyEffect::Type { logical, key_code } =>
        // NOTE: drop the release-webview-focus chord — in Default it was a
        // reserved chord withheld from the PTY by `dispatch_input`; it is the
        // one direct chord the decider emits as `Type` (all others resolve to
        // `Action`). tmux forwards it instead (Task 6), so the drop is here.
        if !shortcuts.is_release_webview_focus(key_code, mods)
            && let Some(key) = bevy_key_to_terminal_key(&logical) {
            commands.trigger(TerminalKeyInput { entity, key, modifiers: current_terminal_modifiers(&bevy_keys) });
        },
    KeyEffect::ReleaseWebviewFocus => focused_webview.0 = None,
    KeyEffect::WebviewForward { .. } => {} // Default never emits this
}
```
Register `apply_default_shortcuts` in `DefaultHostInputPlugin::build` with the same sets/run_ifs the old system had. Delete `gui_action_suppressed_by_webview` if it becomes unused (the decider now owns webview suppression). Keep `maintain_input_gates` and `apply_ime_commit_to_terminal` unchanged.

- [ ] **Step 4: Delete `dispatch_input` and the dead binding machinery**

**Before deleting `dispatch_input`, verify it is inert in tmux mode** (spec requirement): confirm every tmux pane is unconditionally `KeyboardDisabled` (`src/input/tmux/gate.rs:90-93`) and `on_terminal_key_input` needs a `PtyHandle` that tmux panes lack — so `dispatch_input` never typed to a pane, and the Default applier being `run_if(in_state(Default))` (inert in tmux) removes no live typing path. In `keyboard.rs`: delete `dispatch_input`, `chord_matches`, and every `#[test]` exercising `dispatch_input`; keep `current_terminal_modifiers` (already `pub(crate)`) and `bevy_key_to_terminal_key` — **change `bevy_key_to_terminal_key` to `pub(crate)`** (the Default applier in `default_mode.rs` now calls it cross-module) — and their tests. Trim `KeyboardInputPlugin::build` to `app.add_message::<KeyboardInput>();` (drop `init_resource::<TerminalInputBindings>()` and the `dispatch_input` registration). In `shortcuts.rs`: delete `Shortcuts::input_bindings`, `Shortcuts::opens_repeat_window`, `populate_input_bindings`, remove them from the `Startup` chain, and change `LeaderGate` to `enum LeaderGate { Detect, Advance }` with `(LeaderGate::Detect, LeaderGate::Advance).chain()`; update the doc comments that referenced `Read`/`dispatch_input`. In `bindings.rs`: delete `TerminalInputBindings` and `ReservedChord`; keep `OrzmaMouseConfig`/`FineModifier`. Fix all resulting `use`/import breakage.

- [ ] **Step 5: Run tests + verify Default mode**

Run: `cargo test -p orzma --lib input && cargo build -p orzma`
Expected: PASS + clean build. Then run the app in Default mode (`cargo run`) and confirm: typing reaches the shell, `<Leader>` sequences fire, Cmd+Q quits, copy-mode entry works, Cmd+V pastes (and does NOT paste in copy mode), Ctrl+Shift+Escape does not type Escape.

- [ ] **Step 6: Lint + commit**

```bash
cargo clippy --workspace --all-targets && cargo fmt
git add -A
git commit -m "refactor(input): swap Default to apply_default_shortcuts, delete dispatch_input"
```

---

### Task 6: Swap Tmux — `apply_tmux_shortcuts`

Replace the keyboard body of `forward_keys_to_tmux` with the thin applier; keep `forward_wheel_to_tmux`. Tmux mode must work after this task, completing the refactor.

**Files:**
- Modify: `src/input/tmux/input.rs` (replace `forward_keys_to_tmux` with `apply_tmux_shortcuts`; delete the local `PASTE_CHUNK_BYTES`; keep `forward_wheel_to_tmux`, `ActionTargets`, `TmuxWheelAccumulator`, the direction mappers `tmux_pane_direction`/`tmux_split_direction`)
- Reference: `src/input/tmux/input.rs:87-432` (old system — the source of the applier's arms and guards)

**Interfaces:**
- Consumes: `classify_key_batch`/`KeyEffect`/`BatchContext`; `DetachSessionRequest` (Task 2); `ForwardPaneKeysRequest` (Task 3); `PasteAction` (Task 4 tmux observer); the existing `*Request` events; `EnterCopyModeActionEvent`; `trigger_copy_mode_action`; `bevy_key_to_tmux_name`.
- Produces: `apply_tmux_shortcuts` (system, registered `run_if(in_state(AppMode::Tmux)).run_if(on_message::<KeyboardInput>)`, `InputPhase::FocusedKey` + `LeaderGate::Advance` + `TmuxActiveSet`).

- [ ] **Step 1: Update tmux integration tests (failing)**

Add tests registering `apply_tmux_shortcuts` with an `ActivePane`/`TmuxPane` fixture, capturing the `*Request` events + `ForwardPaneKeysRequest`: `leader_h_triggers_select_pane_left`, `quit_writes_appexit`, `plain_keys_batch_into_one_forward_request`, `detach_triggers_detach_session_request`, `select_window_targets_indexed_window`. Reuse the existing tmux test scaffolding in `input.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p orzma --lib input::tmux::input`
Expected: FAIL — `apply_tmux_shortcuts` not defined.

- [ ] **Step 3: Implement `apply_tmux_shortcuts`; delete `forward_keys_to_tmux`**

Body ≤150 lines: guards (drain+return for `CopyPrompt`/`ConfirmState`/`RenamePrompt`/IME/unfocused — port verbatim from `input.rs:113-144`) → resolve `active_entity`/`in_copy_mode`/`webview_focused`/`forward_chords`/targets → `BatchContext` → `classify_key_batch(...)` → accumulate a `Vec<String>` names batch while iterating:

```rust
let mut names: Vec<String> = Vec::new();
for effect in effects {
    match effect {
        KeyEffect::Action { action: Shortcut::Quit, .. } => exit.write(AppExit::Success),
        KeyEffect::Action { action: Shortcut::EnterCopyMode, .. } =>
            if let Some(e) = active_entity && copy_modes.get(e).is_err() { commands.trigger(EnterCopyModeActionEvent { entity: e }); },
        KeyEffect::Action { action: Shortcut::Paste, .. } =>
            if let Some(e) = active_entity { commands.trigger(PasteAction { entity: e }); },
        KeyEffect::Action { action: Shortcut::DetachSession, .. } =>
            if let Ok(e) = targets.session.single() { commands.trigger(DetachSessionRequest { entity: e }); },
        KeyEffect::Action { action, .. } => dispatch_tmux_action(&mut commands, action, active_entity, &targets), // SelectPane/Split/Kill/Zoom/New/Next/Prev/Select/KillWindow/Rename* — port arms from input.rs:324-389
        KeyEffect::CopyMode(a) => if let Some(e) = active_entity { trigger_copy_mode_action(&mut commands, e, a); },
        KeyEffect::Type { logical, key_code } =>
            if let Some(name) = bevy_key_to_tmux_name(&logical, key_code, kmods) { names.push(name); },
        KeyEffect::WebviewForward { logical, key_code } =>
            if let Some(name) = bevy_key_to_tmux_name(&logical, key_code, kmods) { names.push(name); },
        KeyEffect::ReleaseWebviewFocus => focused_webview.0 = None,
    }
}
if let Some(e) = active_entity && !names.is_empty() { commands.trigger(ForwardPaneKeysRequest { entity: e, names }); }
```
Extract `dispatch_tmux_action` (the 11 pane/window arms → `*Request` triggers, port from `input.rs:324-389`) as a private helper to keep the body under the cap. Delete the old `forward_keys_to_tmux` and the local `PASTE_CHUNK_BYTES`. Register `apply_tmux_shortcuts` in `InputPlugin::build` alongside the retained `forward_wheel_to_tmux`, preserving the exact sets/run_ifs.

- [ ] **Step 4: Run tests + verify Tmux mode**

Run: `cargo test -p orzma --lib input::tmux && cargo build -p orzma`
Expected: PASS + clean build. Then attach a `tmux -CC` session (`cargo run`) and confirm: typing reaches the pane, `<Leader>` pane/window ops work, paste, detach (Ctrl+Shift+D), copy-mode entry + vi nav, webview forward-keys, and that plain keys still batch (one `SendPaneKeys` per frame — check `RUST_LOG=orzma_tmux=trace`).

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy --workspace --all-targets && cargo fmt
git add -A
git commit -m "refactor(input): swap Tmux to apply_tmux_shortcuts, delete forward_keys_to_tmux keyboard path"
```

---

### Task 7: Final gate + dead-code sweep

**Files:** whole workspace (verification only; small deletions if the sweep finds leftovers)

- [ ] **Step 1: Dead-code + visibility sweep**

Grep for any remaining references to the deleted items and confirm none survive:
```bash
rg -n 'dispatch_input|app_shortcut_handler|forward_keys_to_tmux|TerminalInputBindings|ReservedChord|input_bindings|opens_repeat_window|LeaderGate::Read' src crates
```
Expected: only the new decider/appliers and doc/spec references (no live code). Demote any item now used in only one module to private (MANDATORY visibility rule); confirm `KeyEffect`/`BatchContext`/`classify_key_batch` are `pub(crate)` (used cross-module by both appliers) and `step_with_repeat` is private.

- [ ] **Step 2: Full test + lint gate**

Run: `cargo test` then `cargo clippy --workspace --all-targets` then `cargo fmt --check`
Expected: all green, zero warnings.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore(input): dead-code + visibility sweep for unified shortcut dispatch"
```

---

## Self-Review

**Spec coverage:** L1 decider → Task 1; L2 Default applier → Task 5; L2 tmux applier → Task 6; L3 `DetachSessionRequest` → Task 2, `ForwardPaneKeysRequest` → Task 3, tmux paste observer + filter → Task 4; deletions/`LeaderGate::Read`/binding trim → Tasks 5+7; behaviour-preservation invariants (release-webview swallow, no-Type-in-copy-mode, via_leader paste, EnterCopyMode per-mode) → Task 1 tests + Task 5/6 applier arms; testing strategy → each task's TDD steps. All spec sections mapped.

**Placeholder scan:** No "TBD"/"add error handling"/"similar to Task N". Port-heavy steps cite exact source line ranges to relocate and state the precise deltas; new code is shown in full. This is deliberate for a behaviour-preserving port — the implementer relocates verified logic rather than inventing it.

**Type consistency:** `classify_key_batch`/`KeyEffect`/`BatchContext` signatures are identical in Task 1's Produces block and their Task 5/6 Consumes usage. `DetachSessionRequest { entity }`, `ForwardPaneKeysRequest { entity, names }`, `PasteAction { entity }`, `EnterCopyModeActionEvent { entity }` field names match across producer and observer tasks. `via_leader` is a `bool` field on `KeyEffect::Action` throughout.
