# Mouse dispatch: per-operation EntityEvents on the apply side

Date: 2026-06-29
Status: design approved, pending spec review

## Goal

Refactor the mouse-effect apply path so each mouse operation is its own
`EntityEvent` with one focused observer, replacing the single
`MouseEffect` enum + `TerminalMouseEffects` carrier + one big-`match`
observer. The gather system (`dispatch_mouse_buttons`) stays a single
system; only the apply side is split per operation.

## Background

Two files own the shared mouse path today:

- `src/input/mouse.rs` — the gather/decide side. `dispatch_mouse_buttons`
  reads `MouseButtonInput` / `CursorMoved`, owns the `OzmaMouseGesture`
  state machine, hit-tests, drives the pure `decide_button` /
  `decide_wheel` deciders (which return `Vec<MouseEffect>`), and triggers
  `TerminalMouseEffects`.
- `crates/ozma_terminal/src/mouse.rs` — the apply side. One observer
  `on_terminal_mouse_effects` applies a `Vec<MouseEffect>` to the
  `TerminalHandle` / `Clipboard`, with two parallel match arms
  (`apply_effect` for the PTY-attached path, `apply_effect_detached` for
  the PTY-less / tmux-pane path).

`MouseEffect` and `TerminalMouseEffects` are used ONLY by these two
files. `TerminalForwardInput` (the PTY-less write-forwarding event) is
ALSO consumed by `src/input/tmux/forward.rs` and MUST be preserved.

The separate `TmuxMouseEffect` / `TmuxMouseEffects` machinery under
`src/input/tmux/mouse/` is a different layer (pane select / resize / copy
drag) and is OUT OF SCOPE.

## Why the gather system cannot be split per event

`dispatch_mouse_buttons` is a single sequential state machine and must
stay one system:

1. `MessageReader<MouseButtonInput>` cannot be partitioned — each system
   with a reader gets every message via its own cursor, so per-event
   reader systems all see the full stream.
2. Same-frame temporal interleaving (`press A → release A → press B`)
   would be lost if press / release / drag were separate systems,
   corrupting `OzmaMouseGesture` (held / drag phase / click count).
3. The drag-synthesis tail depends on `gesture.held` set earlier in the
   same run.

The achievable, repo-idiomatic separation is gather → decide → apply:
keep one slim gather system, keep the deciders pure, and split the apply
side into per-operation observers.

## Design

### Decision IR stays in the host, private

`decide_button`, `decide_wheel`, and `effects_from_wheel_action` keep
returning `Vec<MouseEffect>`. `MouseEffect` MOVES out of `ozma_terminal`
and becomes a **private** enum in `src/input/mouse.rs` — it is now purely
the host-internal decision intermediate representation, no longer a
cross-crate apply contract. This preserves the ~12 pure unit tests of the
deciders (no `App`/`World` needed) and keeps effect ordering decided in
one place.

The gather system, after computing `Vec<MouseEffect>`, translates each
effect into its corresponding per-operation `EntityEvent` and triggers it
via `commands.trigger(...)` **in Vec order**. Bevy 0.18's command queue
is FIFO and each `trigger` command fully resolves before the next (see
research note below), so e.g. `[SelClear, Write]` is observed as
`SelClear` then `Write`.

### New apply-side EntityEvents (in `crates/ozma_terminal/src/mouse.rs`)

One event per operation, each carrying the target `entity` via
`#[event_target]`, replacing the `MouseEffect` variants:

| Event | Fields | Replaces `MouseEffect::` |
| --- | --- | --- |
| `TerminalMouseWrite` | `entity`, `bytes: Vec<u8>` | `Write` |
| `TerminalSelectionStart` | `entity`, `point: Point`, `side: Side`, `ty: SelectionType` | `SelStart` |
| `TerminalSelectionUpdate` | `entity`, `point: Point`, `side: Side` | `SelUpdate` |
| `TerminalSelectionClear` | `entity` | `SelClear` |
| `TerminalSelectionCopy` | `entity` | `Copy` |
| `TerminalViewportScroll` | `entity`, `lines: i32` | `Scroll` |
| `TerminalOpenUri` | `entity`, `uri: String` | `OpenUri` |

`TerminalForwardInput` is unchanged (still emitted by the write path in
the PTY-less case).

### One observer per event

`on_terminal_mouse_effects` is replaced by one observer per event, each
registered in `OzmaMousePlugin` and each holding the
`(&mut TerminalHandle, Option<&mut PtyHandle>, Option<&mut Coalescer>)`
query. Each observer encapsulates its own attached-vs-detached branch:

- **Attached (PTY + Coalescer present):** apply through the coalescer
  exactly as `apply_effect` does today.
- **Detached (no PTY):** the selection/scroll observers mutate via the
  `*_vt_only` methods and then call `handle.flush_emit(&mut commands,
  entity)`; the write observer triggers `TerminalForwardInput`; the copy
  observer writes the clipboard; the open-uri observer calls
  `try_open_uri`.

Each event maps to exactly ONE observer, so the "same event, multiple
observers run in arbitrary order" caveat (research point 5) does not
apply.

### Detached flush behavior (resolved trade-off)

The current detached path batches: it applies all effects in one
`TerminalMouseEffects`, then calls `flush_emit` once. With per-event
observers, a single press whose decided effects are `[SelStart,
SelUpdate]` triggers two observers that each call `flush_emit`, i.e. two
frame emits in that frame. This is **correct** — each `flush_emit`
collects and emits only its own incremental dirty rows — and the extra
emit is rare (selection drag steps only) and low-cost. Accepted as-is; no
coalescing machinery is added.

## Components and data flow

```
[gather: dispatch_mouse_buttons — 1 system]
  read CursorMoved + MouseButtonInput
  drive OzmaMouseGesture state machine + hit-test
  decide_button() / decide_wheel()  -> Vec<MouseEffect>   (pure, host-private enum)
  for each MouseEffect in order: commands.trigger(<per-op EntityEvent>)
        |  FIFO, each trigger resolves before the next
        v
[apply: N observers, one per event, in ozma_terminal::mouse]
  on_terminal_mouse_write       -> PTY write | TerminalForwardInput
  on_terminal_selection_start   -> handle.selection_start_at[ _vt_only ] (+flush_emit if detached)
  on_terminal_selection_update  -> handle.selection_update_to[ _vt_only ] (+flush_emit if detached)
  on_terminal_selection_clear   -> handle.selection_clear[ _vt_only ] (+flush_emit if detached)
  on_terminal_selection_copy    -> clipboard.write(selection_to_string)
  on_terminal_viewport_scroll   -> handle.scroll[ _vt_only ] (+flush_emit if detached)
  on_terminal_open_uri          -> try_open_uri
```

## Testing

- **Deciders** (`src/input/mouse.rs`): unchanged — they still return
  `Vec<MouseEffect>` against the now-host-private enum. All existing
  `decide_button` / `decide_wheel` / `effects_from_wheel_action` tests
  keep working as-is.
- **Gather integration tests** (`make_selection_app`, `make_wheel_app`,
  `CapturedEffects`): reworked to observe the per-operation EntityEvents
  instead of `TerminalMouseEffects`. The capture resource records each
  triggered event type/payload; assertions that matched
  `MouseEffect::SelUpdate { .. }` etc. are rewritten against the
  corresponding event (e.g. `TerminalSelectionUpdate`).
- **Apply tests** (`crates/ozma_terminal/src/mouse.rs`):
  `detached_terminal_forwards_write_and_selects_via_vt_only` and
  `mouse_effects_on_entity_without_terminal_does_not_panic` reworked to
  trigger the per-operation events and assert the same outcomes
  (forward bytes emitted; selection set via vt_only; no panic on a
  missing terminal).
- Full gate: `cargo test`, `cargo clippy --workspace`, `cargo fmt`.

## Research note (Bevy 0.18 trigger ordering)

Confirmed against the pinned `bevy_ecs-0.18.0` source and the crate's own
doc comments:

- `Commands::trigger` queues an ordinary command; the command queue is
  FIFO and each trigger command runs its observers (and flushes their
  queued commands) before the next command applies. So multiple
  `commands.trigger(..)` from one system body are observed in source
  order — `SelClear` before `Write`. (`system/commands/mod.rs:1146`,
  `world/command_queue.rs:235-245`.)
- For a SINGLE event with MULTIPLE registered observers, relative order
  is explicitly arbitrary (storage is `EntityHashMap<ObserverRunner>`).
  This design uses one observer per event, so it is unaffected.
  (`observer/centralized_storage.rs:148`, `event/trigger.rs:103`.)

## Out of scope

- `TmuxMouseEffect` / `TmuxMouseEffects` (`src/input/tmux/mouse/`).
- The wheel gather system structure (`dispatch_mouse_wheel`) beyond
  swapping its `TerminalMouseEffects` trigger for per-op event triggers.
- Any change to `OzmaMouseGesture` semantics or the deciders' logic.
