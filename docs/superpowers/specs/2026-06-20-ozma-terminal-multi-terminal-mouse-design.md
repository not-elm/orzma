# ozma_terminal Multi-Terminal Mouse Input — Design

**Date:** 2026-06-20
**Status:** Design (pre-plan)
**Crate:** `ozma_terminal` (+ host `src/ozma_input.rs`)

## Goal

Make the self-contained `ozma_terminal` crate's **mouse** handling correct when
more than one `OzmaTerminal` entity is live at once, including the case where
terminals **overlap** with a host-assigned z-order. Mouse interaction (app
reporting, text selection + copy, wheel scrollback, Cmd/Ctrl-click hyperlinks,
hover underline) must route to the terminal **under the cursor**, picking the
topmost on overlap, and each terminal must keep its own gesture/selection state.

## Scope

**In scope (mouse only):**

- `dispatch_mouse_buttons`, `dispatch_mouse_wheel` (`crates/ozma_terminal/src/mouse.rs`)
- `hyperlink_hover_cursor` / `resolve_hover` (`crates/ozma_terminal/src/hyperlink.rs`)
- Per-terminal input gating: split the single `InputDisabled` marker into
  `KeyboardDisabled` + `MouseDisabled`.
- Host marker maintenance: `crates/.../src/ozma_input.rs` `maintain_input_disabled`.

**Out of scope (flagged dependencies, not changed here):**

- **Keyboard routing.** `dispatch_input` keeps `terminal.single()` over
  `Without<KeyboardDisabled>`. With multiple terminals the host is responsible
  for keeping exactly one terminal un-`KeyboardDisabled` (the keyboard-focused
  one). A true multi-terminal keyboard-focus mechanism is a separate effort.
- **Layout.** `LayoutPlugin::resize_to_window` (`layout.rs`) uses
  `terminal.single_mut()` and sizes the one terminal to fill the window. A real
  multi-terminal host must own per-terminal sizing/positioning and PTY resize;
  the crate's window-fill convenience does not apply to overlapping terminals.
- **Exit.** `exit.rs` fires `AppExit` on shell exit; multi-terminal exit policy
  is unaddressed here.
- **Single-window assumptions retained.** Both dispatchers keep `windows.single()`
  over `With<PrimaryWindow>` and the `!window.focused` bail; these are orthogonal
  to terminal multiplicity (one window, many terminals) and stay valid.
- The host owns layout, z-stacking (`GlobalZIndex`), and keyboard focus.

## Background — why it breaks today

The mouse path assumes exactly one interactive terminal:

- `dispatch_mouse_buttons` (`mouse.rs:359`), `dispatch_mouse_wheel`
  (`mouse.rs:474`), and `resolve_hover` (`hyperlink.rs:95`) all call
  `terminal.single()` over `Query<…, (With<OzmaTerminal>, Without<InputDisabled>)>`.
  The moment a second un-disabled `OzmaTerminal` exists, `.single()` returns
  `Err` and **all** mouse input is dropped (and gesture state is reset).
- Gesture state is a single global resource (`OzmaMouseGesture`,
  `WheelAccumulator`) with no notion of *which* terminal a gesture targets.
- `InputDisabled` (`input.rs:18`) is one marker gating **both** keyboard and
  mouse, applied uniformly to all terminals by the host's
  `maintain_input_disabled` from one global "modal" bool
  (`picker || ime || !window_focused || webview_focused`).

### Precedent: the tmux backend already solves this

`src/tmux/mouse.rs` + `src/tmux/pane_hit.rs` route mouse across many panes:
`tmux_pane_at_phys` hit-tests **all** panes; `TmuxMouseGesture` is a single
state machine that **embeds the target pane `Entity`** in its states
(`Pressed { pane, … }`); a global `ModalGate` (picker/copy-prompt) suppresses;
keyboard focus is a separate `select-pane` concern. This design mirrors that
separation for `ozma_terminal`.

## Design decisions (from brainstorming)

1. **Multiplicity:** multiple, possibly **overlapping** terminals with z-order →
   hit-test must pick the **topmost** under the cursor.
2. **Scope:** **mouse only**; keyboard stays on `.single()`.
3. **Routing:** mouse acts on **any terminal under the cursor**, including a
   non-keyboard-focused one (scroll/select a background terminal). This forces
   mouse to be gated **independently** of the keyboard gate.
4. **Gating mechanism (user-directed):** rename `InputDisabled` →
   `KeyboardDisabled`; add a new `MouseDisabled` marker; check both **per
   terminal entity**. No new global resource.
5. **Selection:** independent per terminal (mirrors the tmux VT path — each
   `TerminalHandle` owns its selection).
6. **`MouseDisabled` hit-test semantics:** a `MouseDisabled` terminal is
   excluded from the candidate set (mouse **falls through** to the next terminal
   below it in an overlap region). Chosen default; "opaque" (swallow, no
   fall-through, à la `bevy_picking`'s `FocusPolicy::Block`) is a small future
   change. No current host path stacks a `MouseDisabled` terminal over a live one
   (modal suppression marks *all* terminals), so fall-through vs opaque is not yet
   observable — the decision is forward-looking and revisitable.

## Architecture

The pure decision logic — `decide_button`, `decide_wheel`, `update_selection`,
`accumulate_notches`, `wheel_delta_cells` — is **unchanged**. Only the
gather/dispatch systems and the gating markers change. This keeps the repo's
gather → decide → apply seam intact (decide stays pure + unit-testable; the
apply observer `on_terminal_mouse_effects` already targets `ev.entity` and needs
no change).

### 1. Routing — topmost terminal via `ComputedNode::stack_index`

Bevy 0.18.1's `ComputedNode` carries `stack_index: u32` (accessor
`stack_index()`), the resolved global front-to-back UI stacking position; a
higher value is drawn later, i.e. **topmost**. The dispatch systems already
query `&ComputedNode`, so topmost-under-cursor needs no `UiStack` resource:

```text
topmost = terminals
    .iter()
    .filter(|(_, node, transform, …)| node.contains_point(*transform, cursor_phys))
    .max_by_key(|(_, node, …)| node.stack_index())
```

Extract this as a single `pub(crate)` helper (`topmost_terminal_at`) reused by
all three systems rather than inlined three times — it mirrors the repo's
`tmux_pane_at_phys` hit-test home (`src/tmux/pane_hit.rs:31-45`) and is the
`App`-free unit-test target named in Testing. (`contains_point` is the same call
the tmux arbiter uses, `src/tmux/mouse.rs:206`.)

- **Wheel** and **hyperlink hover**: re-hit-test topmost once **per frame** (the
  wheel already coalesces all per-frame deltas into one `accumulate_notches` call
  and resolves the cursor once). Wheel gains a correctness fix — it scrolls only a
  terminal actually under the cursor (today it falls back to cell `(1,1)`,
  `mouse.rs:523-528`). No regression for a full-window terminal.
- **Buttons**: hit-test topmost only on **press**; drag/release stay locked to
  the press terminal (next section).
- Stacking is computed in `PostUpdate` (`UiSystems::Stack`); the `Update`-phase
  hit-test reads the previous frame's `stack_index`, the same one-frame lag as
  `ComputedNode` geometry. Acceptable. Two implementer caveats: (1) `stack_index`
  is written via `bypass_change_detection` (`stack.rs:101`) — do **not** gate on
  `Changed<ComputedNode>` for stacking (the dispatchers gate on input messages,
  which is correct); (2) `contains_point` tests only the node's transformed rect,
  **not** clip rects (unlike `bevy_picking`'s `clip_check_recursive`) — fine
  because `OzmaTerminal` nodes are absolutely-positioned and not nested in
  `overflow: clip` containers; revisit if that changes.

### 2. Per-terminal gesture state — entity-locked, single resource

Keep the single `OzmaMouseGesture` resource (one physical mouse ⇒ at most one
in-flight gesture; no per-entity map needed), but **embed the target `Entity`**
in `HeldPointer` (mirroring tmux's `Pressed { pane }`):

- **Press:** topmost hit → record `held = { entity, button, last_cell }` and the
  drag origin; trigger effects on that entity.
- **Drag motion / release:** look the press entity up via `query.get(entity)`
  (its `ComputedNode` / `UiGlobalTransform` / `TerminalGrid`), **not** a fresh
  topmost hit — a drag that wanders off the origin terminal keeps extending the
  origin terminal's selection, clamped to its grid. Effects target the stored
  entity. **Implementer note:** thread the press-entity geometry through
  `resolve_button_event` and `synthesize_drag` (which build the `CellContext`),
  not just `decide_button` — today they share one `CellContext` from
  `terminal.single()` (`mouse.rs:404-410`), and the off-node release fallback
  (`mouse.rs:583-586`) resolves against the *press* terminal's coordinate space.
- **Robustness:** if the press entity is no longer in the (filtered) query —
  despawned, or newly `MouseDisabled` mid-drag — `query.get` returns `Err`; the
  dispatcher drops the drag effect and **clears the gesture**. The apply observer
  already no-ops on a missing `ev.entity`.

`WheelAccumulator` (sub-notch residual) stays global but **resets its residual
when the hovered terminal changes** (track the last wheel-target entity), so a
pixel-scroll fraction from terminal A cannot bleed into terminal B; within one
terminal it self-corrects on sign flip as today.

### 3. Gating — `KeyboardDisabled` + `MouseDisabled`, per entity

Replace the single `InputDisabled` marker with two markers, each filtered
per-system:

| System | Filter |
| --- | --- |
| `dispatch_input` (keyboard) | `Without<KeyboardDisabled>` (still `.single()`) |
| `dispatch_mouse_buttons` / `dispatch_mouse_wheel` | `Without<MouseDisabled>` (hit-test topmost among these) |
| `hyperlink_hover_cursor` / `resolve_hover` | `Without<MouseDisabled>` |

- `KeyboardDisabled` lives in `input.rs` (renamed in place). `MouseDisabled` is a
  new marker in `mouse.rs` (responsibility-aligned module). Both re-exported from
  `lib.rs`, so the public gating API is colocated at the export site. This
  crate-local `MouseDisabled` is distinct from the host's renderer-side
  `NonInteractive` marker (`src/osc_webview.rs`, used in `src/tmux/mouse.rs`); the
  crate must not depend on host types, so they stay separate, and the host's
  `maintain_input_gates` is the sole writer of `MouseDisabled`.
- **Keyboard guardrail:** `dispatch_input` keeps `.single()` over
  `Without<KeyboardDisabled>`; add a `// NOTE:` at that site that a future
  multi-terminal host MUST keep exactly one terminal un-`KeyboardDisabled` or all
  keys are silently dropped (`.single()` → `Err`).
- **Modal suppression** needs no global resource: when the host marks **all**
  terminals `MouseDisabled` (picker / IME / webview-focused / window-unfocused),
  the mouse query candidate set is empty → the dispatcher drains events — exactly
  today's `.single()`-`Err` behavior. The empty-set branch is **per system**:
  `dispatch_mouse_buttons` also resets the gesture (`drag`/`held` → `None`,
  `mouse.rs:379-384`); `dispatch_mouse_wheel` only clears wheel events
  (`mouse.rs:493-496`); `hyperlink_hover_cursor` clears `HyperlinkHoverState`
  then returns the default cursor (`hyperlink.rs:70-75`).
- **Per-terminal mouse disable becomes expressible** — a single terminal can be
  `MouseDisabled` while others stay live; a single global flag could not express
  this. NOTE: the *current* host disables **all** terminal input whenever a
  webview is focused (`src/ozma_input.rs:51-56`), so that case is global today,
  not per-terminal. The per-terminal capability is latent headroom — do not cite
  webview-overlay as a live fall-through scenario.

### 4. Hyperlink hover & selection

- `hyperlink_hover_cursor` hit-tests topmost (`Without<MouseDisabled>`) and sets
  `HyperlinkHoverState.entity` to it → correct underline / cursor icon on the
  hovered terminal.
- Selections are **independent per terminal**; `MouseEffect::Copy` copies from
  the gesture's terminal. No cross-terminal selection clearing.

### 5. Host changes — maintain both markers

`crates/.../src/ozma_input.rs`:

- `maintain_input_disabled` → renamed `maintain_input_gates`; for each
  `OzmaTerminal`, sync **both** `KeyboardDisabled` and `MouseDisabled` to the
  existing global `disable` bool. Keep the `.before(OzmaTerminalInputSet)` /
  `.before(OzmaTerminalMouseSet)` ordering and `run_if(in_state(AppMode::Ozma))`.
- Single-terminal behavior is **fully preserved** (both markers track `disable`
  identically). Future multi-terminal keyboard focus adds a per-terminal
  "not focused" condition to `KeyboardDisabled` only.

## Alternatives considered

- **`bevy_picking` UI backend** (z-ordered, clip-aware hit-testing via
  `UiPickingPlugin` + `Pointer<…>` observers / `Pickable` / `FocusPolicy::Block`),
  active in this app via `DefaultPlugins`. **Rejected:** (1) zero picking
  precedent in the repo — the entire terminal *and* tmux input stack is raw
  `MouseButtonInput`/`CursorMoved` + manual `contains_point`; (2) the mouse path
  still needs cell-precise geometry, `ClickTracker` burst caps, off-node release
  fallback, drag synthesis, `WheelAccumulator`, and SGR byte forwarding — picking
  replaces only the ~3-line topmost selection while forcing a second (observer)
  event model; (3) `Pickable`/`Block` is a different gating vocabulary than
  per-entity `MouseDisabled` fall-through. Revisit only if clip-aware hit-testing
  becomes necessary.
- **Iterate `Res<UiStack>` back-to-front** instead of `max_by_key(stack_index)` —
  equivalent result; rejected to avoid the extra resource dependency since the
  data is already on the queried `ComputedNode`. Ties are moot (distinct nodes get
  distinct `stack_index`, `stack.rs:99-103`).

## Change surface

**Crate `ozma_terminal`:**

| File | Change |
| --- | --- |
| `input.rs:18` | `InputDisabled` → `KeyboardDisabled` (definition); update `//!`/doc at `:4`, `:67` |
| `input.rs:93`, `:269` | filter `Without<KeyboardDisabled>`; test spawn |
| `mouse.rs` | add `pub struct MouseDisabled;` + doc; fix `use` at `:24` (drop `InputDisabled`) |
| `mouse.rs:359`, `:474` | `Without<MouseDisabled>` + multi-terminal query + topmost hit-test + entity-locked gesture; update docs `:5`, `:78`, `:358`; test `:805` → `MouseDisabled` |
| `hyperlink.rs:6`, `:64`, `:99` | `use crate::mouse::MouseDisabled`; both queries `Without<MouseDisabled>` + topmost |
| `lib.rs:21` | export `KeyboardDisabled` (renamed) + add `MouseDisabled` |
| `spawn.rs:11-14` | fix `OzmaTerminal` doc ("Exactly one entity…") for the multi-terminal premise |

**Host `src/ozma_input.rs`:**

| Lines | Change |
| --- | --- |
| `:1`, `:17` | module doc; import `KeyboardDisabled, MouseDisabled` |
| `:42-64` | `maintain_input_disabled` → `maintain_input_gates`, sync both markers from `disable` |

## Testing

- **Pure:** `topmost_terminal_at` (or equivalent) picks the higher `stack_index`
  on overlap; ignores non-containing nodes.
- **Gesture lock:** press in terminal A, drag across terminal B, release →
  effects target A throughout (selection clamps to A's grid).
- **Gating split:** a `MouseDisabled` terminal drains mouse events without arming
  a gesture; a `KeyboardDisabled`-but-not-`MouseDisabled` terminal still receives
  mouse (proves decoupling).
- **Multi-terminal dispatch:** two live terminals no longer drop all input (the
  old `.single()` failure).
- Existing `decide_*` / accumulator tests stay green (logic untouched).

## Constraints (repo coding rules)

- Rust 2024 / toolchain 1.95. No `mod.rs`. Comments only `// TODO:` / `// NOTE:`
  / `// SAFETY:`, English. Every `pub` item `///`-documented; module files `//!`.
- Imports in one top block, no inline fully-qualified paths.
- Bevy: mutable `SystemParam`s before immutable; whole-system change gates via
  `run_if` not in-body early return; `Plugin::build` one method chain; `Query`
  params descriptive nouns (no `_q`).
- Visibility minimized; private items last in a block; `#[expect(reason=…)]` over
  `#[allow]`.
