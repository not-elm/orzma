# Centralize Shortcut Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lift the duplicated read+decide scaffolding out of the two mode appliers into one mode-agnostic `resolve_shortcuts` System that emits a `ShortcutBatch` `Message`; the two appliers become thin `Message` consumers that trigger the mode-appropriate `EntityEvent`s.

**Architecture:** A new `src/input/dispatch.rs` owns `ShortcutBatch` (Message), the `ShortcutSet::{Resolve, Apply}` ordering set, and `resolve_shortcuts` — which guards, resolves the focused surface via the unified `KeyboardFocused` query, calls the UNCHANGED pure `classify_key_batch`, handles the mode-agnostic effects (Quit, ReleaseWebviewFocus) inline, and writes one `ShortcutBatch`. `apply_default_shortcuts` / `apply_tmux_shortcuts` shrink to `MessageReader<ShortcutBatch>` → trigger. Behaviour-preserving.

**Tech Stack:** Rust 2024, Bevy 0.18.1 ECS (`Message`/`MessageWriter`/`MessageReader`, `SystemSet` chaining, `run_if(in_state)` / `run_if(on_message)`, `EntityEvent` + observers), tmux control-mode client.

## Global Constraints

- Rust edition 2024. No `mod.rs`. Comments English-only; only `// TODO:` / `// NOTE:` / `// SAFETY:` line comments (NOTE = a wrong-if-overlooked caveat). No narrative comments, no commented-out code.
- File-level `//!` on the new module; `///` on every `pub`/`pub(crate)` item. All `use` at the top in one contiguous block; no inline fully-qualified paths; no glob imports.
- Visibility minimization is MANDATORY: `dispatch.rs` items start at the narrowest visibility that compiles — `resolve_shortcuts` private or `pub(super)`; `ShortcutBatch` / `ShortcutSet` are `pub(in crate::input)` (the two appliers are descendants of `crate::input`, so `pub(crate)` is wider than needed); `mod dispatch;` stays private.
- Mutable params first in signatures. `Query` params use descriptive nouns (no `_q`). Items ordered `pub` → `pub(crate)` → private within a block. System bodies ≤ ~150 lines.
- Systems/observers are registered by the `Plugin` in their own file; aggregators only `add_plugins`. Cross-file system ordering is expressed with a shared `SystemSet` (or `.before/.after` a set), NEVER `.after(some_fn)`.
- **`resolve_shortcuts` has ~17 candidate params — over Bevy's 16-`SystemParam` tuple-arity limit — so it MUST tuple-bundle related params (as `apply_tmux_shortcuts` already does) or use `#[derive(SystemParam)]` structs.**
- **Behaviour-preserving:** identical `classify_key_batch` output, identical triggered events, identical guards/suppressions. `classify_key_batch` / `KeyEffect` / `BatchContext` (`src/input/resolve.rs`) and all `src/action/*` observers are UNCHANGED.
- Full gate every commit: `cargo test -p orzma` (bin crate — NOT `--lib`), then `cargo clippy --workspace --all-targets -- -D warnings` (CLEAN), then `cargo fmt`.

---

## File Structure

- **Create** `src/input/dispatch.rs` — `ShortcutBatch` (Message), `ShortcutSet::{Resolve, Apply}` (SystemSet), `resolve_shortcuts` (System), `DispatchPlugin`; unit + ordering tests.
- **Modify** `src/input.rs` — declare `mod dispatch;`; add `DispatchPlugin` to `OrzmaInputPlugin`.
- **Modify** `src/input/default_mode.rs` — replace `apply_default_shortcuts`'s body/params with a thin `MessageReader<ShortcutBatch>` consumer; drop the old read+decide + `LeaderPhase`/`FocusedWebview`/prompt/focused-query params; keep `Res<Shortcuts>` (release-chord drop). Update `DefaultHostInputPlugin` registration.
- **Modify** `src/input/tmux/input.rs` — replace `apply_tmux_shortcuts`'s body/params likewise; keep `ActionTargets`; keep `dispatch_tmux_action`, `forward_wheel_to_tmux`, `TmuxWheelAccumulator`. Update `InputPlugin` registration.
- **Modify** `src/ui/tmux/pane_focus.rs` — order `sync_keyboard_focus_to_active_pane` `.before(InputPhase::FocusedKey)` so the deferred `KeyboardFocused` mirror flush lands before `resolve_shortcuts` reads it.
- **UNCHANGED** `src/input/resolve.rs` (the decider), `src/action/*` (observers), `src/input/shortcuts.rs` (`LeaderGate`/`step_leader`), `src/input/focus.rs`.

---

## Task 1: Atomic resolve/apply cutover

This is a SINGLE atomic change: `resolve_shortcuts` and the old appliers cannot coexist (both would step `LeaderPhase`, double-firing). So `dispatch.rs`, both applier rewrites, the registration moves, and the mirror ordering edge all land together, verified green as one commit.

**Files:**
- Create: `src/input/dispatch.rs`
- Modify: `src/input.rs`, `src/input/default_mode.rs`, `src/input/tmux/input.rs`, `src/ui/tmux/pane_focus.rs`

**Interfaces:**
- Consumes: `classify_key_batch(&mut LeaderPhase, &Shortcuts, &ResolvedCopyModeKeys, impl Iterator<Item=&KeyboardInput>, BatchContext) -> Vec<KeyEffect>`, `KeyEffect`, `BatchContext` (from `crate::input::resolve`, UNCHANGED); `clear_leader_phase`, `LeaderPhase`, `LeaderGate`, `Shortcuts` (`crate::input::shortcuts`); `current_modifiers` (`crate::input`); `KeyboardFocused` (`crate::input::focus`); `OrzmaTerminal`, `CopyModeState`, `FocusedWebview`, `ForwardKeys`, `ImeState`, `Modifiers`, `CopyPrompt`/`ConfirmState`/`RenamePrompt`; the apply-side events (`TerminalKeyInput`, `PasteAction`, `EnterCopyModeActionEvent`, `trigger_copy_mode_action`, `DetachSessionRequest`, `ForwardPaneKeysRequest`, the tmux `*Request`), `ActionTargets`, `dispatch_tmux_action`, `bevy_key_to_terminal_key`, `bevy_key_to_tmux_name`. (NOT `current_terminal_modifiers` — the Default applier builds `TerminalModifiers` inline from `batch.mods`, so that helper is unused by this design.)
- Produces:
  ```rust
  // `pub(in crate::input)` — the narrowest visibility that still lets the two
  // appliers (crate::input::default_mode, crate::input::tmux::input) read the
  // batch; both are descendants of crate::input, so pub(crate) is wider than needed.
  #[derive(Message)]
  pub(in crate::input) struct ShortcutBatch {
      pub(in crate::input) effects: Vec<KeyEffect>,   // excludes Quit / ReleaseWebviewFocus (handled in resolve_shortcuts)
      pub(in crate::input) focused: Option<Entity>,   // the KeyboardFocused OrzmaTerminal (default term / active pane)
      pub(in crate::input) in_copy_mode: bool,
      pub(in crate::input) mods: Modifiers,
  }
  #[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
  pub(in crate::input) enum ShortcutSet { Resolve, Apply }
  pub(super) struct DispatchPlugin;   // added by OrzmaInputPlugin in crate::input
  ```

- [ ] **Step 1: Write the `resolve_shortcuts` unit + ordering tests (RED)**

Create `src/input/dispatch.rs` with the `//!` doc, the `ShortcutBatch`/`ShortcutSet`/`DispatchPlugin`/`resolve_shortcuts` declarations (body `todo!()`), and `#[cfg(test)] mod tests`. Cases (Bevy `App` with `MinimalPlugins`, spawn a `(OrzmaTerminal, KeyboardFocused)` and drive one `Update`):

```rust
// resolve behaviour
guarded_frame_emits_no_batch            // ime composing OR window unfocused -> no ShortcutBatch, leader cleared
normal_batch_carries_effects_focused_in_copy_mode // a plain key -> one ShortcutBatch{ effects:[Type..], focused:Some(term), in_copy_mode:false, mods }
quit_writes_appexit_not_in_batch        // Cmd+Q -> AppExit written, ShortcutBatch has NO Action{Quit}
release_clears_webview_not_in_batch     // webview focused + release chord -> FocusedWebview.0=None, batch has NO ReleaseWebviewFocus
focused_resolves_for_tmux_pane          // a (OrzmaTerminal, TmuxPane, KeyboardFocused) entity resolves as batch.focused
in_copy_mode_flag_set                   // focused surface has CopyModeState -> batch.in_copy_mode==true
// ordering (load-bearing schedule edges)
batch_consumed_same_update              // register resolve_shortcuts + a capture applier under ShortcutSet chain; write KeyboardInput; ONE update; assert the applier saw the batch this frame (not next)
mirror_freshness_before_focusedkey      // register sync_keyboard_focus_to_active_pane(.before FocusedKey) + resolve_shortcuts; move ActivePane p1->p2 and press a key same tick; assert batch.focused==p2
```
Use the existing `KeyboardInput` builder pattern from `resolve.rs`/`keyboard.rs` tests and `test_shortcuts_*` constructors. For `batch_consumed_same_update`, add a tiny `#[cfg(test)]` capture system reading `MessageReader<ShortcutBatch>`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p orzma input::dispatch`
Expected: FAIL (`todo!()` / missing items).

- [ ] **Step 3: Implement `resolve_shortcuts` + `ShortcutBatch` + `DispatchPlugin`**

`resolve_shortcuts` (runs BOTH modes; `run_if(on_message::<KeyboardInput>)`; `in_set(InputPhase::FocusedKey)` + `in_set(ShortcutSet::Resolve)` + `in_set(LeaderGate::Advance)`). Bundle params to stay ≤16 (e.g. group the prompt `Option<Res<..>>` + `ImeState` into one tuple, the `Shortcuts`/`ResolvedCopyModeKeys`/`ButtonInput`/`Time` reads into another — mirror `apply_tmux_shortcuts:99-108`). Body (≤150 lines):

```rust
// guards — behaviour-identical to the old appliers, prompts Option-guarded
let focused_window = windows.single().map(|w| w.focused).unwrap_or(false);
if copy_prompt.open.is_some()               // CopyPrompt is global (Res<CopyPrompt>, always present)
    || confirm_state.is_some() || rename_prompt.is_some()   // ConfirmState/RenamePrompt: Option<Res<..>> (tmux-only)
    || ime.is_composing() || !focused_window
{
    clear_leader_phase(&mut leader_phase);
    events.clear();
    return;                              // no ShortcutBatch -> appliers don't run
}
let focused = focused_surface.single().ok();          // Query<Entity, (With<OrzmaTerminal>, With<KeyboardFocused>)>
let in_copy_mode = focused.is_some_and(|e| copy_modes.get(e).is_ok());
let webview_focused = focused_webview.0.is_some();
let forward_chords = focused_webview.0
    .and_then(|e| forward_keys.get(e).ok()).map(|c| c.0.as_slice()).unwrap_or(&[]);
let mods = current_modifiers(&bevy_keys);
let ctx = BatchContext { mods, now: time.elapsed(), in_copy_mode, webview_focused, forward_chords };
let all = classify_key_batch(&mut leader_phase, &shortcuts, &resolved_copy, events.read(), ctx);
let mut effects = Vec::with_capacity(all.len());
for effect in all {
    match effect {
        KeyEffect::Action { action: Shortcut::Quit, .. } => exit.write(AppExit::Success),
        KeyEffect::ReleaseWebviewFocus => focused_webview.0 = None,
        other => effects.push(other),
    }
}
batch.write(ShortcutBatch { effects, focused, in_copy_mode, mods });
```
`DispatchPlugin::build`: `app.add_message::<ShortcutBatch>().configure_sets(Update, (ShortcutSet::Resolve, ShortcutSet::Apply).chain().in_set(InputPhase::FocusedKey)).add_systems(Update, resolve_shortcuts.in_set(InputPhase::FocusedKey).in_set(ShortcutSet::Resolve).in_set(LeaderGate::Advance).run_if(on_message::<KeyboardInput>));`. Add `mod dispatch;` + `DispatchPlugin` to `OrzmaInputPlugin` in `src/input.rs`.

- [ ] **Step 4: Rewrite `apply_default_shortcuts` as a `ShortcutBatch` consumer**

Replace its body/params. Params: `Commands`, `MessageReader<ShortcutBatch>`, `Res<Shortcuts>` (for the release-chord drop). No `LeaderPhase`/`FocusedWebview`/prompt/`copy_modes`/focused-query/`bevy_keys`. Register `run_if(in_state(AppMode::Default))` + `run_if(on_message::<ShortcutBatch>)` + `in_set(ShortcutSet::Apply)` (drop `LeaderGate::Advance` and `on_message::<KeyboardInput>`). Body:

```rust
for batch in batches.read() {
    let terminal_mods = TerminalModifiers { ctrl: batch.mods.ctrl, shift: batch.mods.shift, alt: batch.mods.alt, meta: batch.mods.meta };
    for effect in &batch.effects {
        match effect {
            KeyEffect::Action { action: Shortcut::EnterCopyMode, .. } =>
                if let Some(e) = batch.focused { commands.trigger(EnterCopyModeActionEvent { entity: e }); },
            KeyEffect::Action { action: Shortcut::Paste, via_leader } =>
                if let Some(e) = batch.focused && (*via_leader || !batch.in_copy_mode) { commands.trigger(PasteAction { entity: e }); },
            KeyEffect::Action { action: Shortcut::SelectPane(_) | /* …all pane/window/detach… */ Shortcut::RenameSession, .. } => {}
            KeyEffect::CopyMode(a) => if let Some(e) = batch.focused { trigger_copy_mode_action(&mut commands, e, *a); },
            KeyEffect::Type { logical, key_code } =>
                if let Some(e) = batch.focused && !shortcuts.is_release_webview_focus(*key_code, batch.mods)
                    && let Some(key) = bevy_key_to_terminal_key(logical) {
                    commands.trigger(TerminalKeyInput { entity: e, key, modifiers: terminal_mods });
                },
            KeyEffect::WebviewForward { .. } => {}   // no-op — see spec (Default has no pane; drop)
            // Quit / ReleaseWebviewFocus never reach here (handled in resolve_shortcuts),
            // but the match must stay exhaustive over `KeyEffect` + `Shortcut`:
            KeyEffect::ReleaseWebviewFocus => {}
            KeyEffect::Action { action: Shortcut::Quit | Shortcut::ReleaseWebviewFocus, .. } => {}
        }
    }
}
```
(Keep the exhaustive `Action` no-op list matching today's applier. Rework the file's applier tests to WRITE a `ShortcutBatch` and assert the triggered events; keep `direct_paste_in_copy_mode_suppressed` via `batch.in_copy_mode`, `pane_action_is_noop`, `release_webview_chord_is_not_typed`.)

- [ ] **Step 5: Rewrite `apply_tmux_shortcuts` as a `ShortcutBatch` consumer**

Params: `Commands`, `MessageReader<ShortcutBatch>`, `ActionTargets`. Register `run_if(in_state(AppMode::Tmux))` + `run_if(on_message::<ShortcutBatch>)` + `in_set(ShortcutSet::Apply)` + `in_set(TmuxActiveSet)`. `batch.focused` IS the active pane. Body (port the current arms; drop the read+decide/guards/leader/focused-webview). Iterating `&batch.effects` yields references, so deref the copied fields. A `let kmods = KeyMods { ctrl: batch.mods.ctrl, shift: batch.mods.shift, alt: batch.mods.alt, super_: batch.mods.meta };` up front; a `names` Vec accumulates `Type`/`WebviewForward` via `bevy_key_to_tmux_name(logical, *key_code, kmods)`; `EnterCopyMode`→`if let Some(e) = batch.focused && !batch.in_copy_mode { commands.trigger(EnterCopyModeActionEvent { entity: e }); }`; `Paste`→`PasteAction{ entity: e }` if `Some`; `DetachSession`→`if let Ok(e) = targets.session.single() { commands.trigger(DetachSessionRequest { entity: e }); }`; pane/window→`dispatch_tmux_action(&mut commands, *action, batch.focused, &targets)`; `CopyMode(a)`→`if let Some(e) = batch.focused { trigger_copy_mode_action(&mut commands, e, *a); }` (signature `(&mut Commands, Entity, CopyModeAction)`); after the loop `if let Some(e) = batch.focused && !names.is_empty() { commands.trigger(ForwardPaneKeysRequest { entity: e, names }); }`. `Quit` and both `ReleaseWebviewFocus` arms (the `Action` one and the top-level `KeyEffect::ReleaseWebviewFocus`) are no-ops (handled upstream) but must be present for exhaustiveness. Rework the tmux applier tests to WRITE a `ShortcutBatch`; keep the target-correctness + one-`ForwardPaneKeysRequest` cases.

- [ ] **Step 6: Add the `KeyboardFocused` mirror ordering edge**

In `src/ui/tmux/pane_focus.rs`, order `sync_keyboard_focus_to_active_pane` `.after(TmuxProjectionSet).before(InputPhase::FocusedKey)` (import `crate::input::InputPhase`; `TmuxProjectionSet` is already used by the sibling `augment_tmux_pane` in the same tuple). BOTH edges are load-bearing: `ActivePane` is set by the tmux-projection observer (not an optimistic same-frame insert), so `.after(TmuxProjectionSet)` makes the mirror run after `ActivePane` is fresh, and `.before(InputPhase::FocusedKey)` makes its deferred `commands` flush land before `resolve_shortcuts` reads `KeyboardFocused`. Together they close projection → mirror → resolve so `batch.focused`/`in_copy_mode` reflect the current active pane the same frame it changes (matching what the old `apply_tmux_shortcuts` saw reading `ActivePane` directly). Verify no schedule cycle (the mirror has no back-edge from `FocusedKey`).

- [ ] **Step 7: Run the full suite + ordering tests (GREEN)**

Run: `cargo test -p orzma` (whole suite, including the `input::dispatch` ordering tests and the reworked applier tests).
Expected: PASS. Then `cargo clippy --workspace --all-targets -- -D warnings` (CLEAN) and `cargo fmt`. Do NOT run the GUI (`cargo run` is interactive) — the reworked capturing tests + the two ordering regression tests are the behaviour verification.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(input): centralize shortcut resolution into resolve_shortcuts + ShortcutBatch"
```

---

## Task 2: Final gate + dead-code / visibility sweep

**Files:** whole workspace (verification; small demotions only if the sweep finds them)

- [ ] **Step 1: Dead-code + visibility sweep**

Confirm the old per-applier read+decide left nothing dangling and the new items are minimally visible:
```bash
rg -n 'MessageReader<KeyboardInput>|classify_key_batch|ShortcutBatch|ShortcutSet|resolve_shortcuts' src
```
Expected: `MessageReader<KeyboardInput>` survives only in `resolve_shortcuts` (+ `detect_modifier_tap`/IME as before), NOT in the appliers; `classify_key_batch` is called only from `resolve_shortcuts`. Demote any `dispatch.rs` item used only within its module to private; confirm `ShortcutBatch`/`ShortcutSet` are `pub(in crate::input)` (not `pub(crate)`), `resolve_shortcuts` is `pub(super)`/private, and `mod dispatch;` is private.

- [ ] **Step 2: Full gate**

Run: `cargo test -p orzma`, then `cargo clippy --workspace --all-targets -- -D warnings`, then `cargo fmt --check`.
Expected: all green, zero warnings.

- [ ] **Step 3: Commit (if the sweep changed anything; else skip)**

```bash
git add -A
git commit -m "chore(input): visibility sweep for centralized shortcut resolution"
```

---

## Self-Review

**Spec coverage:** `ShortcutBatch` + `resolve_shortcuts` + `DispatchPlugin` → Task 1 Steps 1-3; thin appliers → Steps 4-5; mirror ordering edge (SE1) → Step 6; `Res<Shortcuts>` on the Default applier (SE2) → Step 4; param bundling (SE3) → Step 3; `WebviewForward` no-op parity (SE4) → Step 4; `CopyPrompt` global/inert guard (SE5) → Step 3; the two ordering regression tests (SE6) → Step 1; visibility minimization → Task 2. All spec sections mapped.

**Placeholder scan:** No "TBD"/"add error handling"/"similar to Task N". The atomic cutover shows the full `resolve_shortcuts` body and both applier match shapes; the exhaustive `Action` no-op lists are noted to match today's appliers (the implementer copies the current variant list — it is verified live code, not invented).

**Type consistency:** `ShortcutBatch { effects: Vec<KeyEffect>, focused: Option<Entity>, in_copy_mode: bool, mods: Modifiers }` is identical in the Produces block and every consumer step. `ShortcutSet::{Resolve, Apply}`, `resolve_shortcuts`, and the effect variant names (`KeyEffect::{Action, CopyMode, Type, WebviewForward, ReleaseWebviewFocus}`) match across tasks. `batch.focused` is `Option<Entity>` everywhere; `batch.mods` is `Modifiers` (derived to `TerminalModifiers`/`KeyMods` in the appliers).
