# Shortcut Message Fan-out Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single `ShortcutBatch` transport between `resolve_key_effects` and the per-mode shortcut appliers with four typed, context-carrying messages (`ShortcutMessage`, `CopyModeMessage`, `TypeMessage`, `WebviewForwardMessage`), one per responsibility, consumed by per-responsibility appliers.

**Architecture:** The pure decider `classify_key_batch` still returns `Vec<KeyEffect>` (its `Action` variant renamed to `Shortcut`). `resolve_key_effects` fans each remaining effect out to its typed message (stamping frame context: `focused`, `in_copy_mode`, `mods`). Per-mode plugins register split appliers, each gated on its own message and ordered so shortcut/copy-mode application precedes key forwarding. The change lands via a temporary dual-write (producer writes both `ShortcutBatch` and the new messages) so every task leaves the tree compiling and green; the final task removes `ShortcutBatch`.

**Tech Stack:** Rust 2024 (toolchain 1.95), Bevy 0.18 ECS (`Message` / `MessageReader` / `MessageWriter` / `SystemParam` / `SystemCondition::or` / `on_message`).

## Global Constraints

- Comments: only `// TODO:` / `// NOTE:` (critical caveat) / `// SAFETY:`. English only. No narrative or commented-out code. (`.claude/rules/rust.md`)
- Every externally-`pub` item gets a `///` summary; each module file keeps its `//!`. New messages are `pub(in crate::input)` (never crate-external), with `pub` fields (the container's narrow visibility caps them — the repo's container exception).
- No `mod.rs`. All `use` at file top in one contiguous block; no inline fully-qualified paths in signatures/bodies/`run_if`/`.after()`.
- Mutable params before immutable ones in every signature. Whole-system change/message guards go in `run_if`, not in-body early returns.
- Plugin registration lives in the file that defines the systems.
- Visibility minimization: anything used only in its defining module stays private.
- Toolchain pinned 1.95; edition 2024.
- Test command for the binary crate: `cargo test -p ozmux <filter>`. Compile check: `cargo build -p ozmux`.

---

### Task 1: Rename `KeyEffect::Action` → `KeyEffect::Shortcut`

Pure variant rename (fields `{ action, via_leader }` unchanged). Touches the decider, its ~30 unit tests, and every `match` arm in the producer and both appliers. No behavior change — the safety net is the existing test suite staying green.

**Files:**
- Modify: `src/input/keyboard/key_effect.rs` (enum def at :19-30, doc comment, and all `KeyEffect::Action` in `#[cfg(test)] mod tests`)
- Modify: `src/input/keyboard/handler.rs` (two `match` arms at :150-160)
- Modify: `src/input/shortcuts/tmux.rs` (`match` arms at :77-107)
- Modify: `src/input/shortcuts/default_mode.rs` (`match` arms at :50-85)
- Test: existing tests in the files above (no new test file)

**Interfaces:**
- Produces: `enum KeyEffect { Shortcut { action: Shortcut, via_leader: bool }, CopyMode(CopyModeAction), Type { logical: Key, key_code: KeyCode }, WebviewForward { logical: Key, key_code: KeyCode } }` — consumed by Tasks 2–5.

- [ ] **Step 1: Rename the variant in the enum definition**

In `src/input/keyboard/key_effect.rs`, rename the `Action` variant to `Shortcut` (keep both fields and their doc comments). The enum becomes:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyEffect {
    /// Run a bound `Shortcut`. `via_leader` distinguishes a leader-scoped
    /// firing from a direct GUI chord — appliers suppress a different subset
    /// of each (e.g. a direct `Paste` fires in copy mode, a leader `Paste`
    /// does not).
    Shortcut {
        /// The action to run.
        action: Shortcut,
        /// Whether the action was reached through the leader (prefix table)
        /// rather than a direct chord.
        via_leader: bool,
    },
    /// Run a matched `[copy-mode]` key.
    CopyMode(CopyModeAction),
    /// Type the key into the focused terminal (Default: the PTY directly;
    /// tmux: forwarded as a `send-keys`).
    Type {
        /// The logical key, for text/printable-key mapping.
        logical: Key,
        /// The physical key, for named-key mapping.
        key_code: KeyCode,
    },
    /// Forward the key to the focused webview's declared forward-key chord.
    WebviewForward {
        /// The logical key, for text/printable-key mapping.
        logical: Key,
        /// The physical key, for named-key mapping.
        key_code: KeyCode,
    },
}
```

- [ ] **Step 2: Update every `KeyEffect::Action` reference to `KeyEffect::Shortcut`**

Replace all remaining `KeyEffect::Action` with `KeyEffect::Shortcut` in these four files (the field names `action` / `via_leader` are unchanged, only the variant name changes):

- `src/input/keyboard/key_effect.rs` — the `classify_key_batch` body (the three `effects.push(KeyEffect::Action { ... })` sites) and every `KeyEffect::Action` inside `#[cfg(test)] mod tests`.
- `src/input/keyboard/handler.rs` — the two arms in the effects loop:

```rust
    for effect in all {
        match effect {
            KeyEffect::Shortcut {
                action: Shortcut::Quit,
                ..
            } => {
                exit.write(AppExit::Success);
            }
            KeyEffect::Shortcut {
                action: Shortcut::ReleaseWebviewFocus,
                ..
            } => focused_webview.0 = None,
            other => effects.push(other),
        }
    }
```

- `src/input/shortcuts/tmux.rs` — the four `KeyEffect::Action` arms in `apply_tmux_shortcuts` become `KeyEffect::Shortcut`.
- `src/input/shortcuts/default_mode.rs` — the three `KeyEffect::Action` arms in `apply_default_shortcuts` become `KeyEffect::Shortcut`.

Use a search for `KeyEffect::Action` across `src/` to confirm zero remain.

- [ ] **Step 3: Verify the tree compiles**

Run: `cargo build -p ozmux`
Expected: builds with no errors.

- [ ] **Step 4: Run the affected tests**

Run: `cargo test -p ozmux input::keyboard`
Expected: PASS — all `key_effect.rs` and `handler.rs` tests green (the rename is behavior-preserving).

- [ ] **Step 5: Commit**

```bash
git add src/input/keyboard/key_effect.rs src/input/keyboard/handler.rs src/input/shortcuts/tmux.rs src/input/shortcuts/default_mode.rs
git commit -m "refactor(input): rename KeyEffect::Action to KeyEffect::Shortcut"
```

---

### Task 2: Add the four messages + `ShortcutMessages` param + producer dual-write

Define the four typed messages and a `ShortcutMessages` writer bundle in `shortcuts.rs`, register them, and make `resolve_key_effects` fan each effect out to its message **in addition to** writing `ShortcutBatch` (dual-write). Consumers still read `ShortcutBatch`, so behavior is unchanged and every test stays green. Replaces the partial `ShortcutMessage` stub currently at `shortcuts.rs:86-93`.

**Files:**
- Modify: `src/input/shortcuts.rs` (replace stub `ShortcutMessage` at :86-93; add `CopyModeMessage`, `TypeMessage`, `WebviewForwardMessage`, `ShortcutMessages`; add four `add_message` registrations in `ShortcutsPlugin::build`)
- Modify: `src/input/keyboard/handler.rs` (`resolve_key_effects`: add the fan-out loop; add `ShortcutMessages` param)
- Test: existing `handler.rs` tests (unchanged — still assert on `ShortcutBatch`)

**Interfaces:**
- Consumes: `KeyEffect` (from Task 1).
- Produces (all `pub(in crate::input)`, consumed by Tasks 3–5):
  - `ShortcutMessage { action: Shortcut, via_leader: bool, focused: Option<Entity>, in_copy_mode: bool }`
  - `CopyModeMessage { action: CopyModeAction, focused: Option<Entity> }`
  - `TypeMessage { logical: Key, key_code: KeyCode, focused: Option<Entity>, mods: Modifiers }`
  - `WebviewForwardMessage { logical: Key, key_code: KeyCode, focused: Option<Entity>, mods: Modifiers }`
  - `ShortcutMessages<'w>` SystemParam bundling the four `MessageWriter`s: fields `shortcut`, `copy_mode`, `type_keys`, `webview_forward`.

- [ ] **Step 1: Replace the `ShortcutMessage` stub and add the other three messages**

In `src/input/shortcuts.rs`, delete the existing stub (lines 86-93):

```rust
#[derive(Message)]
pub struct ShortcutMessage {
    /// The action to run.
    action: Shortcut,
    /// Whether the action was reached through the leader (prefix table)
    /// rather than a direct chord.
    via_leader: bool,
}
```

Replace it with the four messages. `CopyModeAction`, `Key`, `KeyCode` must be imported (see Step 2). Place these just below the `ShortcutBatch` definition:

```rust
/// One resolved keyboard shortcut action, fanned out from `resolve_key_effects`
/// to the per-mode appliers. Excludes `Quit` / `ReleaseWebviewFocus` (handled
/// inline in `resolve_key_effects`). `focused` is the `KeyboardFocused` surface;
/// `in_copy_mode` gates the copy-mode re-entry and paste-suppression rules.
#[derive(Message)]
pub(in crate::input) struct ShortcutMessage {
    /// The action to run.
    pub action: Shortcut,
    /// Whether the action was reached through the leader rather than a direct chord.
    pub via_leader: bool,
    /// The `KeyboardFocused` surface, or `None` when none is focused.
    pub focused: Option<Entity>,
    /// Whether the focused surface is in copy mode.
    pub in_copy_mode: bool,
}

/// One matched `[copy-mode]` key, fanned out to the per-mode appliers.
#[derive(Message)]
pub(in crate::input) struct CopyModeMessage {
    /// The copy-mode action to run.
    pub action: CopyModeAction,
    /// The `KeyboardFocused` surface, or `None` when none is focused.
    pub focused: Option<Entity>,
}

/// One raw key to type into / forward to the focused terminal.
#[derive(Message)]
pub(in crate::input) struct TypeMessage {
    /// The logical key, for text/printable-key mapping.
    pub logical: Key,
    /// The physical key, for named-key mapping.
    pub key_code: KeyCode,
    /// The `KeyboardFocused` surface, or `None` when none is focused.
    pub focused: Option<Entity>,
    /// The frame's modifier snapshot.
    pub mods: Modifiers,
}

/// One key to forward to the focused webview's declared forward-key chord.
#[derive(Message)]
pub(in crate::input) struct WebviewForwardMessage {
    /// The logical key, for text/printable-key mapping.
    pub logical: Key,
    /// The physical key, for named-key mapping.
    pub key_code: KeyCode,
    /// The `KeyboardFocused` surface, or `None` when none is focused.
    pub focused: Option<Entity>,
    /// The frame's modifier snapshot.
    pub mods: Modifiers,
}

/// The four shortcut-effect message writers `resolve_key_effects` fans out to,
/// bundled to stay within Bevy's system-parameter limit.
#[derive(SystemParam)]
pub(in crate::input) struct ShortcutMessages<'w> {
    pub shortcut: MessageWriter<'w, ShortcutMessage>,
    pub copy_mode: MessageWriter<'w, CopyModeMessage>,
    pub type_keys: MessageWriter<'w, TypeMessage>,
    pub webview_forward: MessageWriter<'w, WebviewForwardMessage>,
}
```

- [ ] **Step 2: Add the required imports to `shortcuts.rs`**

`ShortcutBatch` already brings in `KeyEffect` and `Modifiers`. Add `CopyModeAction`, `Key`, `KeyCode`, and `SystemParam` / `MessageWriter` (the latter two are in `bevy::prelude::*`, already glob-imported). Extend the existing `use` block (keep it one contiguous block, no blank lines between groups):

- Add `use bevy::ecs::system::SystemParam;` (mirrors `handler.rs`).
- Add `use bevy::input::keyboard::Key;` (`KeyCode` and `KeyboardInput` are already imported at :14).
- Add `use ozmux_configs::copy_mode::CopyModeAction;`.

`MessageWriter`, `Entity`, and `Message` come from `bevy::prelude::*` (already imported at :16).

- [ ] **Step 3: Register the four messages in `ShortcutsPlugin::build`**

In `src/input/shortcuts.rs`, in the `Plugin::build` chain (currently `.add_message::<ShortcutBatch>()` at :41), add the four registrations right after it (keep `ShortcutBatch` for now — it is removed in Task 5):

```rust
            .add_message::<ShortcutBatch>()
            .add_message::<ShortcutMessage>()
            .add_message::<CopyModeMessage>()
            .add_message::<TypeMessage>()
            .add_message::<WebviewForwardMessage>()
```

- [ ] **Step 4: Add the `ShortcutMessages` param and dual-write fan-out to `resolve_key_effects`**

In `src/input/keyboard/handler.rs`:

Import the new types — extend the existing `use crate::input::shortcuts::{ ... }` block at :17-19 to add `CopyModeMessage`, `ShortcutMessages`, `ShortcutMessage`, `TypeMessage`, `WebviewForwardMessage`.

Add the writer bundle as a **mutable** param (mutable params come first — insert it directly after `mut batch: MessageWriter<ShortcutBatch>` at :86, before the immutable `guards`/`inputs`/queries):

```rust
    mut batch: MessageWriter<ShortcutBatch>,
    mut messages: ShortcutMessages,
    guards: ModalGuards,
```

Immediately before the existing `batch.write(ShortcutBatch { ... })` (at :163), add a fan-out loop that reads `&effects` by reference (so `effects` can still move into the batch below):

```rust
    for effect in &effects {
        match effect {
            KeyEffect::Shortcut { action, via_leader } => {
                messages.shortcut.write(ShortcutMessage {
                    action: *action,
                    via_leader: *via_leader,
                    focused,
                    in_copy_mode,
                });
            }
            KeyEffect::CopyMode(action) => {
                messages.copy_mode.write(CopyModeMessage {
                    action: *action,
                    focused,
                });
            }
            KeyEffect::Type { logical, key_code } => {
                messages.type_keys.write(TypeMessage {
                    logical: logical.clone(),
                    key_code: *key_code,
                    focused,
                    mods,
                });
            }
            KeyEffect::WebviewForward { logical, key_code } => {
                messages.webview_forward.write(WebviewForwardMessage {
                    logical: logical.clone(),
                    key_code: *key_code,
                    focused,
                    mods,
                });
            }
        }
    }
    batch.write(ShortcutBatch {
        effects,
        focused,
        in_copy_mode,
        mods,
    });
```

- [ ] **Step 5: Verify compile and full green**

Run: `cargo build -p ozmux`
Expected: builds clean (no dead-code warnings — every message type is constructed and registered).

Run: `cargo test -p ozmux input::`
Expected: PASS — existing tests unaffected (consumers still read `ShortcutBatch`).

- [ ] **Step 6: Commit**

```bash
git add src/input/shortcuts.rs src/input/keyboard/handler.rs
git commit -m "feat(input): add per-responsibility shortcut messages, dual-write from resolve"
```

---

### Task 3: Split the tmux appliers to read the new messages

Replace the single `apply_tmux_shortcuts` (reading `ShortcutBatch`) with three appliers reading the new messages, ordered so shortcut/copy application precedes forwarding. Update the tmux test harness. The producer still dual-writes `ShortcutBatch`; Default still reads it — so the tree stays green.

**Files:**
- Modify: `src/input/shortcuts/tmux.rs` (rewrite `ShortcutsTmuxModePlugin::build`; rewrite `apply_tmux_shortcuts`; add `apply_tmux_copy_mode`, `apply_tmux_forward`, `on_tmux_forward_message`, `push_forward_name`; update imports)
- Modify: `src/input/tmux/input.rs` (test harness `dispatch` at :883-889 and the app builder at :841-849 — write the new messages instead of `ShortcutBatch`)
- Test: existing tests in `src/input/tmux/input.rs`

**Interfaces:**
- Consumes: `ShortcutMessage`, `CopyModeMessage`, `TypeMessage`, `WebviewForwardMessage` (from Task 2); `ActionTargets`, `dispatch_tmux_action` (unchanged, same file).
- Produces: no new cross-task interface (internal appliers).

- [ ] **Step 1: Rewrite the tmux plugin registration**

In `src/input/shortcuts/tmux.rs`, replace `ShortcutsTmuxModePlugin::build` with the three ordered, per-message-gated systems (each mirrors the existing per-system condition style; `apply_tmux_forward` runs after the other two to preserve today's "forward after shortcuts" command order):

```rust
impl Plugin for ShortcutsTmuxModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                apply_tmux_shortcuts
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_message::<ShortcutMessage>),
                apply_tmux_copy_mode
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_message::<CopyModeMessage>),
                apply_tmux_forward
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_tmux_forward_message)
                    .after(apply_tmux_shortcuts)
                    .after(apply_tmux_copy_mode),
            )
                .in_set(TmuxActiveSet),
        );
    }
}

/// Run condition for `apply_tmux_forward`: true on any frame carrying a key to
/// forward (typed or webview-forwarded). The two never coexist in a frame.
fn on_tmux_forward_message() -> impl SystemCondition<()> {
    on_message::<TypeMessage>.or(on_message::<WebviewForwardMessage>)
}
```

- [ ] **Step 2: Rewrite `apply_tmux_shortcuts` to read `ShortcutMessage`**

Replace the whole `apply_tmux_shortcuts` fn body. It now reads per-shortcut messages (each carries its own `focused` / `in_copy_mode`); the forwarding responsibility moves to `apply_tmux_forward`:

```rust
/// Applies tmux keyboard shortcuts from `ShortcutMessage`: copy-mode entry,
/// paste (`PasteAction`), detach (`DetachSessionRequest`), and the pane/window
/// action requests. `Quit` / `ReleaseWebviewFocus` are handled upstream in
/// `resolve_key_effects`. Registered in `ShortcutSet::Apply`, gated on
/// `in_state(AppMode::Tmux)` + `on_message::<ShortcutMessage>`, ordered before
/// `apply_tmux_forward`.
pub(in crate::input) fn apply_tmux_shortcuts(
    mut commands: Commands,
    mut shortcuts: MessageReader<ShortcutMessage>,
    targets: ActionTargets,
) {
    for msg in shortcuts.read() {
        match msg.action {
            Shortcut::EnterCopyMode => {
                // NOTE: re-entry guard — re-triggering while already in copy
                // mode would double-insert CopyModeState and re-enter vi mode.
                if let Some(entity) = msg.focused
                    && !msg.in_copy_mode
                {
                    commands.trigger(EnterCopyModeActionEvent { entity });
                }
            }
            Shortcut::Paste => {
                if let Some(entity) = msg.focused {
                    commands.trigger(PasteAction { entity });
                }
            }
            Shortcut::DetachSession => {
                if let Ok(entity) = targets.session.single() {
                    commands.trigger(DetachSessionRequest { entity });
                }
            }
            action => dispatch_tmux_action(&mut commands, action, msg.focused, &targets),
        }
    }
}
```

- [ ] **Step 3: Add `apply_tmux_copy_mode` and `apply_tmux_forward`**

Add these two systems below `apply_tmux_shortcuts` (before the private `dispatch_tmux_action` helper, keeping exported-then-private order):

```rust
/// Applies matched `[copy-mode]` keys from `CopyModeMessage` on the focused
/// pane. Registered in `ShortcutSet::Apply`, gated on `in_state(AppMode::Tmux)`
/// + `on_message::<CopyModeMessage>`.
fn apply_tmux_copy_mode(
    mut commands: Commands,
    mut copy_mode: MessageReader<CopyModeMessage>,
) {
    for msg in copy_mode.read() {
        if let Some(entity) = msg.focused {
            trigger_copy_mode_action(&mut commands, entity, msg.action);
        }
    }
}

/// Forwards typed / webview-forwarded keys to the focused pane as one
/// `ForwardPaneKeysRequest` per pane. `TypeMessage` and `WebviewForwardMessage`
/// never coexist in a frame, so at most one reader is non-empty. Runs after the
/// shortcut/copy appliers so their triggers are queued first (parity with the
/// old single-system order). Gated on `on_tmux_forward_message`.
fn apply_tmux_forward(
    mut commands: Commands,
    mut type_keys: MessageReader<TypeMessage>,
    mut webview_forward: MessageReader<WebviewForwardMessage>,
) {
    let mut by_pane: HashMap<Entity, Vec<String>> = HashMap::new();
    for msg in type_keys.read() {
        push_forward_name(&mut by_pane, msg.focused, &msg.logical, msg.key_code, msg.mods);
    }
    for msg in webview_forward.read() {
        push_forward_name(&mut by_pane, msg.focused, &msg.logical, msg.key_code, msg.mods);
    }
    for (entity, names) in by_pane {
        if !names.is_empty() {
            commands.trigger(ForwardPaneKeysRequest { entity, names });
        }
    }
}

/// Appends the tmux key name for `(logical, key_code, mods)` to `focused`'s
/// per-pane forward list, when the key maps to a name and a pane is focused.
fn push_forward_name(
    by_pane: &mut HashMap<Entity, Vec<String>>,
    focused: Option<Entity>,
    logical: &Key,
    key_code: KeyCode,
    mods: Modifiers,
) {
    let Some(entity) = focused else {
        return;
    };
    let kmods = KeyMods {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        super_: mods.meta,
    };
    if let Some(name) = bevy_key_to_tmux_name(logical, key_code, kmods) {
        by_pane.entry(entity).or_default().push(name);
    }
}
```

- [ ] **Step 4: Update the `tmux.rs` imports**

Update the `use` block at the top of `src/input/shortcuts/tmux.rs`:
- In the `crate::input::{...}` import, drop `keyboard::key_effect::KeyEffect` and `ShortcutBatch`; add `CopyModeMessage`, `ShortcutMessage`, `TypeMessage`, `WebviewForwardMessage`; keep `ShortcutSet`. (`ShortcutMessages` is the producer-side writer bundle and is NOT needed here — consumers use `MessageReader`.)
- Add `use bevy::input::keyboard::Key;` (`KeyCode` is already in `bevy::prelude::*`).
- Add `use ozmux_configs::copy_mode::CopyModeAction;` **only if** `CopyModeMessage`'s field type is referenced directly — it is not here, so skip.
- Add `use ozmux_configs::shortcuts::Modifiers;` (used by `push_forward_name`).
- Add `use std::collections::HashMap;`.
- `KeyMods` and `bevy_key_to_tmux_name` are already imported from `ozmux_tmux` (:24-27). `trigger_copy_mode_action` is already imported (:9). `ForwardPaneKeysRequest` is already imported (:15).
- `SystemCondition` comes from `bevy::prelude::*` (already glob-imported via `bevy::{ecs::system::SystemParam, prelude::*}` at :19).

Confirm no `KeyEffect` / `ShortcutBatch` reference remains in the file.

- [ ] **Step 5: Update the tmux test harness to write the new messages**

In `src/input/tmux/input.rs`, the test module builds an app that runs `apply_tmux_shortcuts` and writes a `ShortcutBatch`. Update it to register + write the new messages and run all three appliers.

At the app builder (around :841-849), replace `.add_message::<ShortcutBatch>()` and the single `apply_tmux_shortcuts` registration with the three messages and three systems (mirror the real plugin ordering). At the `dispatch` helper (around :883-889), replace the `ShortcutBatch` write. Since the harness previously drove a `Vec<KeyEffect>`, translate each effect to its message. Replace the helper with:

```rust
    fn dispatch(app: &mut App, effects: Vec<KeyEffect>, focused: Option<Entity>) {
        let mods = Modifiers::default();
        for effect in effects {
            match effect {
                KeyEffect::Shortcut { action, via_leader } => {
                    app.world_mut().write_message(ShortcutMessage {
                        action,
                        via_leader,
                        focused,
                        in_copy_mode: false,
                    });
                }
                KeyEffect::CopyMode(action) => {
                    app.world_mut().write_message(CopyModeMessage { action, focused });
                }
                KeyEffect::Type { logical, key_code } => {
                    app.world_mut().write_message(TypeMessage {
                        logical,
                        key_code,
                        focused,
                        mods,
                    });
                }
                KeyEffect::WebviewForward { logical, key_code } => {
                    app.world_mut().write_message(WebviewForwardMessage {
                        logical,
                        key_code,
                        focused,
                        mods,
                    });
                }
            }
        }
        app.update();
    }
```

Update that test module's `use` / registration accordingly (register the four messages; add the three systems with the same `.after()` ordering as the real plugin; keep `KeyEffect` imported since the harness signature still takes `Vec<KeyEffect>`). If any test asserted `in_copy_mode = true` behavior through the batch, set that test's `ShortcutMessage.in_copy_mode` accordingly — search the test bodies for `in_copy_mode` usage and preserve each test's intent.

- [ ] **Step 6: Verify compile and green**

Run: `cargo build -p ozmux`
Expected: clean.

Run: `cargo test -p ozmux input::tmux`
Expected: PASS — the tmux appliers behave identically; forwarding still yields one `ForwardPaneKeysRequest` per focused pane.

- [ ] **Step 7: Commit**

```bash
git add src/input/shortcuts/tmux.rs src/input/tmux/input.rs
git commit -m "refactor(input): split tmux appliers to read per-responsibility messages"
```

---

### Task 4: Split the Default appliers to read the new messages

Mirror Task 3 for Default mode: `apply_default_shortcuts` (reads `ShortcutMessage`), `apply_default_copy_mode` (reads `CopyModeMessage`), `apply_default_type` (reads `TypeMessage`). `WebviewForwardMessage` has no Default consumer (it was a no-op). Producer still dual-writes `ShortcutBatch`; after this task nothing reads `ShortcutBatch` anymore, but it is still written (removed in Task 5), so the tree stays green.

**Files:**
- Modify: `src/input/shortcuts/default_mode.rs` (rewrite `ShortcutsDefaultModePlugin::build`; rewrite `apply_default_shortcuts`; add `apply_default_copy_mode`, `apply_default_type`; update imports)
- Modify: `src/input/default_mode.rs` (test harness `dispatch` at :408-416 and the app builder / message registration at :371-375)
- Test: existing tests in `src/input/default_mode.rs`

**Interfaces:**
- Consumes: `ShortcutMessage`, `CopyModeMessage`, `TypeMessage` (from Task 2); `bevy_key_to_terminal_key` (`src/input/keyboard.rs:39`, already imported).
- Produces: no new cross-task interface.

- [ ] **Step 1: Rewrite the Default plugin registration**

In `src/input/shortcuts/default_mode.rs`, replace `ShortcutsDefaultModePlugin::build`:

```rust
impl Plugin for ShortcutsDefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                apply_default_shortcuts
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<ShortcutMessage>),
                apply_default_copy_mode
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<CopyModeMessage>),
                apply_default_type
                    .in_set(ShortcutSet::Apply)
                    .run_if(in_state(AppMode::Default))
                    .run_if(on_message::<TypeMessage>)
                    .after(apply_default_shortcuts)
                    .after(apply_default_copy_mode),
            ),
        );
    }
}
```

- [ ] **Step 2: Rewrite `apply_default_shortcuts` to read `ShortcutMessage`**

Replace the whole fn. Default handles only copy-mode entry and paste; every other action is a no-op in Default:

```rust
/// Applies `AppMode::Default` keyboard shortcuts from `ShortcutMessage`:
/// copy-mode entry and paste (direct paste fires outside copy mode; a leader
/// paste fires unconditionally). `Quit` / `ReleaseWebviewFocus` are handled
/// upstream in `resolve_key_effects`; pane/window actions are no-ops in Default.
/// Registered in `ShortcutSet::Apply`, gated on `in_state(AppMode::Default)` +
/// `on_message::<ShortcutMessage>`.
pub(in crate::input) fn apply_default_shortcuts(
    mut commands: Commands,
    mut shortcuts: MessageReader<ShortcutMessage>,
) {
    for msg in shortcuts.read() {
        match msg.action {
            Shortcut::EnterCopyMode => {
                if let Some(entity) = msg.focused {
                    commands.trigger(EnterCopyModeActionEvent { entity });
                }
            }
            Shortcut::Paste => {
                if let Some(entity) = msg.focused
                    && (msg.via_leader || !msg.in_copy_mode)
                {
                    commands.trigger(PasteAction { entity });
                }
            }
            Shortcut::DetachSession
            | Shortcut::SelectPane(_)
            | Shortcut::SplitPane(_)
            | Shortcut::KillPane
            | Shortcut::ZoomPane
            | Shortcut::NewWindow
            | Shortcut::KillWindow
            | Shortcut::NextWindow
            | Shortcut::PreviousWindow
            | Shortcut::SelectWindow(_)
            | Shortcut::RenameWindow
            | Shortcut::RenameSession
            | Shortcut::Quit
            | Shortcut::ReleaseWebviewFocus => {}
        }
    }
}
```

- [ ] **Step 3: Add `apply_default_copy_mode` and `apply_default_type`**

Add below `apply_default_shortcuts`:

```rust
/// Applies matched `[copy-mode]` keys from `CopyModeMessage` on the focused
/// terminal. Registered in `ShortcutSet::Apply`, gated on
/// `in_state(AppMode::Default)` + `on_message::<CopyModeMessage>`.
fn apply_default_copy_mode(
    mut commands: Commands,
    mut copy_mode: MessageReader<CopyModeMessage>,
) {
    for msg in copy_mode.read() {
        if let Some(entity) = msg.focused {
            trigger_copy_mode_action(&mut commands, entity, msg.action);
        }
    }
}

/// Types raw keys from `TypeMessage` into the focused terminal as
/// `TerminalKeyInput`. Runs after the shortcut/copy appliers. Registered in
/// `ShortcutSet::Apply`, gated on `in_state(AppMode::Default)` +
/// `on_message::<TypeMessage>`.
fn apply_default_type(
    mut commands: Commands,
    mut type_keys: MessageReader<TypeMessage>,
) {
    for msg in type_keys.read() {
        if let Some(entity) = msg.focused
            && let Some(key) = bevy_key_to_terminal_key(&msg.logical)
        {
            let terminal_mods = TerminalModifiers {
                ctrl: msg.mods.ctrl,
                shift: msg.mods.shift,
                alt: msg.mods.alt,
                meta: msg.mods.meta,
            };
            commands.trigger(TerminalKeyInput {
                entity,
                key,
                modifiers: terminal_mods,
            });
        }
    }
}
```

- [ ] **Step 4: Update the `default_mode.rs` imports**

In `src/input/shortcuts/default_mode.rs`:
- Drop `keyboard::{... key_effect::KeyEffect}` and `shortcuts::{ShortcutBatch, ShortcutSet}`. Keep `bevy_key_to_terminal_key`. Add `shortcuts::{CopyModeMessage, ShortcutMessage, ShortcutSet, TypeMessage}`.
- `trigger_copy_mode_action`, `PasteAction`, `EnterCopyModeActionEvent`, `TerminalKeyInput`, `TerminalModifiers`, `Shortcut` are already imported (:1-12).

Confirm no `KeyEffect` / `ShortcutBatch` reference remains.

- [ ] **Step 5: Update the Default test harness**

In `src/input/default_mode.rs` test module, replace the `ShortcutBatch` registration (:375) and `dispatch` helper (:408-416). Register the three messages and run the three appliers with the same ordering. Rewrite `dispatch` to translate `Vec<KeyEffect>` into per-message writes (identical structure to Task 3 Step 5, minus `WebviewForwardMessage`, which Default ignores — still translate it to a `TypeMessage`? No: Default drops webview-forward keys, so **skip** `KeyEffect::WebviewForward` in the Default harness to preserve the old no-op behavior):

```rust
    fn dispatch(app: &mut App, effects: Vec<KeyEffect>, focused: Option<Entity>) {
        let mods = Modifiers::default();
        for effect in effects {
            match effect {
                KeyEffect::Shortcut { action, via_leader } => {
                    app.world_mut().write_message(ShortcutMessage {
                        action,
                        via_leader,
                        focused,
                        in_copy_mode: false,
                    });
                }
                KeyEffect::CopyMode(action) => {
                    app.world_mut().write_message(CopyModeMessage { action, focused });
                }
                KeyEffect::Type { logical, key_code } => {
                    app.world_mut().write_message(TypeMessage {
                        logical,
                        key_code,
                        focused,
                        mods,
                    });
                }
                KeyEffect::WebviewForward { .. } => {}
            }
        }
        app.update();
    }
```

Preserve any test that exercised copy-mode paste suppression: those tests must set `ShortcutMessage.in_copy_mode = true` (search the test bodies for the paste/copy-mode cases and thread the flag through — if the existing harness had a variant of `dispatch` that set `in_copy_mode`, replicate it by adding an `in_copy_mode: bool` parameter to `dispatch` and passing it into each `ShortcutMessage`).

- [ ] **Step 6: Verify compile and green**

Run: `cargo build -p ozmux`
Expected: clean.

Run: `cargo test -p ozmux input::`
Expected: PASS — Default appliers behave identically (paste suppression, copy-mode entry, typing).

- [ ] **Step 7: Commit**

```bash
git add src/input/shortcuts/default_mode.rs src/input/default_mode.rs
git commit -m "refactor(input): split Default appliers to read per-responsibility messages"
```

---

### Task 5: Remove `ShortcutBatch` and clean up the producer

Nothing reads `ShortcutBatch` anymore. Delete the type, its registration, and the dual-write; simplify `resolve_key_effects` to fan out directly from the classified effects (no intermediate `effects` Vec used for a batch). Update the `handler.rs` tests to capture the new messages instead of `ShortcutBatch`.

**Files:**
- Modify: `src/input/shortcuts.rs` (delete `ShortcutBatch` struct at :70-84 and its `.add_message::<ShortcutBatch>()` at :41; update `ShortcutSet` doc comments that mention "the `ShortcutBatch`")
- Modify: `src/input/keyboard/handler.rs` (drop `mut batch` param and the `ShortcutBatch` write + import; keep the fan-out loop, now the sole output path; update the module `//!` and `resolve_key_effects` doc)
- Modify: `src/input/keyboard/handler.rs` tests (`capture_batch` → capture the four messages)
- Modify: any lingering doc comment that references `ShortcutBatch` (`src/input/default_mode.rs:3-9` module doc, `src/ui/tmux/pane_focus.rs:33`)
- Test: `src/input/keyboard/handler.rs` tests

**Interfaces:**
- Consumes: the four messages (final consumers exist from Tasks 3–4).
- Produces: `resolve_key_effects` writing only the four messages (no `ShortcutBatch`).

- [ ] **Step 1: Simplify `resolve_key_effects` to fan out directly**

In `src/input/keyboard/handler.rs`, remove the `mut batch: MessageWriter<ShortcutBatch>` param and the `use ... ShortcutBatch`. Replace the effects loop + dual-write + `batch.write(...)` block with a single fan-out that handles `Quit` / `ReleaseWebviewFocus` inline and writes a message for every other effect:

```rust
    for effect in all {
        match effect {
            KeyEffect::Shortcut {
                action: Shortcut::Quit,
                ..
            } => {
                exit.write(AppExit::Success);
            }
            KeyEffect::Shortcut {
                action: Shortcut::ReleaseWebviewFocus,
                ..
            } => focused_webview.0 = None,
            KeyEffect::Shortcut { action, via_leader } => {
                messages.shortcut.write(ShortcutMessage {
                    action,
                    via_leader,
                    focused,
                    in_copy_mode,
                });
            }
            KeyEffect::CopyMode(action) => {
                messages.copy_mode.write(CopyModeMessage { action, focused });
            }
            KeyEffect::Type { logical, key_code } => {
                messages.type_keys.write(TypeMessage {
                    logical,
                    key_code,
                    focused,
                    mods,
                });
            }
            KeyEffect::WebviewForward { logical, key_code } => {
                messages.webview_forward.write(WebviewForwardMessage {
                    logical,
                    key_code,
                    focused,
                    mods,
                });
            }
        }
    }
```

(Note: `all` is now consumed by value, so `logical` moves — no `.clone()` needed, unlike the Task 2 dual-write.)

- [ ] **Step 2: Delete `ShortcutBatch` and its registration**

In `src/input/shortcuts.rs`:
- Delete the `ShortcutBatch` struct (`:70-84`, the `#[derive(Message)] pub(in crate::input) struct ShortcutBatch { ... }` and its doc comment).
- Delete the `.add_message::<ShortcutBatch>()` line from `ShortcutsPlugin::build`.
- Update the `ShortcutSet` doc comment (`:96-104`) to describe the fan-out instead of "writes the `ShortcutBatch`" — e.g. `Resolve` classifies keys and **fans out the per-responsibility messages**; `Apply` reads those messages.
- If `KeyEffect` is no longer referenced in `shortcuts.rs` after deleting `ShortcutBatch`, remove its `use`.

- [ ] **Step 3: Update the `handler.rs` tests to capture the new messages**

In `src/input/keyboard/handler.rs` tests, replace the `capture_batch` capture system and its `Captured` resource so it counts / records the new messages. Where a test asserted "exactly one `ShortcutBatch` per keyboard frame", assert on the union of the four message queues for that frame; where it asserted a guarded frame emits no batch, assert all four queues are empty. Register the four messages in the test app instead of `ShortcutBatch`. Keep each test's behavioral intent (Quit inline, release inline, copy-mode, typing, CEF filter).

Concretely, replace the capture resource + system with per-message readers, e.g.:

```rust
    #[derive(Resource, Default)]
    struct Captured {
        shortcuts: Vec<(Shortcut, bool)>,
        copy_mode: usize,
        typed: usize,
        webview_forward: usize,
    }

    fn capture_messages(
        mut cap: ResMut<Captured>,
        mut shortcuts: MessageReader<ShortcutMessage>,
        mut copy_mode: MessageReader<CopyModeMessage>,
        mut typed: MessageReader<TypeMessage>,
        mut webview_forward: MessageReader<WebviewForwardMessage>,
    ) {
        for m in shortcuts.read() {
            cap.shortcuts.push((m.action, m.via_leader));
        }
        cap.copy_mode += copy_mode.read().count();
        cap.typed += typed.read().count();
        cap.webview_forward += webview_forward.read().count();
    }
```

Adjust each assertion site to the fields above. For the "one message per frame" invariant, assert `cap.shortcuts.len() + cap.copy_mode + cap.typed + cap.webview_forward` equals the expected effect count for that frame.

- [ ] **Step 4: Purge lingering `ShortcutBatch` doc references**

Update comments that still name `ShortcutBatch` (they are stale after removal):
- `src/input/default_mode.rs:3-9` module `//!` — replace "applies the frame's `ShortcutBatch`" with "applies the frame's shortcut messages".
- `src/input/keyboard/handler.rs` module `//!` (:1-7) and `resolve_key_effects` doc — replace "emits ... as a single `ShortcutBatch`" with the fan-out description.
- `src/input/shortcuts/tmux.rs` / `default_mode.rs` doc comments if any still say `ShortcutBatch` (Tasks 3–4 should have handled these; double-check).
- `src/ui/tmux/pane_focus.rs:33` — the `// NOTE:` mentions `batch.focused`; reword to "so the resolved messages' `focused` reflects the current active pane" (keep it a `NOTE` — it documents a load-bearing ordering edge).

Run a final search: `grep -rn "ShortcutBatch" src/` must return nothing.

- [ ] **Step 5: Verify compile and full green**

Run: `cargo build -p ozmux`
Expected: clean, no warnings.

Run: `cargo test -p ozmux input::`
Expected: PASS — full input suite green with `ShortcutBatch` gone.

- [ ] **Step 6: Run clippy + fmt**

Run: `cargo clippy -p ozmux --all-targets && cargo fmt`
Expected: no clippy warnings; fmt leaves the tree clean (or restages formatting).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(input): remove ShortcutBatch; resolve fans out typed messages directly"
```

---

## Self-Review Notes

- **Spec coverage:** §1 (KeyEffect pure IR, `Action`→`Shortcut`) → Task 1. §2 (four tailored messages, visibility, ordering-safety) → Task 2 (defs) + Tasks 3–4 (ordering constraint via `.after()`). §3 (producer fan-out, `SystemParam` bundle, `ShortcutBatch` deletion, stub replacement) → Tasks 2 + 5. §4 (six appliers, per-message `run_if`, OR-gate, ordering) → Tasks 3–4. Test-updates section → Steps in Tasks 1/3/4/5. Considered-alternatives (3-message fold, `Message` vs `EntityEvent`) are design context, no task.
- **Ordering constraint (the reviewed correctness fix):** encoded as `apply_*_forward.after(apply_*_shortcuts).after(apply_*_copy_mode)` in Tasks 3 (tmux) and 4 (Default).
- **Green at every task:** Task 1 rename compiles; Task 2 dual-writes (consumers unchanged); Task 3 migrates tmux while Default + producer keep `ShortcutBatch`; Task 4 migrates Default (producer still dual-writes); Task 5 removes `ShortcutBatch` once unread.
- **Type consistency:** `ShortcutMessages` fields (`shortcut`, `copy_mode`, `type_keys`, `webview_forward`) are used identically in `handler.rs` (Tasks 2, 5). Message field names (`action`, `via_leader`, `focused`, `in_copy_mode`, `logical`, `key_code`, `mods`) are consistent across producer and all consumers.
