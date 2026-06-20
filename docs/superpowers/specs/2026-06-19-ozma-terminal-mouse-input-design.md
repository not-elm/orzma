# ozma_terminal: Mouse Input

**Date:** 2026-06-19
**Status:** Draft

## Overview

Give the self-contained `crates/ozma_terminal` working mouse support, so a
consumer that spawns one `OzmaTerminalBundle` gets a terminal where the mouse
behaves like any modern terminal — with **no mouse wiring of its own**. Four
behaviors, all driven by routers that **already exist and are unit-tested** in
`ozma_tty_engine`:

- **App mouse reporting** — forward clicks / drags / wheel to TUI apps that
  enable mouse mode (vim, htop, less, an inner tmux) as SGR / X10 reports.
- **Text selection + copy** — click-drag, double / triple-click word / line,
  Alt+drag block, Shift to force-select even under app capture; copy-on-release
  to the system clipboard.
- **Wheel scrollback** — wheel scrolls the viewport when no app has capture;
  alt-screen apps get arrow-key translation instead.
- **Cmd-click hyperlinks** — Cmd/Ctrl-click an OSC-8 link to open it, plus a
  hover cursor (pointer over a link with the modifier held, I-beam over the
  grid).

This mirrors the keyboard work in
[`2026-06-19-ozma-terminal-input-action-design.md`](2026-06-19-ozma-terminal-input-action-design.md):
the crate owns the default mouse dispatcher; the host only maintains
`InputDisabled` and supplies config.

## Motivation

The standalone `ozma_terminal` (Ozma mode, `AppMode::Ozma`) has **zero** mouse
support today — only the keyboard `input.rs` dispatcher. Clicking, dragging,
selecting, and scrolling the wheel all do nothing. Apps that request mouse mode
receive no events.

### Key insight: the engine already owns the mouse decisions

`ozma_tty_engine` contains a complete, pure, Bevy- and PTY-agnostic mouse
stack with **no consumers** — orphaned when the old multiplexer's Bevy glue
was deleted during the tmux migration (commit `8c9a4d9`, "delete old
mouse/input modules"), while the routers survived because they are
Bevy-agnostic:

- `buttons.rs` — `ButtonAction::route(modes, evt, mods, cfg)`: the per-event
  decision. Chooses **forward to app** (SGR/X10 encode → PTY bytes) vs **local
  selection**, including the Shift-bypass and Alt-block rules.
- `wheel.rs` — `WheelAction::route(modes, notches, cell, mods, cfg)`: chooses
  **app wheel report** vs **alt-screen arrow translation** vs **host
  scrollback**.
- `mouse_encode.rs` — the shared SGR / X10 encoder (`encode_protocol_event`).

These return abstract actions (`WriteToPty(bytes)`, `StartLocalSelection{…}`,
`ScrollViewport(lines)`, …). `TerminalHandle` already exposes everything to
apply them: `current_modes()`, `write()`, `scroll()`, `selection_start_at()` /
`selection_update_to()` / `selection_clear()` / `selection_to_string()`. The
hyperlink predicate `grid.hyperlink_at()` and `HyperlinkHoverState` live in
`ozma_tty_renderer`.

So this is **Bevy glue only**: read input, hit-test the cursor to a cell, track
click count + drag state, call the router, apply the result. The protocol bytes
and routing logic are not re-implemented.

## Goals

- A consumer that spawns `OzmaTerminalBundle` gets all four behaviors with no
  mouse-specific wiring.
- The crate stays self-contained: it consumes only `ozma_tty_engine`,
  `ozma_tty_renderer`, `bevy`, `arboard`, and (new) `open`. No dependency on
  `ozmux_configs`, `ozmux_tmux`, or `bevy_cef`.
- Mouse is gated by the existing `InputDisabled` marker, identically to the
  keyboard dispatcher — the host's `src/ozma_input.rs` already maintains it.
- The security-sensitive URL-scheme allowlist has **one** source of truth
  shared by the standalone crate and the tmux path (no drift).

## Non-Goals (v1)

- **Drag-autoscroll into scrollback** (dragging a selection past the window
  edge to extend into off-screen lines). Deferred; the engine routers do not
  include it and it needs a timer-driven tick.
- **Inline-webview mouse routing** — the crate has no webview dependency.
- **Middle-click primary-selection paste** — `ButtonAction::route` intentionally
  `Noop`s middle/right on the local path.
- **Simultaneous multi-button gestures** — track one primary button at a time.
- **`Cmd+C` keyboard copy of a selection** — copy-on-release covers the mouse
  flow; keyboard copy is a separate concern.
- **Any change to the tmux mouse path** (`src/tmux/mouse.rs`).

## Architecture

### Crate module map (`crates/ozma_terminal/src`)

| File | Role |
| --- | --- |
| `mouse.rs` (new) | `OzmaMousePlugin`, `OzmaMouseConfig`, `OzmaTerminalMouseSet`, `OzmaMouseGesture`, the cursor→cell hit-test + `to_viewport_point` helpers, and the button + wheel dispatch systems (`ClickTracker` + `DragGesture` state machine, pixel→notch accumulation, `ButtonAction` / `WheelAction` application). Split into `mouse/buttons.rs` + `mouse/wheel.rs` only past ~350 lines (see note). |
| `hyperlink.rs` (new) | Hover-cursor system (updates `HyperlinkHoverState` + `CursorIcon`) and `try_open_uri` (allowlist check + `open::that_detached`). |
| `lib.rs` (extended) | `mod mouse; mod hyperlink;`; `OzmaTerminalPlugin` adds `OzmaMousePlugin`; re-export `OzmaMouseConfig`, `OzmaTerminalMouseSet`. |

Start with a flat `mouse.rs` — it matches the keyboard sibling `input.rs` (the
crate's current largest file at 360 lines), and the Ozma glue is materially
smaller than the tmux path (no panes / dividers / webview routing). Split into
`mouse/buttons.rs` + `mouse/wheel.rs` (mirroring the engine's own division) only
if the file grows past ~350 lines.

### Shared crate change (`ozma_tty_renderer`)

Move **only the URL-scheme allowlist** out of `src/input/hyperlink.rs` (which
keeps its tmux-coupled hover system and its own `try_open_uri`) into the
renderer `schema` module, where `TerminalGrid` already lives and which is a
dependency of **both** hosts:

- `schema::scheme_of(uri) -> Option<&str>` (private)
- `schema::is_allowed(uri) -> bool` — the `http` / `https` / `mailto` / `ftp`
  allowlist that rejects `javascript:` / `file:` / `data:`

`should_open_at` is **not** relocated: it takes `ozma_tty_engine::MouseButtonKind`
and `ButtonEventKind`, so moving it into `ozma_tty_renderer` would add a new
`renderer → engine` crate dependency that does not exist today
(`crates/ozma_tty_renderer/Cargo.toml` has no `ozma_tty_engine`). Instead each
host keeps its own trivial `Press + Left + modifier` gate and calls
`grid.hyperlink_at(row, col)` directly. The security-sensitive part — the
allowlist — is the single source of truth that must not drift, and that is what
moves. `src/input/hyperlink.rs` deletes its `scheme_of` / `is_allowed` copies
and delegates; its `try_open_uri` calls `schema::is_allowed`.

### Three input systems, cleanly partitioned

| System | Set / gating | Reads | Effect |
| --- | --- | --- | --- |
| `dispatch_mouse_buttons` | `OzmaTerminalMouseSet`, `run_if(on_message::<MouseButtonInput>.or(on_message::<CursorMoved>))`, skips `InputDisabled` | `MouseButtonInput`, window cursor | `ButtonAction::route` → PTY write / local selection / clipboard |
| `dispatch_mouse_wheel` | `OzmaTerminalMouseSet`, `run_if(on_message::<MouseWheel>)`, skips `InputDisabled` | `MouseWheel`, window cursor | `WheelAction::route` → PTY write / viewport scroll |
| `hyperlink_hover_cursor` | `OzmaTerminalMouseSet`, `run_if(on_message::<CursorMoved>.or(on_message::<KeyboardInput>))`, skips `InputDisabled` | window cursor, keys, `TerminalGrid` | updates `HyperlinkHoverState` + window `CursorIcon` |

## Component Specification

### `crates/ozma_terminal/src/mouse.rs`

`OzmaMousePlugin` registers the three systems in `OzmaTerminalMouseSet`,
`init_resource`s `OzmaMouseConfig`, `OzmaMouseGesture`, and the wheel
accumulator, and `add_message`s the Bevy input messages it gates on.

```rust
#[derive(Resource)]
pub struct OzmaMouseConfig {
    pub buttons: ButtonConfig,        // engine: max_protocol_events_per_frame
    pub wheel: WheelConfig,           // engine: lines_per_notch, fine_lines, max_…
    pub double_click_timeout: Duration,
    pub click_drift_px: f32,
    pub drag_threshold_px: f32,
    pub fine_modifier: FineModifier,  // crate-local enum (NOT ozmux_configs); selects WheelModifiers.fine
}
```

`Default`: 400 ms / 8 px drift / 4 px drag threshold, `WheelConfig::default()`
(3 lines-per-notch, fine = 1, cap 8). **Caveat:** `ButtonConfig` derives
`Default` with `max_protocol_events_per_frame = 0`, and a zero cap makes
`route()` drop every forwarded button event (`buttons.rs:117`). So
`OzmaMouseConfig::default` must set `buttons: ButtonConfig {
max_protocol_events_per_frame: 8 }` **explicitly** — not `ButtonConfig::default()` —
or app mouse reporting silently never fires.

Shared helpers (pure, unit-tested):

- `cell_at_cursor(node, transform, cursor_phys, cell_w, cell_h, cols, rows) -> (col0, row0, Side)`
  — 0-indexed cell + half-cell side, clamped to the grid. Same math as
  `src/tmux/pane_hit::cell_at_local`. `cols` / `rows` come from the live
  `TerminalGrid` (window-fill sized, `layout.rs`), not assumed.
- `to_protocol_cell(col0, row0) -> CellCoord` — `+1` to each (SGR/X10 is
  1-indexed).
- `to_viewport_point(cell: CellCoord) -> Point` — `Point { line: Line(row-1),
  column: Column(col-1) }`; the engine's `selection_start_at` translates this
  viewport-relative row through `viewport_row_to_line`, so scroll during a drag
  is handled engine-side.
- **Thresholds are logical px; the cursor is physical.** `drag_threshold_px` /
  `click_drift_px` are logical config units — multiply by `window.scale_factor()`
  before comparing against physical cursor distances (as `src/tmux/mouse.rs:366`
  does).

### Button dispatch (in `mouse.rs`, or `mouse/buttons.rs` if split)

State:

```rust
#[derive(Resource, Default)]
struct OzmaMouseGesture { click: ClickTracker, drag: Option<DragGesture> }

struct DragGesture {
    button: MouseButtonKind,
    origin: CellCoord,      // 1-indexed; selection anchor
    side: Side,
    ty: SelectionType,
    phase: DragPhase,       // Armed | Started
    last_cell: CellCoord,   // dedup + drag synthesis
}
enum DragPhase { Armed, Started }
```

The dispatch system queries `(&mut TerminalHandle, &mut PtyHandle, &mut
Coalescer)` on the single `OzmaTerminal` — all three are already components on
the entity (`layout.rs` queries the same trio) — and the `selection_*` /
`scroll` calls take `&mut Coalescer` to arm the renderer emit.

`ClickTracker` is the timeout+drift→count(1/2/3) tracker already proven in
`src/tmux/mouse.rs:177` (ported verbatim, not shared — ~19 lines with three
existing unit tests; the tmux copy stays put).

Per-frame `dispatch_mouse_buttons`:

1. Bail (clearing `MouseButtonInput`, resetting `gesture.drag`, keeping any
   committed selection) when: no single `OzmaTerminal` without `InputDisabled`,
   no window, window unfocused, or no cursor position.
2. Resolve cursor→cell (0-indexed for selection, 1-indexed `CellCoord` for the
   protocol). `modes = handle.current_modes()`; `mods` from `ButtonInput<KeyCode>`.
3. Per `MouseButtonInput`:
   - **Left Press with `link_modifier_held` + a link under the cursor**
     (`grid.hyperlink_at(row0, col0)` returns `Some(uri)`): `try_open_uri(uri)`,
     consume (no select / forward). Link check precedes selection, matching the
     tmux arbiter.
   - Else build `ButtonEvent { kind, button, cell: protocol_cell, side,
     click_count }` (count from `ClickTracker` on press) and call
     `ButtonAction::route(modes, evt, mods, &cfg.buttons)`; apply (table below).
4. After events: if `gesture.drag` is held and the resolved cell differs from
   `last_cell`, synthesize a `Drag` event, route it, apply, update `last_cell`.

`apply_button_action`:

| `ButtonAction` | Effect |
| --- | --- |
| `WriteToPty(b)` | `handle.write(&mut pty, &b)` |
| `ClearAndWriteToPty(b)` | `selection_clear(&mut coalescer)`; `handle.write(&mut pty, &b)` |
| `ArmDrag{cell,side,ty}` | `gesture.drag = DragGesture{phase: Armed, …}`; `selection_clear(&mut coalescer)` |
| `StartLocalSelection{cell,side,ty}` | `selection_start_at(&mut coalescer, to_viewport_point(cell), side, ty)`; `phase = Started` |
| `UpdateLocalSelection{cell,side}` | if `Armed` and `cell != origin`: `selection_start_at(&mut coalescer, origin)` then mark `Started`; `selection_update_to(&mut coalescer, to_viewport_point(cell), side)` (these arm the emit — no separate flush) |
| `ClearLocalSelection` | `selection_clear(&mut coalescer)` |
| `Noop` | — |

**Copy-on-release:** on Left Release with a live selection (`Started`, or a
double/triple-click word/line), `if let Some(text) = handle.selection_to_string()
{ clipboard.write(text); }` (the method returns `Option<String>`), then
`gesture.drag = None`. App-forward path never sets a selection.

### Wheel dispatch (in `mouse.rs`, or `mouse/wheel.rs` if split)

`WheelAccumulator` resource holds the fractional Pixel-unit remainder so pixel
deltas accrue into whole notches (Line-unit `y` maps directly). Per-frame
`dispatch_mouse_wheel`:

1. Same gating as buttons (single enabled terminal, focused window).
2. Sum `MouseWheel` events into integer `notches` (sign-significant; negative =
   up/older), keeping the remainder. `Line`-unit `y` maps directly; `Pixel`-unit
   `y` divides by `cell_h` to match the tmux convention (`src/tmux/input.rs:747`)
   so trackpad scroll speed is consistent across modes.
3. Resolve cursor cell (1-indexed) and `WheelModifiers { shift, ctrl, alt, fine }`
   (`fine` resolved from `cfg.fine_modifier`).
4. `WheelAction::route(modes, notches, cell, mods, &cfg.wheel)`:

| `WheelAction` | Effect |
| --- | --- |
| `ScrollViewport(lines)` | `handle.scroll(&mut coalescer, lines)` |
| `WriteToPty(b)` | `handle.write(&mut pty, &b)` |
| `Noop` | — |

### `crates/ozma_terminal/src/hyperlink.rs`

- `link_modifier_held(mods) -> bool` — Cmd (meta) on macOS, Ctrl elsewhere.
  Reuses the crate's `current_terminal_modifiers` (promoted to `pub(crate)`).
- `try_open_uri(uri)` — `schema::is_allowed` then `open::that_detached`; drop
  disallowed with a debug log, warn on opener error.
- `hyperlink_hover_cursor` system — single full-screen grid, no tmux/CEF. Reads
  window cursor + keys + `TerminalGrid`, updates `HyperlinkHoverState`
  (`entity`, `hyperlink_id`, `modifier_held`) so the renderer's hover-underline
  works in Ozma mode, and sets `PrimaryWindow` `CursorIcon`: `Pointer` over a
  link with the modifier held, `Text` over the grid, else `Default`. The
  idempotent-write pattern (only mutate `CursorIcon` on change) is preserved.

### Binary changes (`src/`)

- `src/main.rs` — unchanged; `OzmaTerminalPlugin` already added pulls in
  `OzmaMousePlugin`. Mouse works at defaults with no further wiring.
- `src/ozma_input.rs` — add `.before(OzmaTerminalMouseSet)` to the existing
  `InputDisabled` maintainer so mouse is gated identically to keyboard.
- `src/input/shortcuts.rs` (or sibling) — a `Startup` system inserts
  `OzmaMouseConfig` derived from `ozmux_configs` `[mouse]`, mirroring the
  existing `TerminalInputBindings` derivation.
- `src/input/hyperlink.rs` — delete `scheme_of` / `is_allowed` / `should_open_at`;
  delegate to `schema::`.

## Data Flow

```
winit → Bevy MouseButtonInput / MouseWheel / CursorMoved
  → OzmaTerminalMouseSet (skips when InputDisabled / unfocused)
    → hit-test cursor → cell (ComputedNode + UiGlobalTransform + CellMetrics)
    → buttons: ClickTracker + DragGesture → ButtonEvent
         → ButtonAction::route(handle.current_modes(), …)
              → WriteToPty        → handle.write(pty)            [app gets SGR/X10]
              → Start/Update/Clear → handle.selection_*           [local highlight]
              → (release)          → clipboard.write(sel_to_string) [copy-on-release]
    → wheel: accumulate notches → WheelAction::route(…)
              → ScrollViewport → handle.scroll(coalescer)         [scrollback]
              → WriteToPty     → handle.write(pty)                [app wheel / alt-screen]
    → hyperlink: Cmd/Ctrl + link → try_open_uri; hover → CursorIcon + HoverState
```

### Behavior table

| Input | `MOUSE_MODE` off (or Shift held) | `MOUSE_MODE` on, no Shift |
| --- | --- | --- |
| Left click | clear selection | forward press report |
| Left drag | local selection, copy on release | forward motion reports |
| Double / triple click | word / line select + copy | forward (router decides) |
| Alt+drag | block selection | block selection (local, Alt is local) |
| Wheel (primary screen) | scroll viewport | forward wheel report |
| Wheel (alt-screen) | arrow translation | forward wheel report |
| Cmd/Ctrl+click on link | open URL | open URL (link check precedes routing) |

> The two wheel rows actually gate on `MOUSE_REPORT_CLICK | MOUSE_DRAG |
> MOUSE_MOTION` (the set `WheelAction::route` checks), not the aggregate
> `MOUSE_MODE` the button rows use; the column headers are a simplification.

## Error Handling

- No window / no cursor position / no enabled terminal → drain events, reset
  in-progress `DragGesture`, keep committed selection.
- Window unfocused or `InputDisabled` present → same drain + reset.
- `handle.write` error → `tracing::warn!` (as the key path does).
- Protocol burst is capped inside `route()` (`max_protocol_events_per_frame`).
- Disallowed / malformed link scheme → dropped with a debug log.

## Testing

Protocol-byte correctness and routing decisions are **already** covered by the
engine's `buttons.rs` / `wheel.rs` / `mouse_encode.rs` tests, so glue tests
focus on what is new:

- **Pure helpers** (the bulk): `cell_at_cursor` / `to_viewport_point` /
  `to_protocol_cell`, `ClickTracker`, the `DragGesture` Armed→Started
  materialization, wheel pixel→notch accumulation, and the relocated
  `schema::is_allowed` / `scheme_of`.
- **System-level** (headless app, following the crate's `input.rs` /
  `clipboard.rs` test style): `InputDisabled` and unfocused gating drop
  everything; a left drag materializes a selection and copy-on-release writes
  `Clipboard`; double / triple-click selects word / line; Shift-bypass selects
  locally even when `MOUSE_MODE` is set; wheel with no app capture scrolls the
  viewport.

## Migration Steps

1. Relocate the scheme allowlist (`scheme_of` / `is_allowed`) into
   `ozma_tty_renderer::schema`; update `src/input/hyperlink.rs` to delegate. (No
   behavior change; the allowlist tests move with them. `should_open_at` stays
   per-host.)
2. Add `open = { workspace = true }` to `crates/ozma_terminal/Cargo.toml` (the
   workspace already pins `open = "5"`; use the `workspace = true` form like the
   root crate).
3. Add `crates/ozma_terminal/src/mouse.rs` + `mouse/buttons.rs` + `mouse/wheel.rs`
   (config, set, gesture, hit-test, systems, apply).
4. Add `crates/ozma_terminal/src/hyperlink.rs` (hover + open).
5. Wire `OzmaMousePlugin` into `OzmaTerminalPlugin`; re-export config + set.
6. Host: `.before(OzmaTerminalMouseSet)` in `src/ozma_input.rs`; insert
   `OzmaMouseConfig` from `ozmux_configs`.
7. `cargo test`, `cargo clippy --workspace`, `cargo fmt`; manual smoke (vim
   mouse, drag-select + paste, wheel scrollback, Cmd-click a link).

## Decisions & Rationale

- **Glue in the crate, not `src/`** — matches the self-contained direction of
  the keyboard work; the crate can drive a mouse on its own.
- **Flat `mouse.rs` first** — matches the keyboard sibling `input.rs`; split into
  `mouse/buttons.rs` + `mouse/wheel.rs` only if it grows past ~350 lines.
- **Allowlist (only) in renderer `schema`** — single source of truth for
  security-sensitive scheme filtering, shared by both hosts. `should_open_at` is
  *not* moved: it depends on `ozma_tty_engine` mouse enums and would create a
  `renderer → engine` dependency; each host keeps the trivial click-gate and
  calls `grid.hyperlink_at` directly.
- **Copy-on-release to system clipboard** — matches the repo's tmux VT
  selection convention and macOS norms.
- **Poll the window cursor (not `CursorMoved` deltas) for drag** — matches the
  tmux arbiter; simpler than reconstructing position from motion events.

## Open Questions

- Should `ClickTracker` (and `cell_at_*`) be extracted to a shared location for
  both the tmux path and the crate, or is the small duplication acceptable?
  (Current plan: duplicate; the tmux path is otherwise structurally different.)
- ~~Default `fine_modifier`~~ **Resolved:** `FineModifier` is a **crate-local**
  enum (mirroring `ozmux_configs::mouse::FineModifier` but without the
  dependency, per the self-contained goal); the host maps its config value
  across the boundary. Default `Shift` (the engine's wheel tests treat
  Shift-as-fine, `wheel.rs:457`).
