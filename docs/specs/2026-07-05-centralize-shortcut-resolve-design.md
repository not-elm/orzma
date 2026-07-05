# Centralize shortcut resolution: one resolve System + ShortcutBatch Message + mode appliers

Date: 2026-07-05
Status: design approved, pending spec review

## Goal

Lift the duplicated "read `KeyboardInput` → resolve context → `classify_key_batch`"
scaffolding out of the two mode appliers (`apply_default_shortcuts`,
`apply_tmux_shortcuts`) into a single mode-agnostic `resolve_shortcuts` System.
That System emits the decided `Vec<KeyEffect>` as a buffered `ShortcutBatch`
`Message`; each mode keeps a thin, `run_if(in_state)`-gated applier that reads
the batch and triggers the mode-appropriate `EntityEvent`s. Behaviour is
preserved exactly — this is a structural rewire of the just-shipped unified
dispatch (`docs/specs/2026-07-05-unified-shortcut-dispatch-design.md`), not a
behaviour change.

## Background

The current design (shipped in PR #239) has a single pure decider,
`classify_key_batch` (`src/input/resolve.rs`), called from two mode-gated
appliers. Each applier repeats the same front half:

- `apply_default_shortcuts` (`src/input/default_mode.rs`) — guards (IME / focus),
  resolves the focused `OzmaTerminal`, `in_copy_mode`, `mods`, builds
  `BatchContext`, calls `classify_key_batch`, then `match`es effects.
- `apply_tmux_shortcuts` (`src/input/tmux/input.rs`) — guards (5: `CopyPrompt` /
  `ConfirmState` / `RenamePrompt` / IME / focus), resolves `ActivePane`,
  `in_copy_mode`, `forward_chords`, `mods`, builds `BatchContext`, calls
  `classify_key_batch`, then `match`es effects.

The heavy decision logic is already shared (`classify_key_batch`), but the
gather/guard/context scaffolding and the `classify_key_batch` call are
duplicated, and the two systems each own `ResMut<LeaderPhase>` /
`ResMut<FocusedWebview>` / the prompt guards. Motivation: **DRY the read+decide
scaffolding** and **separate concerns** — "read input + decide the cut" as one
System, "apply the cut per mode" as another, decoupled by a `Message`.

### The enabling invariant

Both modes already maintain exactly one **`KeyboardFocused` `OzmaTerminal`** on
the active surface:

- Default: the focused terminal carries `KeyboardFocused`
  (`src/input/focus.rs`, `src/input/default_mode.rs:182`).
- Tmux: `sync_pane_keyboard_focus` (`src/ui/tmux/pane_focus.rs:90-103`) mirrors
  `ActivePane` onto `KeyboardFocused` — the active pane (a `TmuxPane` that is
  also an `OzmaTerminal`) gains it, every other pane loses it. The gateway
  terminal has `KeyboardFocused` removed on adopt (`src/session/tmux/adopt.rs:163`).

So a single `Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>`
resolves THE focused surface in both modes (the default terminal, or the active
tmux pane). This is what lets one mode-agnostic System compute `in_copy_mode`
and the effect targets — the reason the original design used two mode systems
(each resolving its own focused entity) no longer applies.

**Freshness caveat (load-bearing).** Today `apply_tmux_shortcuts` targets
`ActivePane` DIRECTLY, which the input pipeline guarantees is fresh in
`InputPhase::FocusedKey` (click-to-focus retargets `ActivePane` in `Dispatch`
before the keyboard dispatcher reads it). `KeyboardFocused` is instead
maintained by `sync_keyboard_focus_to_active_pane` (`pane_focus.rs`), which runs
`in_set(TmuxActiveSet)` with NO ordering edge to `OzmuxSystems::Input` and
applies via DEFERRED `commands`. So on a frame where the active pane changes,
`resolve_shortcuts` (in `FocusedKey`) could read a STALE `KeyboardFocused` and
target the previous pane / wrong `in_copy_mode`. This refactor therefore MUST
add an ordering edge so the mirror flush lands before the resolver reads it:
order `sync_keyboard_focus_to_active_pane` `.before(InputPhase::FocusedKey)`
(or move it into `InputPhase::Dispatch`). This is a NEW requirement the switch
from `ActivePane` to `KeyboardFocused` introduces.

## Non-goals

- No behaviour change. Same `classify_key_batch` output, same triggered events,
  same guards/suppressions. Default pane/window shortcuts stay no-ops; tmux
  keeps its exact prompt guards, batch-forward, paste/detach semantics.
- `classify_key_batch` / `KeyEffect` / `BatchContext` (`src/input/resolve.rs`)
  are UNCHANGED and reused verbatim. All apply-side observers
  (`src/action/*`, `on_paste_tmux`, `on_forward_pane_keys`, etc.) are unchanged.
- Not touching `detect_modifier_tap`, `step_leader`, or the IME/mouse paths.

## Design

### 1. `ShortcutBatch` Message + `resolve_shortcuts` System (new: `src/input/dispatch.rs`)

```rust
/// A frame's resolved shortcut decision, broadcast from `resolve_shortcuts` to
/// the mode-specific appliers. Carries only the effects the appliers apply —
/// mode-agnostic effects (Quit, ReleaseWebviewFocus) are handled in
/// `resolve_shortcuts` and never appear here.
#[derive(Message)]
struct ShortcutBatch {
    effects: Vec<KeyEffect>,
    /// The `KeyboardFocused` surface (default terminal / active tmux pane), or
    /// `None` when the focused-surface query did not resolve to exactly one.
    focused: Option<Entity>,
    in_copy_mode: bool,
    mods: Modifiers,
}

/// Ordered sub-phases within `InputPhase::FocusedKey`: `resolve_shortcuts` runs
/// in `Resolve`, the mode appliers in `Apply`, so the Message is written before
/// it is read (same-frame).
#[derive(SystemSet, ...)]
pub(crate) enum ShortcutSet { Resolve, Apply }
```

`resolve_shortcuts` (runs BOTH modes; `run_if(on_message::<KeyboardInput>)`;
`in_set(InputPhase::FocusedKey)` + `in_set(ShortcutSet::Resolve)` +
`in_set(LeaderGate::Advance)`):

- Params (no `Commands` — it triggers no entity events; those move to the
  appliers): `MessageWriter<AppExit>`, `MessageWriter<ShortcutBatch>`,
  `MessageReader<KeyboardInput>`, `ResMut<LeaderPhase>`, `ResMut<FocusedWebview>`,
  `Res<Shortcuts>`, `Res<ResolvedCopyModeKeys>`, `Res<ButtonInput<KeyCode>>`,
  `Res<Time<Real>>`, `Res<ImeState>`, `Query<&Window, With<PrimaryWindow>>`,
  the focused-surface query
  `Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>`,
  `Query<(), With<CopyModeState>>`, `Query<&ForwardKeys>`, and the tmux prompt
  guards. `ConfirmState` / `RenamePrompt` are genuinely transient (inserted
  on demand by tmux-only plugins, absent in Default) so they MUST be
  `Option<Res<..>>`. `CopyPrompt`, however, is `init_resource`'d by a GLOBAL
  plugin (`CopyPromptPlugin`, `main.rs`) and is always present, including in
  Default — a plain `Res<CopyPrompt>` also works. NOTE: `resolve_shortcuts`
  evaluating the `copy_prompt.open.is_some()` guard in Default is a guard the
  old `apply_default_shortcuts` did not have; it is behaviour-neutral ONLY
  because `CopyPrompt.open` is currently never set (inert). Record this as an
  accepted, latent difference.
- **Param count / bundling:** this signature is ~17 flat params, over Bevy's
  16-`SystemParam` tuple-arity limit, so it will not compile flat. Tuple-bundle
  the related params (as `apply_tmux_shortcuts` already does for its prompts) or
  use `#[derive(SystemParam)]` structs.
- Guards: if any prompt open / IME composing / window unfocused →
  `clear_leader_phase`, `events.clear()`, return (NO message written → appliers
  don't run).
- Resolve `focused = query.single().ok()`,
  `in_copy_mode = focused.is_some_and(|e| copy_modes.get(e).is_ok())`,
  `webview_focused`, `forward_chords` (from the focused webview's `ForwardKeys`),
  `mods`. Build `BatchContext` and call the UNCHANGED
  `classify_key_batch(&mut leader_phase, &shortcuts, &resolved_copy, events.read(), ctx)`.
- Walk the effects: `Action{Quit}` → `exit.write(AppExit::Success)`;
  `ReleaseWebviewFocus` → `focused_webview.0 = None`; every other effect is
  pushed into the batch. Write one `ShortcutBatch { effects, focused,
  in_copy_mode, mods }`.

### 2. Thin mode appliers (consume `ShortcutBatch`)

Each replaces the current applier body; both drop `LeaderPhase`,
`FocusedWebview`, the prompt guards, the `copy_modes` query, the focused-entity
query, and `bevy_keys` — everything now arrives in the batch.

**`apply_default_shortcuts`** (`src/input/default_mode.rs`;
`run_if(in_state(AppMode::Default))` + `run_if(on_message::<ShortcutBatch>)` +
`in_set(ShortcutSet::Apply)`): params `Commands`,
`MessageReader<ShortcutBatch>`, `Res<Shortcuts>` (still required — the
release-webview-focus chord arrives as `KeyEffect::Type` and Default must drop
it while tmux forwards it, so the applier calls
`shortcuts.is_release_webview_focus(key_code, batch.mods)`; it cannot be dropped
from the params). For each batch, for each effect:
- `Action{EnterCopyMode}` → trigger `EnterCopyModeActionEvent{ batch.focused }` if `Some`.
- `Action{Paste, via_leader}` → trigger `PasteAction{ batch.focused }` if `Some` and `via_leader || !batch.in_copy_mode`.
- `Action{pane/window/detach}` → no-op (exhaustive list, as today).
- `CopyMode(a)` → `trigger_copy_mode_action(batch.focused, a)` if `Some`.
- `Type{logical, key_code}` → drop if `is_release_webview_focus`; else trigger
  `TerminalKeyInput{ batch.focused, key, TerminalModifiers-from-batch.mods }` if `Some`.
- `WebviewForward` → **no-op** (kept as an explicit arm). NOTE: unlike today
  (where Default hardcodes `forward_chords: &[]`, so the decider never emits
  this), `resolve_shortcuts` now resolves the focused webview's real
  `ForwardKeys` in BOTH modes, so the decider CAN emit `WebviewForward` in
  Default when a focused Default webview declares forward chords. Behaviour is
  preserved by this drop arm (Default has no pane to forward to; the key was
  dropped before, and is dropped here) — the drop lives in the applier, not in
  "empty forward_chords".

**`apply_tmux_shortcuts`** (`src/input/tmux/input.rs`;
`run_if(in_state(AppMode::Tmux))` + `run_if(on_message::<ShortcutBatch>)` +
`in_set(ShortcutSet::Apply)` + `in_set(TmuxActiveSet)`): params `Commands`,
`MessageReader<ShortcutBatch>`, `ActionTargets`. `batch.focused` IS the active
pane (the `KeyboardFocused` surface in tmux). For each effect:
- `Action{EnterCopyMode}` → trigger on `batch.focused` if `Some` and `!batch.in_copy_mode`.
- `Action{Paste}` → `PasteAction{ batch.focused }`.
- `Action{DetachSession}` → `DetachSessionRequest{ targets.session }`.
- `Action{pane/window}` → `dispatch_tmux_action(..)` on `batch.focused` / `targets`.
- `CopyMode(a)` → `trigger_copy_mode_action(batch.focused, a)`.
- `Type` / `WebviewForward` → `bevy_key_to_tmux_name(.., KeyMods-from-batch.mods)`
  into a `names` Vec → one `ForwardPaneKeysRequest{ batch.focused, names }` after
  the loop.

### 3. Ordering and registration

- New sub-phase set `ShortcutSet::{Resolve, Apply}` (chained) inside
  `InputPhase::FocusedKey`. `resolve_shortcuts` in `Resolve`, both appliers in
  `Apply`; the `.chain()` guarantees the `ShortcutBatch` write precedes its
  reads in the same frame (avoids a one-frame apply lag).
- `LeaderGate` is unchanged (`{Detect, Advance}`); `resolve_shortcuts` is the
  sole `LeaderPhase`-stepping system (in `Advance`), `detect_modifier_tap` still
  precedes it in `Detect`. The appliers no longer touch `LeaderPhase`, so they
  leave `LeaderGate`.
- `resolve_shortcuts` + `add_message::<ShortcutBatch>()` + the `ShortcutSet`
  chain are registered by a new `DispatchPlugin` (`src/input/dispatch.rs`),
  added by `OzmuxInputPlugin`. `apply_default_shortcuts` stays registered by
  `DefaultHostInputPlugin`, `apply_tmux_shortcuts` by the tmux `InputPlugin` —
  each now gated on `on_message::<ShortcutBatch>` + `ShortcutSet::Apply` instead
  of `on_message::<KeyboardInput>` + `LeaderGate::Advance`.
- **`KeyboardFocused` mirror must be fresh before the resolver reads it**
  (the freshness caveat above): order `sync_keyboard_focus_to_active_pane`
  (`src/ui/tmux/pane_focus.rs`) `.before(InputPhase::FocusedKey)` — expressed as
  a cross-plugin ordering edge on the `InputPhase::FocusedKey` set, not a
  `.after(fn)` — so its deferred `commands` flush lands before
  `resolve_shortcuts` reads `KeyboardFocused`. Without this edge, the tmux target
  can lag one frame when the active pane changes.
- New items in `dispatch.rs` start at the narrowest visibility that compiles
  (`ShortcutBatch` / `resolve_shortcuts` private or `pub(super)`; `ShortcutSet`
  `pub(crate)` only because the appliers in other files reference it).

## Components and data flow

```
[detect_modifier_tap]  (LeaderGate::Detect — unchanged)
        v
[resolve_shortcuts]  (ShortcutSet::Resolve / LeaderGate::Advance; BOTH modes)
  guard (Option prompts / ime / focus) -> clear_leader + drain + return (no msg)
  focused = Query<(OzmaTerminal, KeyboardFocused)>.single().ok()
  in_copy_mode / webview_focused / forward_chords / mods
  classify_key_batch(&mut LeaderPhase, ...)  -> Vec<KeyEffect>   (UNCHANGED decider)
  handle inline: Action{Quit} -> AppExit ; ReleaseWebviewFocus -> FocusedWebview.0 = None
  MessageWriter<ShortcutBatch>.write({ effects (rest), focused, in_copy_mode, mods })
        |
        +-------------------------+  run_if(in_state) + on_message::<ShortcutBatch>, ShortcutSet::Apply
        v                         v
[apply_default_shortcuts]   [apply_tmux_shortcuts]
  effect -> trigger on batch.focused   effect -> trigger on batch.focused / targets
  (TerminalKeyInput/PasteAction/…)     (*Request / PasteAction / ForwardPaneKeysRequest / …)
        v
[observers]  (src/action/* — unchanged)
```

## Testing

- `classify_key_batch` unit tests (`src/input/resolve.rs`): UNCHANGED.
- New `resolve_shortcuts` tests (`src/input/dispatch.rs`, Bevy `App`): the
  guards drain + emit no `ShortcutBatch`; a normal batch emits one
  `ShortcutBatch` with the expected `effects`/`focused`/`in_copy_mode`; `Quit`
  writes `AppExit` and is NOT in the batch; `ReleaseWebviewFocus` clears
  `FocusedWebview` and is NOT in the batch; the focused surface resolves for
  both a plain `KeyboardFocused OzmaTerminal` and a `TmuxPane`+`KeyboardFocused`.
- Applier tests (`default_mode.rs` / `tmux/input.rs`): reworked to WRITE a
  `ShortcutBatch` (instead of `KeyboardInput`) and assert the triggered events —
  smaller/faster than today (no leader/guard setup). Keep the discriminating
  cases: Default pane/window no-op, `direct_paste_in_copy_mode_suppressed`
  (via `batch.in_copy_mode`), tmux target correctness, one
  `ForwardPaneKeysRequest` per batch, `quit`/`release` handled upstream (assert
  the appliers do NOT need them).
- Ordering regression tests (the two load-bearing schedule edges): (1) write a
  `KeyboardInput` and assert the resulting `ShortcutBatch` is consumed in the
  SAME `Update` (guards the `ShortcutSet::Resolve.chain(Apply)` edge — a dropped
  chain would defer consumption a frame); (2) with a tmux fixture, change
  `ActivePane` and press a key in the same tick, asserting the batch's `focused`
  is the NEW pane (guards the `sync_keyboard_focus_to_active_pane`
  `.before(FocusedKey)` edge).
- Full gate: `cargo test -p ozmux`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo fmt`.

## Behaviour-preservation checklist

- Same `classify_key_batch` decision (byte-identical decider). Guards: the
  transient `ConfirmState`/`RenamePrompt` are absent in Default (Option → None),
  so the Default guard set is IME/focus as before; the always-present
  `CopyPrompt` guard now also runs in Default but is inert (never set), so it is
  behaviour-neutral. Same `LeaderPhase` stepping (now in exactly one system).
- `focused` resolves to the same entity each applier used today (default
  terminal / active pane), because `KeyboardFocused` already tracks the active
  surface in both modes.
- Quit / ReleaseWebviewFocus fire identically (moved upstream, still every
  frame the effect is produced). Default direct-paste-in-copy-mode suppression
  and tmux EnterCopyMode re-entry guard preserved via `batch.in_copy_mode`.
- One `ForwardPaneKeysRequest` per frame in tmux; snap/flush unchanged
  (observer untouched).
- No one-frame lag: `ShortcutSet::Resolve.chain(Apply)` keeps write-before-read
  in the same frame.

## Risks and staging

- **Two load-bearing schedule edges** (both covered by the ordering tests
  above): (1) the same-frame Resolve→Apply ordering — the `ShortcutSet` chain
  makes it explicit; dropping it defers apply a frame (visible input lag).
  (2) the `KeyboardFocused` mirror must flush before the resolver reads it —
  `sync_keyboard_focus_to_active_pane.before(InputPhase::FocusedKey)`; without
  it, the tmux target lags one frame on active-pane changes. Both are cheap edges
  but silently break correctness if omitted, so both get a regression test.
- Staging: this must be a SINGLE atomic cutover — `resolve_shortcuts` and the
  old appliers cannot coexist (both would step `LeaderPhase`, double-firing).
  So one change: add `src/input/dispatch.rs` (`ShortcutBatch`,
  `resolve_shortcuts`, `ShortcutSet`, `DispatchPlugin`) AND rewrite both appliers
  to consume `ShortcutBatch`, in the same commit, then verify the full gate.
  (The pure `classify_key_batch` is unchanged, so the risky decision logic is
  not touched — only the wiring around it moves.)

## Alternatives considered

- **Message carries only `Vec<KeyEffect>`; appliers recompute `focused` /
  `in_copy_mode`.** Rejected: reintroduces the per-mode context duplication this
  refactor removes; the `KeyboardFocused` invariant lets the resolve System
  compute them once and ship them in the batch.
- **Keep Quit / ReleaseWebviewFocus in both appliers.** Rejected: they are
  mode-agnostic, so handling them once in `resolve_shortcuts` keeps the appliers
  purely mode-specific (no duplicated global-effect arms).
- **Keep two mode systems calling the shared decider (the shipped design).**
  This is what we are moving away from; it duplicates the read+decide
  scaffolding, which is the stated motivation for this change.

## Out of scope

- `classify_key_batch` and all `src/action/*` observers (unchanged).
- `forward_wheel_to_tmux` / mouse / IME / copy-mode applier.
- The deferred behaviour edge-cases from PR #239 (DetachSession session-entity
  precondition, webview per-key `continue`, repeat-while-Pending drop) — carried
  over unchanged.
