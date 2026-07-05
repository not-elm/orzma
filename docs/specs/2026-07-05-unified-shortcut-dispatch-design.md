# Unified shortcut dispatch: one pure decider + mode-gated appliers

Date: 2026-07-05
Status: design approved, pending spec review

## Goal

Collapse the two near-duplicate keyboard-shortcut dispatchers
(`app_shortcut_handler` for `AppMode::Default`, `forward_keys_to_tmux` for
`AppMode::Tmux`) into a single shared decision path so the leader/repeat
state machine and per-key routing live in ONE place and can never drift
between modes. The shared decision becomes a pure function
`classify_key_batch(...) -> Vec<KeyEffect>`; each mode keeps a thin,
`run_if(in_state)`-gated applier that translates those effects into the
existing per-action `EntityEvent`s. Behaviour is preserved exactly; this
is a pure structural refactor.

## Background

### The two dispatchers today

- **Default** — `app_shortcut_handler` (`src/input/default_mode.rs:163`,
  ~124-line body, `run_if(in_state(AppMode::Default))` +
  `run_if(on_message::<KeyboardInput>)`, in `InputPhase::FocusedKey` +
  `LeaderGate::Advance`). Applies the GUI shortcuts the terminal crate
  does not own (Quit, copy-mode entry, leader sequences, leader-scoped
  paste) plus the shared `[copy-mode]` key table; **all tmux pane/window
  actions are explicit no-ops** (`default_mode.rs:274`).
- **Tmux** — `forward_keys_to_tmux` (`src/input/tmux/input.rs:87`,
  ~345-line body, `run_if(in_state(AppMode::Tmux))` +
  `run_if(on_message::<KeyboardInput>)`, same sets). Does three jobs in
  one body: modal-prompt guards, leader/GUI action dispatch (full tmux
  pane/window set), and plain-key forwarding to the active pane
  (`SendPaneKeys` batch).

Both call the shared pure leader machine `step_leader`
(`src/input/shortcuts.rs:269`) with the shared `LeaderPhase` resource, but
each re-implements the surrounding per-key loop: the `ev.repeat`
auto-repeat wrapper, the `RunAction / Swallow / Passthrough →
match_gui_action` resolution, the copy-mode fallthrough
(`ResolvedCopyModeKeys::resolve` → `trigger_copy_mode_action`), and the
`match Shortcut`. That duplicated scaffolding is the drift risk this
refactor removes. `forward_keys_to_tmux`'s 345-line body also violates the
`.claude/rules/rust.md` ~150-line/system cap.

### Typing today is split, asymmetrically

A third system, `dispatch_input` (`src/input/keyboard.rs:46`,
`LeaderGate::Read`, no `in_state` gate), types plain keys into the single
`KeyboardFocused` `OzmaTerminal` (via `TerminalKeyInput`) and **withholds**
leader-consumed keys from the PTY. In Default mode it owns typing; in Tmux
mode it finds no `KeyboardFocused` terminal, clears its own reader cursor,
and returns — so `forward_keys_to_tmux` owns typing there (to the pane).
The `dispatch_input` ↔ `app_shortcut_handler` split forces the two systems
to coordinate `LeaderPhase` through same-frame snapshots — the source of
the long `// NOTE:` cluster in `keyboard.rs` about opening/closing the
repeat window "with" the leader machine. Folding typing into the same
single pass as action resolution removes that whole class of same-frame
coordination.

### The chosen shape already exists in this repo

`src/input/tmux/mouse/` is the direct precedent and template: a
gather→decide→apply pipeline where `tmux_gesture` calls pure deciders
returning `Vec<TmuxMouseEffect>` (`src/input/tmux/mouse/effect.rs:21`),
triggers one `TmuxMouseEffects` `EntityEvent`
(`effect.rs:54`), and `on_tmux_mouse_effects` applies the batch. The
closely-related `2026-06-29-mouse-buttons-per-event-design.md` spec
establishes the exact variant we adopt: a host-private effect enum used as
the decision IR, with the gather system translating each effect into a
per-operation `EntityEvent` and triggering them in `Vec` order (Bevy 0.18
command-queue FIFO guarantees observation order). Two parallel
investigations (Codex + web/agent, 2026-07-05) confirmed this structure is
the better fit for both Bevy 0.17/0.18's Message-vs-observer split and this
repo's own `rust.md`, over a single fat multi-responsibility system.

### Existing per-action EntityEvents (reused as the apply vocabulary)

Already in the right shape (`#[event_target] entity` + `on_*` observer +
per-file plugin), all under `src/action/`:

- Pane ops → `ActivePane` entity: `SelectPaneRequest`, `SplitPaneRequest`,
  `KillPaneRequest`, `ZoomPaneRequest`.
- Window/session ops: `NewWindowRequest`/`NextWindowRequest`/
  `PreviousWindowRequest`/`RenameSessionRequest` → `TmuxSession` entity;
  `KillWindowRequest`/`RenameWindowRequest` → `ActiveWindow` entity;
  `SelectWindowRequest` → the indexed `TmuxWindow` entity.
- `PasteAction` (`src/action/terminal/paste.rs`) → focused terminal
  surface; `EnterCopyModeActionEvent` (`src/ui/copy_mode.rs`) → focused
  surface; copy-mode nav via `trigger_copy_mode_action`
  (`src/action/vi/keymap.rs`) → the shared, mode-agnostic
  `src/action/vi/applier.rs`; `TerminalKeyInput`
  (`crates/ozma_tty_engine`) → PTY-attached surface.

Only two actions are **not** yet events: `Quit` (writes `AppExit`) and
`DetachSession` (calls `request_detach(client)`, `src/session/tmux.rs:51`).

## Non-goals

- No behaviour change. Default keeps treating pane/window shortcuts as
  no-ops; the tmux path keeps its exact prompt guards, batch-forward, and
  paste/detach semantics.
- Not touching `forward_wheel_to_tmux` (mouse wheel), the mouse pipeline,
  the IME path (`apply_ime_commit_to_terminal` stays), `detect_modifier_tap`,
  or the internals of `step_leader` / `step_tap` / the vi applier.
- Not introducing a `Message`/buffered handoff. The research flagged that
  entity-targeted apply idiomatically uses `EntityEvent` + observer here;
  the only batch-shaped effect (tmux plain-key forwarding) is carried by a
  single batch `EntityEvent`, not a `Message`.

## Design

### 1. The pure decider (`src/input/resolve.rs`, new)

A new host-private module holds the decision IR and the pure function.
No systems, no plugin — pure library code, exhaustively unit-tested
(mirrors `step_leader`/`step_tap` and `ime::apply_event`).

```rust
/// One resolved outcome for a single pressed key. Host-private decision IR;
/// carries no ECS handles so the classifier is unit-testable and the same
/// batch drives either mode's applier.
enum KeyEffect {
    /// A GUI/app shortcut fired. `via_leader` records the paste origin: a
    /// direct-chord paste vs a leader-scoped one. It is NOT dropped (see the
    /// paste note below) — Default suppresses direct-chord paste in copy mode.
    Action { action: Shortcut, via_leader: bool },
    /// The focused surface is in copy mode and this key maps to a vi action.
    CopyMode(CopyModeAction),
    /// A plain key to type into the focused surface. Each applier encodes it
    /// for its own target (PTY VT bytes vs tmux key name) using the batch's
    /// modifier snapshot; `mods` is a per-frame constant held by the applier,
    /// so it is NOT carried per key.
    Type { logical: Key, key_code: KeyCode },
    /// A webview holds focus and declared this chord for forwarding to its
    /// pane. Only emitted when `ctx.forward_chords` is non-empty (tmux).
    WebviewForward { logical: Key, key_code: KeyCode },
    /// A webview holds focus and the configured release chord fired.
    ReleaseWebviewFocus,
}

/// Immutable per-frame context the applier resolves from its own mode's
/// queries and hands to the pure decider.
struct BatchContext {
    mods: Modifiers,
    now: Duration,
    in_copy_mode: bool,
    webview_focused: bool,
    forward_chords: /* borrowed slice of the focused webview's ForwardKeys */,
}

/// Threads `LeaderPhase` across the frame's pressed keys and classifies each
/// into a `KeyEffect`. Absorbs every per-key rule currently duplicated across
/// the two dispatchers AND `dispatch_input`: the `ev.repeat` auto-repeat
/// wrapper, `step_leader`, the `RunAction/Swallow/Passthrough→match_gui_action`
/// resolution, the copy-mode fallthrough, the `mods.meta` unmatched-key drop
/// (== "do not emit `Type`"), the withhold logic (a leader/repeat-consumed
/// key simply produces no `Type`), and the webview-focused branch.
fn classify_key_batch(
    leader_phase: &mut LeaderPhase,
    shortcuts: &Shortcuts,
    resolved_copy: &ResolvedCopyModeKeys,
    events: impl Iterator<Item = &KeyboardInput>,
    ctx: BatchContext,
) -> Vec<KeyEffect>
```

Key properties:

- **Single source of truth.** Both modes' per-key loops become this one
  function; nothing to drift.
- **Collapses Read+Advance.** Because one pass decides Type-vs-Action-vs-
  Swallow per key against one `LeaderPhase`, the `dispatch_input`
  snapshot-coordination (open/close the repeat window "with" the leader
  machine) disappears. `dispatch_input` is deleted.
- **`via_leader` is preserved (single-fire, not dropped).** One pass does
  remove the *double-fire* risk (each key yields exactly one effect, so a
  direct Cmd+V and a `<Leader>p` both resolve to one `Action(Paste)`), but
  `via_leader` served a second purpose the collapse must keep: today Default
  suppresses direct-chord paste while in copy mode (copy mode sets
  `KeyboardDisabled`, so `dispatch_input` never fires the direct Cmd+V; the
  leader path still pastes). Emitting an unconditional `Action(Paste)` for a
  direct Cmd+V would newly paste into a copy-mode terminal — a regression.
  So the Paste effect carries `via_leader`, and the Default applier pastes a
  direct-chord Cmd+V only when NOT in copy mode; leader paste always fires.
- **Typing stays target-specific at the edge.** `Type` carries the raw
  key (no modifiers — those are a batch constant the applier holds); the
  Default applier encodes via `bevy_key_to_terminal_key` + the frame's
  `TerminalModifiers`, the tmux applier via `bevy_key_to_tmux_name` + the
  frame's `KeyMods`. The decider does not unify the encoding (VT bytes vs
  tmux key names are genuinely different targets).

Behaviour-preserving decider invariants (fold these edge cases in exactly —
each is a ported test):

- **Release-webview-focus chord is swallowed even with no webview focus.**
  Today `Shortcuts::input_bindings()` reserves EVERY non-paste direct chord
  (including `ReleaseWebviewFocus`, e.g. the default Ctrl+Shift+Escape), so
  `dispatch_input` withholds it from the PTY unconditionally — but
  `match_gui_action`/`find_entry` deliberately EXCLUDE `ReleaseWebviewFocus`.
  A naive "emit `Type` whenever `match_gui_action` is `None`" would type
  Escape into the shell in Default. The decider MUST emit no `Type` for any
  key matching `is_release_webview_focus`, regardless of webview focus.
- **No `Type` while in copy mode.** Both current paths suppress typing in
  copy mode (Default via `KeyboardDisabled`, tmux via its `in_copy_mode`
  arm). When `ctx.in_copy_mode`, an unmatched key resolves to `CopyMode(..)`
  or is swallowed — never `Type`.
- **Webview branch is evaluated first and clears the leader.** When
  `ctx.webview_focused`, the decider tests the webview branch per key BEFORE
  stepping the leader, and CLEARS `LeaderPhase` to `Idle` (does not step it)
  — mirroring `default_mode.rs:207-210` and `input.rs:230`. Under webview
  focus it emits only `ReleaseWebviewFocus` / `WebviewForward`; GUI/leader
  handling is suppressed.

### 2. Two thin mode appliers (`run_if(in_state)`)

Each replaces one of today's dispatchers, stays under the 150-line cap,
and does gather → (pure decide) → trigger. Neither holds a `TmuxClient`,
`Clipboard`, or `TerminalHandle`: every apply is delegated to an observer.

**`apply_default_shortcuts`** (in `src/input/default_mode.rs`, replaces
`app_shortcut_handler`; `run_if(in_state(AppMode::Default))`):

- Guards: IME composing / window unfocused → `clear_leader_phase` + drain.
- Resolves `in_copy_mode` / `webview_focused` from its own queries
  (`KeyboardFocused OzmaTerminal`, `CopyModeState`, `FocusedWebview`).
- `classify_key_batch(...)`, then for each effect in `Vec` order:
  - `Action(Quit)` → `exit.write(AppExit::Success)`.
  - `Action(EnterCopyMode)` → `commands.trigger(EnterCopyModeActionEvent { entity })`
    **unconditionally** (matches today's guardless Default path; the tmux
    applier keeps its re-entry guard — see the EnterCopyMode note below).
  - `Action { action: Paste, via_leader }` → `commands.trigger(PasteAction { entity })`
    only when `via_leader || !in_copy_mode` (preserves today's suppression
    of direct-chord paste in copy mode).
  - `Action(pane/window/DetachSession/ReleaseWebviewFocus)` → **no-op**
    (Default owns no such targets — preserves today's behaviour).
  - `CopyMode(a)` → `trigger_copy_mode_action(&mut commands, entity, a)`.
  - `Type{..}` → `commands.trigger(TerminalKeyInput { entity, key, modifiers })`
    (per key; the engine observer applies).
  - `ReleaseWebviewFocus` → `focused_webview.0 = None`.
  - `WebviewForward{..}` → not emitted in Default (empty `forward_chords`).

**`apply_tmux_shortcuts`** (in `src/input/tmux/input.rs`, replaces the
keyboard half of `forward_keys_to_tmux`; `run_if(in_state(AppMode::Tmux))`;
`forward_wheel_to_tmux` is untouched):

- Guards: `CopyPrompt` / `ConfirmState` / `RenamePrompt` / IME / unfocused
  → `clear_leader_phase` + drain (unchanged set).
- Resolves `in_copy_mode` (active pane), `webview_focused` +
  `forward_chords` (from the focused webview's `ForwardKeys`), and the
  target entities via `ActionTargets` + `Option<Single<ActivePane>>`.
- `classify_key_batch(...)`, then for each effect in `Vec` order:
  - `Action(Quit)` → `AppExit`; `Action(EnterCopyMode)` →
    `EnterCopyModeActionEvent { active_pane }` (with the existing re-entry
    guard); `Action(Paste)` → `PasteAction { active_pane }`;
    `Action(DetachSession)` → `DetachSessionRequest { session }` (new);
    `Action(SelectPane/..)` etc. → the existing `*Request` on the resolved
    target entity (`ActivePane` / `TmuxSession` / `ActiveWindow` / indexed
    `TmuxWindow`, per the mapping in Background).
  - `CopyMode(a)` → `trigger_copy_mode_action`.
  - `Type{..}` → accumulate the tmux key name (`bevy_key_to_tmux_name`)
    into a frame-local `Vec<String>`.
  - `WebviewForward{..}` → accumulate its name into the same/adjacent
    forward batch.
  - After the loop: if any names accumulated, trigger ONE
    `ForwardPaneKeysRequest { active_pane, names }` (new).

Some effects (Quit, ReleaseWebviewFocus, CopyMode) are genuinely identical
in both appliers. A private helper
`apply_shared_effect(effect, entity, &mut commands, &mut exit,
&mut focused_webview) -> bool` MAY factor those arms out — but only extract
it once the duplication is real after implementation (YAGNI); do not
pre-build it. Two effects are NOT identical and must stay per-applier:

- **`EnterCopyMode` diverges by mode today.** The tmux applier guards
  re-entry (`copy_modes.get(entity).is_err()`, `input.rs:315-322`); Default
  triggers unconditionally; the `handle_enter_copy_mode_request` observer
  itself has no guard and re-clears the selection + re-enters vi mode on
  every trigger (`copy_mode.rs:51-70`). To stay behaviour-preserving, keep
  each mode's current behaviour (Default unconditional, tmux guarded) and do
  NOT route `EnterCopyMode` through `apply_shared_effect`. (Follow-up, out of
  scope: moving the re-entry guard into the observer would make it idempotent
  and unify both — but that changes Default's current re-clear behaviour, so
  it is a separate, non-behaviour-preserving change.)
- **`Paste` carries the copy-mode nuance** above (`via_leader || !in_copy_mode`
  in Default; the tmux applier always triggers `PasteAction`, applied by
  `on_paste_tmux`).

Only the mode-divergent arms (pane/window targeting; Paste/EnterCopyMode as
above; Type encoding; the tmux forward batch) live in each applier.

### 3. New apply-side EntityEvents

- **`DetachSessionRequest`** — new `src/action/tmux/detach_session.rs`
  (per-file `DetachSessionPlugin`, added to `TmuxActionPlugin`).
  `#[event_target] entity` (the `TmuxSession`); observer looks up the
  session and calls the existing `request_detach` logic (moved from the
  inline dispatcher call into the observer). Default has no session entity,
  so the applier never triggers it there.
- **`ForwardPaneKeysRequest`** — new event carrying
  `{ #[event_target] entity, names: Vec<String> }`, modelled on
  `TmuxMouseEffects`. Its observer sends exactly one
  `SendPaneKeys { pane, names }` on the `TmuxClient` and performs the
  single `snap_to_bottom_vt_only` / `flush_emit` (relocating
  `forward_keys_to_tmux:411-431`). This keeps the "accumulate → one
  `SendPaneKeys` per frame" contract that the module doc and the research
  both require, while removing `TmuxClient`/`TerminalHandle` from the
  applier's params. Placed in `src/input/tmux/forward.rs`, which already owns
  backend-bound key/byte/IME forwarding to panes — a closer home than the
  semantic `src/action/tmux/*Request` action modules. It cannot reuse the
  existing `TerminalForwardInput` (that sends `SendBytes` hex via
  `send-keys -H`, not `SendPaneKeys` names).
- **Tmux paste observer** — reuse the existing `PasteAction` event; add a
  `With<TmuxPane>` observer `on_paste_tmux` (in
  `src/action/terminal/paste.rs` or `src/action/tmux/`) that reads
  `Clipboard`, builds bytes via `build_paste_bytes`, and sends
  `SendBytes` in `PASTE_CHUNK_BYTES` chunks (relocating
  `forward_keys_to_tmux:281-308`), and add `Without<TmuxPane>` to the
  existing `on_paste` so it stays the PTY-only path. This mirrors the
  existing dual-observer split for `apply_ime_commit_to_terminal`
  (`Without<TmuxPane>`) vs the tmux forward observer.

### 4. Deletions and ordering

- **Delete** `dispatch_input` (`src/input/keyboard.rs`),
  `app_shortcut_handler`, and the keyboard body of `forward_keys_to_tmux`.
- `LeaderGate::Read` is removed (its only member was `dispatch_input`).
  `LeaderGate` becomes `{ Detect, Advance }`; `detect_modifier_tap`
  (`Detect`) still runs before the appliers (`Advance`), which stay in
  `InputPhase::FocusedKey`.
- `KeyboardInputPlugin` currently owns `add_message::<KeyboardInput>()` and
  the `TerminalInputBindings` init. **The message registration MUST remain** —
  `KeyboardInput` is read by many systems beyond the old dispatcher (the
  appliers, prompts, `detect_modifier_tap`); relocate `add_message` to the
  input root if the plugin is retired. Delete only the KEYBOARD binding path:
  `TerminalInputBindings` + `ReservedChord`, `populate_input_bindings`,
  `Shortcuts::input_bindings()`, and — now dead once `dispatch_input` is gone —
  `Shortcuts::opens_repeat_window` (its only caller was `keyboard.rs:101`).
  The unified decider resolves reserved chords to `Action`/`Swallow` and paste
  to `Action(Paste)` directly from `Shortcuts`. **Do NOT touch the rest of
  `src/input/bindings.rs`** — it also owns mouse policy (`OzmaMouseConfig`,
  `FineModifier`) that the mouse path still uses. (Confirm no other consumer of
  `TerminalInputBindings` before deleting; it is currently read only by
  `dispatch_input` and produced by `populate_input_bindings`.)
- **Verify before deleting `dispatch_input`:** it is mode-agnostic and types
  to the `KeyboardFocused` `OzmaTerminal`. tmux mirrors `ActivePane` onto
  `KeyboardFocused` (`src/ui/tmux/pane_focus.rs:90`), so a `KeyboardFocused`
  entity exists in tmux mode too. Confirm the active tmux pane is
  `KeyboardDisabled` (or otherwise not typed by `dispatch_input`) today, so
  that deleting it — with the Default applier gated `run_if(in_state(Default))`
  and thus inert in tmux — does not remove a live typing path. Expected: tmux
  typing goes only through `apply_tmux_shortcuts` → `ForwardPaneKeysRequest`,
  never `TerminalKeyInput`.
- Per the repo's plugin-registration rule, each surviving/new system is
  registered by the plugin in its own file; aggregators only `add_plugins`.

## Components and data flow

```
[gather+decide: apply_default_shortcuts | apply_tmux_shortcuts]   (run_if(in_state))
  guard (ime/focus [+ tmux prompts]) -> clear_leader_phase + drain on block
  resolve in_copy_mode / webview_focused / forward_chords / target entities
  classify_key_batch(&mut LeaderPhase, &Shortcuts, &ResolvedCopyModeKeys,
                     events, ctx)  -> Vec<KeyEffect>   (pure, host-private IR)
  for each KeyEffect in Vec order:            (Bevy 0.18 command queue is FIFO)
     Action(Quit)            -> exit.write(AppExit)
     Action(EnterCopyMode)   -> trigger EnterCopyModeActionEvent{entity}
     Action(Paste)           -> trigger PasteAction{entity}
     Action(DetachSession)   -> trigger DetachSessionRequest{session}      (tmux; default no-op)
     Action(SelectPane/…)    -> trigger <*Request>{resolved target}        (tmux; default no-op)
     CopyMode(a)             -> trigger_copy_mode_action(entity, a)
     ReleaseWebviewFocus     -> focused_webview.0 = None
     Type{..}                -> default: trigger TerminalKeyInput{entity}
                                tmux:    push bevy_key_to_tmux_name(..) into batch
     WebviewForward{..}      -> tmux: push into forward batch
  tmux only, after loop: if batch non-empty -> trigger ForwardPaneKeysRequest{active_pane, names}
        |
        v
[apply: existing + new observers]
  on_select_pane / on_kill_pane / … (unchanged)               -> tmux control commands
  on_enter_copy_mode / vi applier (unchanged, mode-agnostic)  -> copy mode / vi ops
  on_paste (Without<TmuxPane>)                                -> PTY paste
  on_paste_tmux (With<TmuxPane>, new)                         -> clipboard -> SendBytes chunks
  on_terminal_key_input (unchanged)                           -> PTY VT bytes
  on_detach_session (new)                                     -> request_detach(client)
  on_forward_pane_keys (new)                                  -> one SendPaneKeys + snap/flush
```

## Testing

- **`classify_key_batch` unit tests** (`src/input/resolve.rs`, no `App`):
  port the ~10 `dispatch_input` withhold tests (`keyboard.rs` tests:
  repeat-window withhold/close, pending suppression, same-frame duplicate,
  bare-modifier skip) and both dispatchers' repeat tests
  (`os_key_repeat_*`) as pure assertions over the returned `Vec<KeyEffect>`;
  add cases for copy-mode shadowing order, `mods.meta` drop, the webview
  branch (release chord + `ForwardKeys` + leader-clear), the
  release-webview-focus chord swallowed with no webview focus, no `Type`
  while `in_copy_mode`, and the `via_leader` paste distinction in copy mode.
  This is the primary safety net and the main win of the enum IR.
- **`step_leader` / `step_tap` tests** (`src/input/shortcuts.rs`):
  unchanged.
- **Applier integration tests** (Bevy `App` + capturing observers, per
  mode): assert which `EntityEvent`s fire for a given key batch; explicitly
  assert Default fires nothing for pane/window actions; assert tmux fires
  the correct `*Request` on the correct target and one
  `ForwardPaneKeysRequest` for a plain-key batch.
- **New observer tests**: `on_detach_session` (calls detach),
  `on_forward_pane_keys` (one `SendPaneKeys`, snap/flush once),
  `on_paste_tmux` (chunked `SendBytes`).
- Full gate: `cargo test`, `cargo clippy --workspace`, `cargo fmt`.

## Behaviour-preservation checklist

- Default pane/window shortcuts remain no-ops (applier skips those arms).
- Copy-mode keys stay shadowed by leader/GUI chords (decider resolves
  Action/GUI before the copy-mode fallthrough — `default_mode.rs:239`,
  `input.rs:398` parity).
- `mods.meta` / `mods.super_` unmatched keys are dropped, never typed
  (decider emits no `Type`).
- The release-webview-focus chord (default Ctrl+Shift+Escape) is swallowed
  even with NO webview focused — never typed (Default) / never plain-forwarded
  as an unmatched key; preserving today's reserved-chord withholding.
- No `Type` is emitted while `in_copy_mode` (parity with today's
  `KeyboardDisabled` (Default) / `in_copy_mode` arm (tmux) suppression).
- `EnterCopyMode` keeps each mode's current guard behaviour: Default triggers
  unconditionally, tmux guards re-entry — no unification in this refactor.
- Direct-chord paste stays suppressed in Default copy mode (`via_leader ||
  !in_copy_mode`); leader paste still fires; tmux paste unchanged.
- Repeat window and pending-suppression semantics are identical (same
  `step_leader` + the same `ev.repeat` wrapper, now in the decider).
- Webview-focused handling preserved: the decider evaluates the webview branch
  first per key and clears `LeaderPhase` to `Idle` (does not step it); release
  chord clears focus; GUI chords are suppressed under webview focus (Default);
  declared `ForwardKeys` chords forward to the pane (tmux).
- tmux emits exactly one `SendPaneKeys` per frame for plain keys, with the
  single `snap_to_bottom_vt_only`/`flush_emit`, via the batch event.
- Paste byte encoding is unchanged per mode (PTY paste vs chunked `SendBytes`).

## Risks and staging

The decider must reproduce the withhold/repeat edge cases exactly; those
are the fixed-bug tests, so porting them first (red → green against the new
pure function) de-risks the collapse. Suggested staging (detailed steps go
to the implementation plan):

1. Add `src/input/resolve.rs` (`KeyEffect`, `BatchContext`,
   `classify_key_batch`) + ported unit tests. No wiring yet.
2. Add the new events/observers (`DetachSessionRequest`,
   `ForwardPaneKeysRequest`, `on_paste_tmux`, `on_paste` filter).
3. Replace `app_shortcut_handler` with `apply_default_shortcuts`; delete
   `dispatch_input`; retire/trim `TerminalInputBindings`; verify Default
   mode (typing, leader, copy-mode, paste, quit).
4. Replace the keyboard body of `forward_keys_to_tmux` with
   `apply_tmux_shortcuts`; verify Tmux mode (pane/window ops, forwarding
   batch, paste, detach, webview forward). Keep `forward_wheel_to_tmux`.
5. Remove `LeaderGate::Read`; final `cargo test` / clippy / fmt gate.

## Alternatives considered

- **Single unified system, direct emit, no enum (`案1`).** One
  `dispatch_shortcuts` running both modes, all params tuple-bundled
  (~15), decision logic inline. Rejected: merges gather+decide+apply in
  one body (the `rust.md` anti-pattern it is meant to fix), concentrates
  both modes' params past the smell threshold, drops `run_if(in_state)`
  against the state idiom, and traps the gnarly repeat/withhold logic
  behind ECS params instead of a pure, cheaply-tested function. The 2026-
  07-05 parallel research judged this the worse fit for both Bevy and the
  repo rules.
- **Pure decider + common driver + `Message` + two appliers (`案2`
  original).** Same decider, but the driver emits a `KeyEffects`
  `Message` consumed by two `run_if(in_state)` appliers. Rejected in
  favour of triggering the existing per-action `EntityEvent`s directly:
  the repo's canonical apply seam for entity-targeted effects is
  `EntityEvent` + observer (the `dispatch_input → TerminalKeyInput`
  example), and a `Message` adds one indirection plus a same-frame
  ordering constraint for effects that are not one-to-many broadcasts.

## Out of scope

- `forward_wheel_to_tmux` and the mouse pipeline.
- IME (`apply_ime_commit_to_terminal`), which already routes plain vs
  tmux surfaces via `Without/With<TmuxPane>`.
- Internals of `step_leader`, `step_tap`, `detect_modifier_tap`, and the
  vi copy-mode applier.
- Any change to shortcut config, key→`KeyCode` resolution, or the
  `Shortcut` vocabulary.
