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

Each message carries only the frame context its consumer actually uses:

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

Ordering safety: `classify_key_batch` branches on `webview_focused` at the top,
so a single frame emits either the webview family (`WebviewForward` + leader
`Shortcut`) or the terminal family (`Type` / `CopyMode` / `Shortcut`), never
both. Splitting the transport into independent message queues therefore
introduces no cross-type ordering hazard. Within a family, `Type` order is
preserved by `MessageReader` write order; `Shortcut` triggers are deferred
commands whose ordering relative to the once-per-frame forward request is
already flush-ordered today.

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
the system is gated on `on_message::<TypeMessage>` OR `on_message::<WebviewForwardMessage>`.

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

## Risks

- **Message-count churn.** Six appliers (3 tmux + 3 default) replace two. Each
  is focused and matches the repo's "keep systems focused; split by
  responsibility" rule, so this is intended, not accidental growth.
- **Context re-derivation drift.** Not applicable — context is stamped once by
  `resolve_key_effects` at Resolve time and carried on each message (Option B),
  so appliers see the exact snapshot the keys were classified against, same as
  the old batch.
