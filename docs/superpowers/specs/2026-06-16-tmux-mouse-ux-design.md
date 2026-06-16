# tmux Backend Native Mouse UX — Design

**Date:** 2026-06-16
**Status:** Draft (pre-implementation)
**Branch:** `mouse-support`

## Summary

Add native multiplexer mouse UX under the tmux control-mode backend. This is
**not** about forwarding mouse events to applications running inside panes
(no SGR/X10 passthrough); it is about ozmux interpreting the mouse itself to
drive tmux, the way `tmux` with `set -g mouse on` feels in a normal terminal.

Four behaviors:

1. **Drag a pane divider to resize** adjacent panes (NEW).
2. **Click-drag to select text** — auto-enter tmux copy-mode, select, copy to
   the clipboard on release (today's copy-mode drag-select only runs when the
   pane is *already* in copy-mode).
3. **Double-click selects a word, triple-click selects a line** (auto
   copy-mode), then copy.
4. **Keep existing behaviors consistent** — click-to-focus-pane,
   click-tab-to-switch-window, wheel→copy-mode — with clean gesture
   disambiguation (a press that becomes a drag/resize must not also misfire
   focus or other handlers).

## Context — current state

The backend was migrated to tmux control mode (`tmux -CC`). Each tmux pane is
projected to a Bevy entity carrying:

- `TmuxPane { id: PaneId, dims: CellDims { width, height, xoff, yoff } }`
  (cells) — `crates/tmux_session/src/components.rs:34`.
- A detached, PTY-less `TerminalHandle` (alacritty VT emulator) fed from the
  tmux `%output` stream — `src/tmux_render.rs` `route_tmux_output`.

`layout_tmux_panes` (`src/tmux_render.rs:201`) converts each pane's cell rect to
an absolute Bevy UI node rect every frame; it does **not** inflate panes over
the inter-pane gap. tmux reserves a 1-cell gap between adjacent pane rects where
it would draw its border line; the window clear color shows through that gap
(`src/tmux_render.rs:36`).

What already works under the tmux backend:

- **Click a pane → focus** — `src/ui/tmux_pane_focus.rs` `focus_pane_on_click`
  gives each pane a Bevy UI `Button` (+ `FocusPolicy::Block`) and sends
  `select-pane -t %id` on `Interaction::Pressed`.
- **Click a window tab → switch window** — `src/ui/tmux_window_bar_input.rs`
  sends `select-window -t @id` (its own `Interaction` path).
- **Wheel → copy-mode + scroll** — `src/tmux_input.rs`.
- **Copy-mode drag-select** — `src/tmux_copy_mode.rs`, but **only when the pane
  is already in copy-mode**. It targets the `ActivePane` singleton, issues
  `send-keys -X begin-selection`, moves via relative `send-keys -X -N <n>
  cursor-<dir>` (computed from pixel delta — tmux has no absolute copy-cursor
  primitive), and on release `copy-selection` + `show-buffer` → clipboard.

What is missing:

- No drag-to-resize. `resize-pane` is only bound to keys; no
  `resize_pane_command` builder exists beside `select_pane_command` /
  `select_window_command` (`crates/tmux_session/src/enumerate.rs:150`).
- No drag-select / multi-click outside copy-mode.

## External validation (Codex, tmux 3.6b manpage)

A read-only Codex pass verified the premises against the code and the local
tmux manpage. Verdict: **Approach 1 is feasible, but only sound if the new
arbiter becomes the single authority over pane-body left-button gestures.**
Concrete confirmations and the refinements they force are folded into the
design below:

- `InputPhase::Dispatch` systems are **unordered** unless explicitly chained
  (`src/input.rs:45,64`); `focus_pane_on_click`, wheel forwarding, copy-drag,
  and window-bar clicks all run there. → the arbiter must replace/disable the
  competing left-button readers, not run beside them.
- `resize-pane -t %id -x/-y` (absolute) and `-L/R/U/D <n>` (relative) both
  exist (manpage). → prefer **absolute `-x/-y`** for idempotency under delayed
  `%layout-change` (relative `-L/R/U/D` as a nested-layout fallback), and
  **throttle by gating on the confirmed layout** (one in-flight resize per drag,
  measured against `TmuxPane.dims`), not just a per-frame cap. See "Resize
  dispatch".
- `begin-selection` / `copy-selection` / `select-word` / `select-line` are
  valid copy-mode commands (manpage). Targeted `send-keys -X -t %id …` is
  already exercised in tests (`src/tmux_copy_mode.rs:1360`). → target the pane
  **under the cursor by `%id`**, never `ActivePane` (which updates
  asynchronously from `%window-pane-changed`).

## Goals / Non-goals

**Goals**

- Single gesture arbiter owns all tmux pane-body + divider left-button
  gestures; consistent disambiguation.
- Drag-to-resize via `resize-pane`, throttled.
- Drag-select / double / triple-click auto-enter copy-mode (server-side),
  pane-targeted, copy to clipboard.
- No regression to wheel→copy-mode or window-tab switching.

**Non-goals**

- Mouse passthrough to in-pane apps (SGR/X10 reporting). Out of scope.
- Host-side local selection rendering (the rejected Approach 3 — loses
  scrollback selection because the detached handle only holds what `%output`
  sent).
- Right-button / middle-button behaviors, context menus.

## Architecture

New module `src/tmux_mouse.rs` exposing `OzmuxTmuxMousePlugin`. It owns a single
left-button gesture state machine for tmux pane bodies and dividers, reading raw
`MouseButtonInput` + `Window::cursor_position()` (+ accumulated motion).

**Single-authority changes (required for soundness):**

- Remove `select-pane`-on-`Interaction` from `focus_pane_on_click`, and the
  per-pane `Button` / `FocusPolicy::Block` that exists solely to capture that
  click. Pane focus is re-issued by the arbiter. Two caveats before removing
  the `Button`:
  - The `FocusPolicy::Block` is **load-bearing** (its doc comment says so): it
    stops a pane click from falling through to underlying nodes (e.g. a webview
    surface). The arbiter's raw hit-test must replicate that — a divider grab or
    pane-body gesture must not also click through to a webview.
  - Confirm nothing else reads pane `Interaction` (grep before removal). The
    pane **dim** system in the same file keys off `ActivePane`, not
    `Interaction`, and is independent — it stays. Once only `sync_pane_dim`
    remains, `OzmuxTmuxPaneFocusPlugin`'s name + file comment are misleading;
    rename it (e.g. `OzmuxTmuxPaneDimPlugin`) or move dim-sync into the render
    layer.
- **Remove** the existing copy-mode `MouseButtonInput` drag-select system
  (`drag_select_in_copy_mode`, `src/tmux_copy_mode.rs`) **entirely** — do not
  keep it as a "residual" system ordered after the arbiter, which would
  re-introduce the competing-reader problem in a weaker form. The arbiter makes
  the same copy-mode command-helper + clipboard-bridge calls instead. The
  copy-mode rendering / snapshot infrastructure is reused unchanged.

**Stays separate:** wheel→copy-mode (`tmux_input.rs`), window-tab clicks
(`tmux_window_bar_input.rs`) — neither reads the left button, so no
disambiguation conflict. The arbiter is the sole left-button reader in
`InputPhase::Dispatch`.

### Gesture state machine

```
Idle
 └─ press → hit-test target (divider first, else pane-body)
      → Pressed { target, pane_id, cell, click_count }   // click_count from time+drift config
 ├─ motion past drag threshold:
 │    target = divider   → Dragging(Resize { border, primary_pane, axis })
 │    target = pane-body → Dragging(Select { pane_id, anchor_cell })  // ensure copy-mode entered
 └─ release:
      from Pressed (no drag):
        click_count == 1 → focus only (already sent select-pane on press)
        click_count == 2 → word select  (auto copy-mode, position, select-word, copy)
        click_count == 3 → line select  (auto copy-mode, select-line, copy)
      from Dragging(Select)  → copy-selection + show-buffer → clipboard (then per-config stay/exit)
      from Dragging(Resize)  → nothing extra (resizes streamed live)
 → Idle
```

- **Pane-body press** immediately sends `select-pane -t %id` (the pane under the
  cursor); a later drag/multi-click then operates on that same pane. Targeting
  copy-mode commands by explicit `%id` removes any dependency on the async
  `ActivePane` update.
- **Divider press** never focuses.

### Divider derivation & hit-test

tmux already parses the window's split **tree** (`WindowLayout` / `Cell` with
`SplitDir::LeftRight` | `TopBottom`) in the session layer
(`crates/tmux_control_parser/src/layout.rs`); it is currently **flattened** into
a flat `Vec<PaneGeom>` at `crates/tmux_session/src/events.rs:79` before reaching
the ECS, which is why the ECS sees only leaf rects. Reconstructing dividers from
leaf-rect adjacency is O(n²) and ambiguous at T-junctions in nested splits.
Instead, **preserve the split structure**:

- Carry each pane's immediate split parent down through projection — e.g. a
  `TmuxSplitParent { dir, sibling }` component (or equivalent metadata on
  `TmuxPane`) derived from the parsed tree, plumbed through `TmuxLayoutChanged`
  (which today carries id + dims only).
- A divider is then simply the gap between adjacent children of a `Split` node;
  the split node's orientation gives the `axis` and the owning child gives the
  `primary_pane` directly — no geometric grouping, no T-junction heuristics.
- **Compute the divider set on demand** inside the arbiter (on press, and while
  a resize drag is active) from the projected split-parent data. Dividers are
  consulted only on press/drag, never per frame, so there is no per-frame
  hit-test to cache — a separate change-gated `DividerLayout` resource is **not**
  needed and is dropped (avoids a class of staleness bugs where the resource and
  `TmuxPane` rects disagree for a frame after `%layout-change`).
- **Hit-test:** pointer within ±`grab_tolerance_px` of a divider gap and within
  the shared span → grab that border. `primary_pane` is the child whose edge the
  split owns; `axis` is the split orientation.

*Future affordance (out of scope for v1):* dividers could instead be thin
transparent UI node entities carrying `Pickable` + a `Divider` component, which
would make hover show a `ColResize`/`RowResize` system cursor (the codebase
already sets hover cursors this way in `tmux_window_bar_input.rs`). Left for
later — v1 uses on-demand geometric hit-testing because `Pointer<Drag>` carries
only screen-pixel `delta`, not the absolute cell under the cursor.

### Resize dispatch

- New builder `resize_pane_command(id, …)` in
  `crates/tmux_session/src/enumerate.rs`. Two forms are viable; the design uses
  **absolute** as the primary path:
  - **Absolute (preferred):** `resize-pane -t %primary -x <new_width>` /
    `-y <new_height>`, where `new_width = pointer_col − primary.xoff`. Absolute
    sizing is **idempotent** under a delayed `%layout-change`, so re-issuing it
    cannot accumulate drift.
  - **Relative (fallback):** `resize-pane -t %primary -L|-R|-U|-D <n>`, for
    nested layouts where the absolute size of one pane does not cleanly move the
    grabbed edge. Choose per the split parent; settle empirically in the
    real-tmux test.
- **Throttle = one in-flight resize per drag, anchored to confirmed geometry.**
  Compute the target from the pointer cell against the **current
  `TmuxPane.dims`** (tmux's last *acknowledged* layout), not the last *sent*
  command. Emit at most one `resize-pane` and then **wait for its
  `%layout-change`** (which refreshes `TmuxPane.dims`) before computing the next
  delta. This — not merely a per-frame cap — is what breaks the documented
  `%layout-change` resize-feedback flood (cf. iTerm2 #9801). A per-frame command
  cap (`max_resize_commands_per_frame`, see below) is kept as a secondary
  backstop.

### Selection dispatch (server-side, pane-targeted)

- **Auto-enter:** `copy-mode -t %id`, then insert a local `CopyModeState` on
  that pane entity (reusing the existing copy-mode machinery), so the snapshot /
  render path engages for that pane.
- **Positioning:** copy-mode has no absolute **column** primitive, but it *does*
  have absolute **row** positioning. Position in two axes:
  - **Row (absolute):** `send-keys -X -t %id goto-line <line>` against the
    target grid line. `goto-line` is idempotent w.r.t. the current row, so
    re-issuing it on a fresh snapshot self-corrects — the y-axis never
    accumulates drift. (The visible-row ↔ absolute-line mapping already exists at
    `crates/tmux_session/src/enumerate.rs:265`; invert it carefully and test
    across scroll regions.)
  - **Column (relative):** `start-of-line` + `cursor-right -N <col>`, or relative
    `cursor-left/right` deltas from the snapshot — the existing helper,
    generalized to an explicit pane id. Maintain an optimistic
    `commanded_cursor` in `TmuxMouseGesture` (updated as deltas are sent) and
    reconcile it when a fresh `CopyModeSnapshot` arrives, so fast drags don't
    re-issue deltas from a stale readback.
  - Because the first snapshot may lag a frame after entry, the machine holds a
    **`PendingPosition`** sub-state: defer `begin-selection` / `select-word` /
    `select-line` until the first `CopyModeSnapshot` for that pane arrives (with
    a bounded timeout — see edge cases). This residual (column-only) drift is
    inherited from today's drag-select (documented at
    `src/tmux_copy_mode.rs:498`).
  - **Caveat:** `select-word` has a known off-by-one (it can grab the *previous*
    word when the cursor lands on a word's first character, tmux #1820) — verify
    against real tmux and nudge positioning if needed.
- **Drag:** at the anchor cell `send-keys -X -t %id begin-selection`; move the
  copy cursor on drag; on release `copy-selection` + `show-buffer` → clipboard.
- **Double-click:** position to the clicked cell → `send-keys -X -t %id
  select-word` → copy. **Triple-click:** `send-keys -X -t %id select-line` →
  copy.

## Data flow

```
Bevy MouseButtonInput / cursor_position / MouseMotion
        │  (InputPhase::Dispatch, arbiter ordered first)
        ▼
  tmux_mouse arbiter ── hit-test ──► split-parent metadata (on-demand)
        │                                  │
        │  pane-body                       │ divider
        ▼                                  ▼
  select-pane / copy-mode -t %id     resize_pane_command(id,dir,n)
  send-keys -X -t %id begin/cursor/   (throttled, relative)
   select-word/select-line/copy-selection
  show-buffer → clipboard
        │
        ▼
  TmuxConnection control socket (single FIFO, ordered)
```

## Components / resources / helpers

- `OzmuxTmuxMousePlugin` (registers the arbiter).
- `TmuxMouseGesture` resource — state machine state (incl. `PendingPosition`
  and the optimistic `commanded_cursor`).
- `TmuxSplitParent { dir, sibling }` component (or equivalent) — split-parent
  metadata projected from the parsed `WindowLayout` tree; dividers are computed
  on demand from it (no `DividerLayout` resource).
- New tmux command builders in `crates/tmux_session/src/enumerate.rs`, matching
  the existing `select_pane_command` pattern:
  - `resize_pane_command(id, …)` — absolute `-x/-y` and relative `-L/R/U/D`.
  - `copy_mode_command(id)` → `copy-mode -t %id`.
  - `send_copy_command(id, action)` → `send-keys -X -t %id <action>`
    (`begin-selection`, `copy-selection`, `select-word`, `select-line`,
    `goto-line`, `cursor-*`).
- Helper reuse needs a visibility change: `cursor_deltas`, `cell_at_pane`,
  `phys_to_pane_local` are **private** in `src/tmux_copy_mode.rs` (`:426/:458/:476`);
  promote to `pub(crate)` (or relocate to a shared module) so the arbiter can
  call them.
- Reused config: `MouseConfig.double_click_timeout_ms`, `click_drift_px`.
- New config knobs: `drag_threshold_px`, `divider_grab_tolerance_px`,
  `max_resize_commands_per_frame`. **Do not** overload
  `max_protocol_events_per_frame` — that field is documented as a PTY
  mouse-protocol byte cap (`crates/configs/src/mouse.rs:41`); the tmux wheel path
  uses its own hard-coded `MAX_NOTCHES_PER_FRAME = 10` (`src/tmux_input.rs:419`),
  and resize warrants its own dedicated knob.

## Error handling / edge cases

- **No live tmux client** → no-op (mirror `focus_pane_on_click`'s guard).
- **Single pane / zoomed pane** → no split parents; no divider gestures.
- **Click-through to webviews** → the arbiter's hit-test must consume the press
  for pane bodies/dividers exactly where the removed `FocusPolicy::Block` did,
  so a gesture never also reaches an underlying webview surface.
- **Snapshot never arrives** for an auto-entered copy-mode pane → bounded
  `PendingPosition` timeout, then abort the gesture (no partial selection
  left), leaving copy-mode as the user can dismiss it.
- **Drag crosses pane boundary** → selection stays bound to the origin pane's
  `%id`; cells clamp to that pane (as `cell_at_pane` already clamps).
- **Command flooding** during fast resize/drag → per-frame cap + cell-delta
  gating.

## Testing strategy

- **Pure unit tests:** divider derivation from the split-parent tree (adjacent,
  nested, single-pane; verify no T-junction ambiguity), hit-test classification
  (divider vs body vs miss), click-count/drift logic, resize target computation
  (absolute width/height from pointer cell), `resize_pane_command` /
  `copy_mode_command` / `send_copy_command` formatting, gesture state
  transitions (incl. `PendingPosition`).
- **App-level (Bevy) tests**, following existing `tmux_copy_mode.rs` /
  `tmux_render.rs` patterns: press→release click emits `select-pane`;
  press→drag emits `begin-selection` + cursor positioning + `copy-selection`;
  double-click emits `select-word` + copy; divider drag emits **one** throttled
  `resize-pane` that does not re-fire until a new layout is observed.
- **Real-tmux integration** (`crates/tmux_session/tests/real_tmux_*` pattern),
  required for the two Medium-confidence risks: (a) `resize-pane` →
  `%layout-change` settle / no feedback flood / absolute-vs-relative behavior in
  a nested layout; (b) `select-word` first-character positioning quirk.

## Implementation phasing

Each phase is independently shippable:

1. **Arbiter skeleton + fold pane focus.** Move `select-pane` from
   `focus_pane_on_click` into the arbiter; remove the per-pane focus `Button`
   (replicating its `Block` semantics in the hit-test); rename the now
   dim-only plugin. Behavior identical (click → focus); establishes single
   authority.
2. **Split-parent projection + drag-resize.** Plumb `TmuxSplitParent` from the
   parsed `WindowLayout` tree through projection; on-demand divider hit-test;
   `resize_pane_command` (absolute primary, relative fallback); confirmed-layout
   throttle.
3. **Auto-enter copy-mode drag-select.** Promote the private copy-mode helpers
   to `pub(crate)`; arbiter drives pane-targeted auto-entry + `goto-line`/column
   positioning + `begin-selection`/`copy-selection`; **delete**
   `drag_select_in_copy_mode` entirely.
4. **Double / triple-click word/line select** (`select-word` / `select-line`).

## Open risks

- **Copy-cursor column drift** (snapshot lag). Reduced to the **column axis
  only** now that the row uses absolute `goto-line`; mitigated by the
  `PendingPosition` defer + optimistic `commanded_cursor` reconciliation.
  Inherent to tmux's lack of an absolute *column* primitive.
- **`%layout-change` resize feedback loop.** Real (corroborated by iTerm2
  #9801). Mitigated by absolute `-x/-y` idempotency + one-in-flight-resize
  gated on observing the prior `%layout-change`. Must be exercised by the
  real-tmux test; revisit if a flood still appears on large nested layouts.
- **`select-word` first-character quirk** (tmux #1820) — may need a
  positioning nudge; settle in the real-tmux test.
- **Nested-layout resize feel.** Absolute `-x/-y` on the chosen `primary_pane`
  should move the grabbed edge toward the cursor; complex nested trees may need
  the relative `-L/R/U/D` fallback — acceptable for v1, revisit if reported.
