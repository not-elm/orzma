# Horizontal mouse-wheel / trackpad scroll

## Problem

The terminal wheel path drops the horizontal axis. Trackpad two-finger
horizontal swipes and Shift+wheel (which macOS converts to a horizontal scroll
at the OS level) arrive as `MouseWheel` events with a non-zero `ev.x`, but every
terminal-side consumer reads only `ev.y`:

- `ozma_tty_engine::wheel::WheelDir` is `Up` / `Down` only; its doc comment
  states *"Horizontal wheels are out of scope."*
- `ozma_terminal::mouse::dispatch_mouse_wheel` aggregates only
  `wheel_delta_cells(ev.unit, ev.y, cell_h)`; `WheelAccumulator` carries a single
  residual axis.

As a result an application that understands horizontal wheel reports — e.g.
Neovim with `set mouse=a` and `nowrap` (`ScrollWheelLeft` / `ScrollWheelRight`) —
never receives them.

Webviews are already correct: `webview_wheel_delta(unit, x, y)` plumbs both axes
to CEF. Only the terminal path is missing horizontal support.

**Goal:** when the pane under the pointer has mouse reporting enabled, translate
horizontal wheel/trackpad input into SGR (or X10 fallback) horizontal wheel
reports (`cb = 66` left, `cb = 67` right) and deliver them to the application,
in both Default mode and tmux mode.

## Scope

In scope:

- Horizontal wheel reporting to **mouse-mode applications only** (any of
  `MOUSE_REPORT_CLICK` / `MOUSE_DRAG` / `MOUSE_MOTION` set), via SGR/X10
  encoding, mirroring the existing vertical mouse-protocol path.

Out of scope (decided during brainstorming — a terminal has no horizontal
scrollback, so non-mouse-mode horizontal input has no meaningful target):

- Horizontal scrollback / viewport scrolling in the normal screen.
- Alt-screen `←` / `→` arrow translation (the horizontal analog of the vertical
  `ALTERNATE_SCROLL → ↑/↓` path). Non-mouse-mode horizontal input is a no-op.
- New configuration keys (see "Configuration").
- Any change to copy-mode or the tmux `forward_wheel_to_tmux` path.

## Approach

Chosen: **a dedicated horizontal route function in the engine** (Approach A of
three considered).

- **B — generalize `WheelAction::route` with an axis parameter:** rejected.
  It changes the signature of the well-tested vertical router and all its call
  sites/tests for no behavioral gain; vertical and horizontal have genuinely
  different routing rules (vertical has scrollback + alt-screen branches;
  horizontal has neither), so merging them only adds branching.
- **C — encode SGR 66/67 in the host dispatcher, no engine change:** rejected.
  It duplicates the mouse-mode detection and SGR/X10 encoding that already live
  in `ozma_tty_engine`, splitting protocol encoding across two crates.

Approach A keeps the vertical `route` untouched (zero regression risk on the
existing path) and matches the repo's "pure decider returning effect values"
idiom.

## Design

### 1. Engine layer — `crates/ozma_tty_engine/`

**`wheel.rs`**

- Extend `WheelDir` with `Left` and `Right`. Update the enum doc comment (drop
  "Horizontal wheels are out of scope").
- `encode_wheel_report`: add `WheelDir::Left => 66`, `WheelDir::Right => 67` to
  the `cb_base` match. The rest of the function (modifier bits, motion bit
  handling, SGR-vs-X10 selection) is unchanged and shared with the vertical
  directions. `motion = false` is passed through, as for vertical wheels.
- Add `WheelAction::route_horizontal(modes, notches, mouse_cell, mods, cfg)`:
  - `notches == 0` → `Noop`.
  - Direction: `notches < 0 → Left`, `notches > 0 → Right` (the host owns the
    mapping from `ev.x` sign to this convention — see "Direction").
  - If any of `MOUSE_REPORT_CLICK | MOUSE_DRAG | MOUSE_MOTION` is set: emit
    `min(|notches|, cfg.max_protocol_events_per_frame)` concatenated reports via
    `encode_wheel_report`; return `WheelAction::WriteToPty(buf)` (or `Noop` if
    the cap is 0). This is the only branch.
  - Otherwise (no mouse mode): `Noop`. No scrollback, no alt-screen translation.

  Reuses the existing `WheelAction` enum (`WriteToPty` / `Noop`); horizontal
  never returns `ScrollViewport`.
- Factor the mouse-protocol emission shared by `route` and `route_horizontal`
  (`count = min(|notches|, cap)`, loop `encode_wheel_report`, return
  `WriteToPty`/`Noop`) into one private helper, so horizontal does not duplicate
  that block and `route`'s public signature + scrollback/alt-screen branches stay
  untouched.

The shared encoder `mouse_encode::encode_protocol_event` needs **no change** —
`cb_base` 66/67 already flow through its `+32` (X10) / SGR formatting unchanged.

### 2. Host layer — `crates/ozma_terminal/src/mouse.rs`

- `WheelAccumulator`: hold two **named** per-axis residuals — the existing
  `residual_cells` (vertical) plus a new horizontal residual — sharing one
  `last_target`. Refactor `accumulate_notches` to take the residual by
  `&mut f32` (not the whole accumulator) so both axes reuse the identical carry +
  sign-flip logic; the `delta_cells != 0.0` guard (no reset on `0.0`/`-0.0`) is
  preserved per axis. Retarget-on-entity-change zeroes both. Named fields, not a
  `Vec2`, to keep accumulation axis-semantic and tests readable.
- Reuse the `WheelAction → Vec<MouseEffect>` mapping for both axes instead of a
  separate `decide_wheel_horizontal`: `route_horizontal` returns the same
  `WheelAction` enum `decide_wheel` already maps, so factor that mapping into one
  shared helper (e.g. `effects_from_wheel_action`); the horizontal path simply
  never produces `ScrollViewport`.
- `dispatch_mouse_wheel` (single read pass, both axes):
  - Sum BOTH axes in ONE `wheel.read()` pass — the `MessageReader` drains once
    per frame, so a second `read()` would see nothing. `Line` units contribute
    `ev.x` / `ev.y` directly; `Pixel` units contribute `ev.x / cell_w` (advance)
    and `ev.y / cell_h` (line height).
  - Accumulate each axis on its own residual → independent notch counts.
  - **Restructure the early return:** the current `if raw == 0 { return; }`
    guards only the vertical count; a pure-horizontal swipe (the common case)
    has vertical `raw == 0` and would be dropped. Return early only when BOTH
    axes yield zero notches.
  - Emit the vertical + horizontal effects in ONE merged `TerminalMouseEffects`
    trigger per frame (one observer pass; report order is app-irrelevant).
  - Axes accumulate/emit **independently** (a diagonal swipe produces both); see
    "Axis handling".

`MouseEffect::Write` already routes correctly for both backends via
`on_terminal_mouse_effects`: a PTY-backed Default terminal writes to the PTY; a
PTY-less tmux pane emits `TerminalForwardInput`, which the host delivers to tmux
via `send-keys -H`.

### Modifier handling (Shift+wheel on macOS)

`build_wheel_modifiers` reads physical Shift from `ButtonInput<KeyCode>`. On
macOS the OS converts Shift+vertical-wheel into horizontal scroll (`ev.x`) while
Shift stays physically held, so a naive encoding sets the SGR Shift bit (+4):
`cb` becomes 70/71, which Neovim parses as `<S-ScrollWheelLeft/Right>` — an
unmapped-by-default event that defeats the "mouse wheel horizontal scroll" half
of the goal. The two-finger trackpad path (no Shift) is already clean.

Decision: on macOS (`cfg!(target_os = "macos")`), strip the Shift modifier from
horizontal wheel reports so an OS-converted Shift+wheel yields a plain
`<ScrollWheelLeft/Right>`. Other platforms pass modifiers through unchanged;
Ctrl/Alt are unaffected.

### 3. Data flow (all reused; no new effect/event types)

```
MouseWheel{ev.x} → dispatch_mouse_wheel
  → aggregate horizontal cells (ev.x; Pixel ÷ cell_w)
  → accumulate_notches (horizontal residual, &mut f32)
  → WheelAction::route_horizontal → effects_from_wheel_action
  → MouseEffect::Write(SGR/X10 66|67)
  → TerminalMouseEffects → on_terminal_mouse_effects
     ├ Default (PtyHandle): handle.write(pty, …)
     └ tmux pane (no PtyHandle): TerminalForwardInput → tmux `send-keys -H` (raw
       hex bytes injected into the pane, bypassing tmux's own mouse translation)
  → application reads ScrollWheelLeft/Right
```

### 4. What does NOT change

- `WheelAction::route` (vertical) — untouched.
- `src/mode/tmux/input.rs` `forward_wheel_to_tmux` / `aggregate_tmux_wheel_cells`
  — untouched. Horizontal is mouse-mode-only, and mouse-mode panes are
  `WheelOwner::CededToOzma`, owned by `dispatch_mouse_wheel`. The tmux forward
  path only handles copy-mode and alt-screen-residual, neither of which has a
  horizontal meaning. The two systems hold independent `MessageReader<MouseWheel>`
  cursors, so both observe every event regardless of which drains first.
- `src/mode/default/*` — terminal wheel in Default mode is already handled by
  `dispatch_mouse_wheel` (the always-on `OzmaMousePlugin`); Default's only wheel
  system, `forward_default_webview_wheel`, is webview-only.
- Webview wheel — already handles both axes.
- Bevy's frame-summed `AccumulatedMouseScroll` is intentionally NOT adopted over
  `MessageReader<MouseWheel>`: it collapses a frame to a single `unit`, losing the
  per-event `Line`/`Pixel` handling both wheel consumers rely on and the
  `clear()`-on-suppression semantics.

## Direction (sign convention)

Two independent mappings set the on-screen direction; BOTH are confirmed
end-to-end against a real Neovim trace during implementation:

1. **`cb` ↔ left/right.** Horizontal wheel = xterm buttons 6/7 = `cb 66/67`.
   Neovim's SGR parser maps `cb66 → ScrollWheelLeft`, `cb67 → ScrollWheelRight`
   (`neovim` `src/nvim/tui/input.c`), so the spec uses `Left=66`, `Right=67`.
   Older xterm prose has been read as assigning 6/7 the opposite way, so this is
   treated as verify-against-the-target-app, not assumed-from-prose.
2. **`ev.x` sign ↔ left/right.** The host maps the accumulated delta at a single
   call site. winit/Bevy horizontal sign is platform-dependent (macOS
   `PixelDelta` horizontal sign is historically opposite X11/Wayland), so the
   sign is confirmed empirically; the one-line mapping makes flipping trivial.

The implementation verifies the *composed* result — finger/wheel direction → the
Neovim view (`nowrap`) moving the intended way — not either mapping alone. Per
the scope decision, no invert / enable config key is added this iteration.

## Axis handling

Horizontal and vertical residuals are accumulated **independently** and each
emits notches when it crosses `cells_per_notch`. Rationale: it matches the
existing vertical behavior, keeps the accumulator simple, and the default
`cells_per_notch = 0.5` plus the per-axis sign-flip reset means small
perpendicular trackpad jitter does not cross the threshold. Axis-locking
(dominant-axis-wins per gesture) is a possible future refinement, not part of
this iteration.

Sensitivity caveat: a horizontal cell (`advance_phys`, ~8px) is ~half a vertical
cell (`line_height_phys`, ~16px). Dividing `ev.x` by the cell WIDTH under a
shared `cells_per_notch` therefore made horizontal ~2× more sensitive per pixel
of finger travel. **RESOLVED** after trackpad testing: both axes divide by
`line_height_phys` (cell height), so the per-pixel notch rate is uniform across
axes. In a mouse-mode report the pitch is a pure sensitivity threshold (each
notch = one report; the application decides columns), so a common pitch is
correct — the horizontal "cell" need not be a literal column. (NVim's default
`mousescroll=hor:6` vs `ver:3` further scales the per-report effect, but that is
NVim's config, not ours.)

## Configuration

No new keys. In a mouse mode the protocol path emits one report per notch and
intentionally does **not** multiply by `lines_per_notch` (the application
decides line counts; this matches the vertical mouse-mode path and alacritty).
The existing `cells_per_notch` accumulation threshold is reused for the
horizontal axis. `max_protocol_events_per_frame` caps horizontal bursts, as for
vertical.

## Testing

Engine (`crates/ozma_tty_engine/src/wheel.rs`, pure unit tests):

- `route_horizontal` in a mouse mode emits SGR `\x1b[<66;…M` (left) and
  `\x1b[<67;…M` (right) at the cursor cell.
- `route_horizontal` with no mouse mode → `Noop` (normal screen AND alt-screen,
  confirming no horizontal scrollback / arrow translation).
- X10 fallback bytes for 66/67 (`cb+32` = 98/99).
- Modifier bits (shift/ctrl/alt → meta) on horizontal reports.
- Multi-notch concatenation and the `max_protocol_events_per_frame` cap
  (including the 0-cap `Noop`).
- `encode_wheel_report` Left/Right encoding.

Host (`crates/ozma_terminal/src/mouse.rs`):

- The shared `effects_from_wheel_action` maps `route_horizontal`'s `WriteToPty`
  → `Write` (mouse mode) and `Noop` → `[]` (no mouse mode).
- Horizontal-axis accumulation: sub-notch carry, sign-flip reset, retarget
  reset — independent of the vertical residual.
- A `dispatch_mouse_wheel` integration test (existing app-harness style) feeding
  an `ev.x` event over a mouse-mode terminal and asserting a
  `MouseEffect::Write` carrying the 66/67 bytes; and a diagonal event asserting
  both axes emit.
- A pure-horizontal frame (vertical delta ≈ 0) still emits a horizontal `Write`
  — guards the `raw == 0` vertical early-return regression.
- On macOS, a Shift-held horizontal report carries NO SGR Shift bit (plain
  `cb 66/67`, not 70/71).
- A diagonal frame emits both axes via a single `TerminalMouseEffects` trigger.

No `ozmux_configs` test changes (no config surface change).

## Assumptions to validate in review

1. Horizontal sensitivity is matched to vertical by dividing `ev.x` by the cell
   HEIGHT (`line_height_phys`) — the same pitch the vertical axis uses — NOT the
   cell width. Trackpad testing confirmed the cell-width pitch (`advance_phys`,
   ~½ of `line_height_phys`) made horizontal ~2× too sensitive; see "Axis
   handling".
2. Independent x/y accumulation (no axis-lock) is acceptable for v1.
3. No new configuration key is desired for enabling/inverting horizontal scroll.
4. SGR `cb = 66/67` (xterm buttons 6/7) is the correct wire encoding for
   horizontal wheel, and `send-keys -H` injects the raw bytes into the pane
   verbatim (not tmux's own wheel translation) — the same delivery the existing
   vertical mouse-mode reports already use.
