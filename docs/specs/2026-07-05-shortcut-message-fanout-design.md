# Shortcut dispatch: fan `ShortcutBatch` into per-responsibility messages

## Problem

`resolve_key_effects` (`src/input/keyboard/handler.rs`) packs a frame's decided
keyboard effects into a single `ShortcutBatch` message and the per-mode appliers
(`apply_tmux_shortcuts`, `apply_default_shortcuts`) `match` over its
`Vec<KeyEffect>`. `KeyEffect` mixes genuinely different responsibilities under
one transport: a bound shortcut action (`Action`), copy-mode keys (`CopyMode`),
raw typing (`Type`), and webview key forwarding (`WebviewForward`). Bundling all
four into one grab-bag batch obscures which system owns which effect and forces
every applier through one wide `match`.

## Goal

Replace the single `ShortcutBatch` transport with **four typed, context-carrying
messages**, one per responsibility, fanned out by `resolve_key_effects`. Delete
`ShortcutBatch`. Split the per-mode appliers so each system reads exactly the
message it owns.

The pure decision layer (`classify_key_batch` and its `KeyEffect` IR) is
unchanged in spirit — only the transport between resolve and apply changes.

## Non-goals

- No change to the leader state machine (`step_leader`, `LeaderPhase`,
  `LeaderGate`), the modifier-tap machine, or `Shortcuts` resolution.
- No change to `classify_key_batch`'s decision logic or its unit tests' intent.
- No change to `ShortcutSet::Resolve → Apply` ordering or `InputPhase`.
- `Quit` / `ReleaseWebviewFocus` stay handled inline in `resolve_key_effects`
  exactly as today (never become messages).

## Design

### 1. `KeyEffect` stays the pure classifier IR

`classify_key_batch` keeps returning `Vec<KeyEffect>` carrying pure payloads
only (no `focused` entity, no message types). The `Action` variant is renamed to
`Shortcut` for naming parity with `ShortcutMessage`, but the variant keeps its
pure fields `{ action, via_leader }` — it does **not** wrap the `Message` struct,
because the pure decider never knows the focused `Entity`. The fan-out to
messages happens one layer up, in `resolve_key_effects`.

Rationale: `classify_key_batch` is pure and fully unit-tested without a Bevy
`App`. Keeping its return type context-free preserves that test suite
(`src/input/keyboard/key_effect.rs`, ~30 tests) intact.

### 2. Four typed, context-carrying messages (tailored fields)

Each message carries only the frame context its consumer actually uses. All four
are `pub(in crate::input)` (matching `ShortcutBatch`'s current visibility — they
are cross-file within `input` only, never crate-external) and each carries a
`///` summary per the repo doc rule. The field lists below omit these for
brevity:

```rust
#[derive(Message)]
struct ShortcutMessage {
    action: Shortcut,
    via_leader: bool,
    focused: Option<Entity>,
    in_copy_mode: bool,
}

#[derive(Message)]
struct CopyModeMessage {
    action: CopyModeAction,
    focused: Option<Entity>,
}

#[derive(Message)]
struct TypeMessage {
    logical: Key,
    key_code: KeyCode,
    focused: Option<Entity>,
    mods: Modifiers,
}

#[derive(Message)]
struct WebviewForwardMessage {
    logical: Key,
    key_code: KeyCode,
    focused: Option<Entity>,
    mods: Modifiers,
}
```

Field tailoring, derived from what the current appliers read off the batch:

- `ShortcutMessage` needs `focused` (trigger target) and `in_copy_mode` (the
  copy-mode re-entry guard for `EnterCopyMode`, and the
  `via_leader || !in_copy_mode` paste-suppression rule). It does **not** need
  `mods` — no shortcut action consumes the modifier snapshot.
- `CopyModeMessage` needs only `focused`.
- `TypeMessage` / `WebviewForwardMessage` need `focused` and `mods` (to build
  the tmux key name / terminal modifiers). They do **not** need `in_copy_mode`.

Ordering safety: `classify_key_batch` reads `webview_focused` once as a
batch-level constant, so `Type` / `CopyMode` (terminal branch) and
`WebviewForward` (webview branch) are **mutually exclusive within a frame** —
only one forwarding queue is ever non-empty. `Shortcut`, however, is emitted in
**both** branches (a leader action fires under webview focus too), so it can
coexist with `WebviewForward` or with `Type` / `CopyMode` in one frame.

This coexistence is the one ordering hazard the split introduces. Today a single
applier triggers the shortcut/copy-mode commands while iterating and then
triggers the once-per-frame `ForwardPaneKeysRequest` **after** the loop
(`tmux.rs`), so shortcut-triggered commands are always queued before the forward
request. Independently-scheduled appliers do not preserve that order. §4 pins it
back with an explicit `apply_*_shortcuts` / `apply_*_copy_mode` **before**
`apply_*_forward` ordering, restoring the current deterministic command-queue
order. Within the forward queue, key order is preserved by `MessageReader` write
order.

### 3. Producer: `resolve_key_effects` fans out; `ShortcutBatch` deleted

`resolve_key_effects` keeps its coarse-guard early return, `Quit` / release
inline handling, and `CefKeyboardFilter` logic unchanged. The single
`batch.write(ShortcutBatch { .. })` is replaced by a loop that stamps the frame
context (`focused`, `in_copy_mode`, `mods`) and writes each remaining
`KeyEffect` to its typed writer:

- `KeyEffect::Shortcut { action, via_leader }` → `ShortcutMessage`
- `KeyEffect::CopyMode(action)` → `CopyModeMessage`
- `KeyEffect::Type { logical, key_code }` → `TypeMessage`
- `KeyEffect::WebviewForward { logical, key_code }` → `WebviewForwardMessage`

The four `MessageWriter`s are bundled into one `#[derive(SystemParam)]`
(`ShortcutMessages`) so `resolve_key_effects` stays within Bevy's
system-parameter limit.

`ShortcutBatch` and its `add_message::<ShortcutBatch>()` registration are
removed; the four new messages are registered in `ShortcutsPlugin` (they are the
cross-file transport, so registering them in the plugin that owns
`ShortcutSet` is consistent).

Existing-worktree note: `src/input/shortcuts.rs` already carries a partial
`#[derive(Message)] pub struct ShortcutMessage { action, via_leader }` (a bare
`pub`, no doc comment, missing `focused` / `in_copy_mode`). This refactor
**replaces** that stub with the final form above — widen the fields, demote its
visibility to `pub(in crate::input)`, and add the `///` summary — rather than
adding a second type alongside it.

### 4. Consumers: split appliers per message

Each applier reads exactly the message it owns and is gated on its own
`run_if(on_message::<T>)`. Systems are registered by the plugin in their
defining file (`ShortcutsTmuxModePlugin` / `ShortcutsDefaultModePlugin`),
staying in `ShortcutSet::Apply`.

| Mode    | System                   | Reads                                     | Effect                                                             |
| ------- | ------------------------ | ----------------------------------------- | ----------------------------------------------------------------- |
| tmux    | `apply_tmux_shortcuts`   | `ShortcutMessage`                         | copy-mode entry / paste / detach / pane-window request triggers   |
| tmux    | `apply_tmux_copy_mode`   | `CopyModeMessage`                         | `trigger_copy_mode_action`                                        |
| tmux    | `apply_tmux_forward`     | `TypeMessage` + `WebviewForwardMessage`   | one `ForwardPaneKeysRequest` per frame                            |
| default | `apply_default_shortcuts`| `ShortcutMessage`                         | copy-mode entry / paste (direct suppressed in copy mode)          |
| default | `apply_default_copy_mode`| `CopyModeMessage`                         | `trigger_copy_mode_action`                                        |
| default | `apply_default_type`     | `TypeMessage`                             | `TerminalKeyInput`                                                |

`WebviewForwardMessage` has no consumer in Default mode (it was a no-op in the
current `apply_default_shortcuts`), so no Default system reads it.

`apply_tmux_forward` reads both `TypeMessage` and `WebviewForwardMessage`
because both map to `bevy_key_to_tmux_name` → a single `ForwardPaneKeysRequest`.
Since the two never coexist in a frame, only one queue is non-empty per frame;
the system is gated on `on_message::<TypeMessage>` OR
`on_message::<WebviewForwardMessage>` (`SystemCondition::or`, already used in the
repo, e.g. `src/input/mouse.rs`).

Ordering constraint (preserves current behavior): within each mode, the shortcut
and copy-mode appliers must run **before** the forward applier —
`(apply_tmux_shortcuts, apply_tmux_copy_mode).before(apply_tmux_forward)` and the
Default equivalent — because a `Shortcut` can share a frame with a forwarded key,
and today shortcut/copy commands are queued before the frame's
`ForwardPaneKeysRequest`. Expressed with `.before()` / a shared sub-set inside
`ShortcutSet::Apply`, per the repo's cross-file-ordering rule.

`dispatch_tmux_action`, `tmux_pane_direction`, `tmux_split_direction` are
unchanged; `apply_tmux_shortcuts` still calls `dispatch_tmux_action` for the
pane/window actions, now sourcing `focused`/`in_copy_mode` from `ShortcutMessage`
instead of `ShortcutBatch`.

## Test updates

- `key_effect.rs` unit tests: unchanged in intent. Mechanically update the
  renamed `Action` → `Shortcut` variant name in assertions.
- `handler.rs` tests (`capture_batch` etc.): the harness captures the new typed
  messages instead of `ShortcutBatch`; assertions rewritten per message type.
  The "exactly one message per keyboard frame" and guarded-frame "no message"
  invariants become per-type assertions.
- `default_mode.rs` and `tmux/input.rs` test harnesses: their `dispatch`
  helpers write the new typed messages (with stamped context) instead of a
  `ShortcutBatch`.

## Considered alternatives

- **Three messages (fold `WebviewForward` into `Type`).** `resolve_key_effects`
  already knows the `AppMode`, and in tmux `WebviewForward` and `Type` are
  handled identically (both → `bevy_key_to_tmux_name`), while in Default
  `WebviewForward` is a no-op. So the fan-out could map
  `KeyEffect::WebviewForward` → `TypeMessage` only in tmux and drop it in
  Default, eliminating `WebviewForwardMessage`, the OR-gate on
  `apply_tmux_forward`, and a never-read Default queue (3 messages / 5 appliers).
  **Deferred** in favor of the 4-message form chosen during brainstorming: the
  4-message split keeps a webview-forwarded key semantically distinct from a
  typed key and keeps `resolve_key_effects` mode-agnostic in its fan-out. Revisit
  if the OR-gate or the dead Default queue proves awkward in implementation.

- **`EntityEvent` + observer instead of `Message`.** The repo's documented
  primary handoff idiom is `EntityEvent` + observer (entity-targeted,
  flush-ordered), and this dispatch is entity-targeted (`focused`) and applies
  same-frame. `Message` is chosen anyway because the appliers are **mode
  routers** — they branch on `AppMode` and resolve session/window targets — and
  the leaf effects they emit (`EnterCopyModeActionEvent`, `PasteAction`,
  `TerminalKeyInput`, `trigger_copy_mode_action`) are *already* `EntityEvent`s.
  A buffered `Message` transport into mode-gated appliers is the right vehicle;
  per-effect observers would each have to duplicate the mode branch and target
  resolution.

## Risks

- **Message-count churn.** Six appliers (3 tmux + 3 default) replace two. Each
  is focused and matches the repo's "keep systems focused; split by
  responsibility" rule, so this is intended, not accidental growth.
- **Context re-derivation drift.** Not applicable — context is stamped once by
  `resolve_key_effects` at Resolve time and carried on each message (Option B),
  so appliers see the exact snapshot the keys were classified against, same as
  the old batch.
