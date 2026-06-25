# Pane Black-Screen Paint Recovery — Design

- **Date:** 2026-06-26
- **Status:** Draft (post-spec-review)
- **Topic:** Panes frequently render fully black after a layout change (pane
  split, window resize, copy-mode entry) and only recover on the next
  unrelated interaction.

## 1. Problem

After a layout-changing event — splitting a pane, resizing the window, or
entering copy mode — a tmux pane frequently renders **completely black**. The
black recovers when the user does *something* (key input, mouse move, another
resize, window switch), but the same operations frequently re-trigger it
("recovers on interaction, but recurs frequently", user-confirmed).

This is not a one-frame flash and not a permanent wedge: the grid is left
unpainted and stays black until the next event happens to drive a fresh paint.

## 2. Root Cause

### 2.1 How a tmux pane is painted

tmux `-CC` does not replay a pane's existing screen on attach; it only streams
new `%output`. So ozmux seeds each pane's display mirror from tmux's
authoritative grid via `capture-pane`:

```
request_pane_captures (Added<TmuxPane>)            crates/tmux_session/src/plugin.rs:121
recapture_settled_panes (size settle, one-shot)    crates/tmux_session/src/plugin.rs:181
   → tmux capture-pane + cursor query
   → apply_reply(Capture arm) → emits PaneOutput
   → route_tmux_output: handle.advance + flush_emit src/tmux/render.rs:165
   → FrameSnapshot → apply_snapshot → TerminalGrid  crates/ozma_tty_renderer/src/grid.rs:19
```

### 2.2 The colour mechanism (corrected)

A pane shows fully black when its `TerminalGrid` is **structurally empty /
unpainted** — `grid.cells` empty or shorter than `grid.rows` — combined with
`default_bg = [0,0,0]`:

- `TerminalGrid::default()` is `0×0` with `default_bg = [0,0,0]`
  (`crates/ozma_tty_renderer/src/schema/grid.rs:7`, `:46`).
- The material's `bg_padding_color` is derived from `grid.default_bg`
  (`crates/ozma_tty_renderer/src/material.rs:606`).
- `TerminalParams::new` clamps `grid_size` to at least `1×1`
  (`crates/ozma_tty_renderer/src/material.rs:370`:
  `grid_size: UVec2::new(cols.max(1), rows.max(1))`). **Therefore the shader's
  `grid_size == 0 → fallback` branch
  (`crates/ozma_tty_renderer/src/shaders/terminal_ui_material.wgsl:128`) is only
  a narrow default-material / first-frame fallback, NOT the normal
  unpainted-grid path.** The *persistent* black is a ≥1×1 grid whose cells are
  dummy / default — each cell's background unpacks to black/transparent and is
  painted with the black `bg_padding_color`.

This corrects the earlier "grid_size==0 fallback" framing. The signal is
**structural**, but note which clause is load-bearing: in the common cases the
grid is left at `0×0` — a fresh handle is born at the pane dims so
`layout_tmux_panes` never writes new dims (`src/tmux/render.rs:134`, `:461`),
and `apply_delta` never sets `rows` (`crates/ozma_tty_renderer/src/grid.rs:44`)
— where `cells.len() != rows` is `0 != 0` → **false**. The detector that
actually fires for the common case is **`(grid.cols, grid.rows)` disagreeing
with the handle geometry**; `cells.len() != rows` only catches the
dims-already-written-but-cells-empty variant. Both clauses are required. The
signal is neither `grid_size == 0` nor `last_seq == 0`.

### 2.3 The four drop-and-no-retry holes

The grid stays empty because the seed→route→paint pipeline can lose the
authoritative `capture-pane` bytes (or the resulting snapshot) and then never
reliably retries:

| # | Hole | Evidence |
|---|------|----------|
| 1 | `route_tmux_output` **discards** a pane's `PaneOutput` bytes when the pane entity or its `TerminalHandle` is not yet present (`continue`, not buffered). | `src/tmux/render.rs:180`, `src/tmux/render.rs:184` |
| 2 | `apply_snapshot` **silently drops** the `FrameSnapshot` when `TerminalGrid` is absent (early `return`). | `crates/ozma_tty_renderer/src/grid.rs:19` |
| 3 | `recapture_settled_panes` is **one-shot per size** (`done` flag, re-armed only on a dims change). If its capture is lost, no retry until dims change again. | `crates/tmux_session/src/plugin.rs:188`, `:200`, `:204` |
| 4 | CopyMode capture replies have **no resend backstop**. The copy-STATE query resends every `STALE_STATE_RESEND_UPDATES` frames, but the capture query has no equivalent. Worse: `last_scroll` is recorded at **send** time (`copy_mode.rs:265`), so a lost capture at the same scroll position **suppresses** any future recapture. | `src/tmux/copy_mode.rs:142`, `:145`, `:263`, `:265` |

### 2.4 Aggravators surfaced by the Codex review

- **A delta advances `last_seq` while leaving the grid empty.** `apply_delta`
  does not set `cols`/`rows`/`default_bg` or allocate rows; dirty rows are
  ignored when `cells.len()==0` (`crates/ozma_tty_renderer/src/grid.rs:61`:
  `if row_idx < grid.cells.len()`). So a *lost snapshot followed by a delta*
  leaves `last_seq > 0` yet the grid structurally empty. **This is why
  `last_seq == 0` is an invalid "never painted" sentinel** — and why the rescue
  condition must be structural (`cells.len() != rows`).
- **`FrameDelta` carries no `default_bg`** (`crates/ozma_tty_renderer/src/schema/frame.rs`).
  After a lost snapshot, later deltas cannot restore a non-black background.
- The "chained systems never flush commands before `route_tmux_output`" theory
  is **not** the primary race: this repo relies on auto-inserted `ApplyDeferred`
  between ordered systems (see the load-bearing NOTE at
  `src/tmux/copy_mode.rs:44`). The real race is **replies/messages arriving
  before projection/attachment exists, or a snapshot triggered before
  `TerminalGrid` exists** — i.e. holes 1 and 2.

### 2.5 Non-causes (ruled out)

- **Material / bind-group failure** is not the main cause: both storage buffers
  are seeded with a dummy element to avoid a zero-size bind failure
  (`crates/ozma_tty_renderer/src/material/state.rs:79`).
- **Webview/CEF `last_seq` ordering** (`crates/ozma_webview/src/webview/mount.rs:620`)
  explains *missing webviews*, not a fully black terminal pane. Out of scope.

## 3. Design (A-iii)

The earlier proposal "A-ii" — a perpetual reconciler keyed on `last_seq == 0` —
is rejected: `last_seq == 0` is not "never painted" (§2.4) and a perpetual loop
risks spamming tmux. The refined design pairs a **primary fix that stops losing
the bytes** with a **narrow, structural, self-stopping rescue**, plus targeted
copy-mode and defence-in-depth pieces.

### 3.1 Component 1 (PRIMARY) — buffer & replay routed output

Stop dropping the authoritative `capture-pane` bytes.

- Introduce a `PendingPaneOutput` resource: `HashMap<PaneId, Vec<u8>>` (bounded).
- In `route_tmux_output`, when the pane entity is unknown (`entity_of` miss) or
  its `TerminalHandle` is not yet present, **append the bytes to
  `PendingPaneOutput` instead of `continue`-dropping** them.
- Each frame, for every pane now ready (entity + `TerminalHandle` +
  `TerminalGrid` present), drain its pending buffer and replay through
  `handle.advance` + `flush_emit`. Because the repo inserts `ApplyDeferred`
  between ordered systems, "ready next frame" is reliable.
- **Bound** the buffer (cap total bytes and/or age per pane). On overflow, drop
  oldest and `tracing::warn!` — never a silent cap (repo rule: no silent
  truncation).

This closes holes 1 and (transitively) 2: by replaying only once both the
handle and grid exist, the snapshot is no longer emitted into a gridless entity.

### 3.2 Component 2 — structural rescue (narrow, self-stopping)

A debounced system that heals a grid that ended up structurally empty despite
the pane being attached (covers holes 2/3 and the delta-advances-last_seq
aggravator).

- **Sentinel (structural, not `last_seq`):** for a non-copy-mode tmux pane with
  both `TerminalHandle` and `TerminalGrid`, treat the grid as unpainted when
  **`(grid.cols, grid.rows)` disagrees with the handle geometry** (the
  load-bearing clause — it catches the common `0×0` grid; see §2.2) **or**
  `grid.cells.len() != grid.rows as usize` (the dims-written-but-cells-empty
  variant). Extract this as a pure, unit-testable helper
  `grid_needs_full_seed(grid, handle_geometry)` so the "blank captured pane does
  not misfire" guarantee (a real snapshot yields `cells.len() == rows`) is
  covered by a test.
- **Authoritative rescue only — NO local-repaint-first:** when the sentinel
  fires, request a fresh tmux `capture-pane` via the existing
  `request_pane_capture` (`crates/tmux_session/src/plugin.rs:134`). The earlier
  "repaint_full from the local mirror first" idea is **dropped**: in the hole-1
  case the local mirror is itself blank, so `repaint_full`
  (`crates/ozma_tty_engine/src/handle.rs:318`) would paint a blank grid, flip
  the sentinel false, and **suppress the very recapture that restores the real
  content** — actively masking the bug. tmux is the source of truth, matching
  the existing `recapture_settled_panes` approach. Rescue is bounded by:
  - **in-flight suppression** — use a **dedicated** capture-in-flight age map
    keyed by `PaneId`, *not* `EnumerationState.panes_with_cursor_pending` (the
    latter tracks only the cursor half of a paired request and misses a capture
    whose companion `CursorQuery` send failed —
    `crates/tmux_session/src/plugin.rs:140`);
  - **debounce** — at most one re-request every `N` frames (model after the
    existing `STALE_STATE_RESEND_UPDATES` copy-state cadence).
- **Ordering — avoid the resize transient:** `layout_tmux_panes` writes
  `grid.rows` immediately (`src/tmux/render.rs:466`) but its `emit_pending`
  snapshot is a deferred `commands.trigger` applied only at the next sync point
  (`crates/ozma_tty_engine/src/handle.rs:304`), so `cells.len() != rows` is
  transiently true on **every** resize. The rescue must run **before**
  `layout_tmux_panes` or **after the snapshot flush** (or skip a pane that
  already has an `emit_pending`/capture in flight) so it does not misfire on
  this transient.
- **Self-stopping:** once a capture lands and `apply_snapshot` repopulates
  `cells` and dims agree with the handle, the sentinel is false and the system
  goes quiet. No perpetual loop.

### 3.3 Component 3 — CopyMode capture backstop

Copy mode renders the scrolled view through a separate `CopyRenderHandle`, not
the live `TerminalHandle`, so it needs its own fix (Component 2 must **skip**
panes with `CopyModeState`).

- Add a **capture-in-flight age map** mirroring `STALE_STATE_RESEND_UPDATES` so
  a lost copy capture is resent.
- **Fix the `last_scroll` suppression bug:** record `last_scroll` only after the
  capture **reply** is applied (or track an independent "captured at scroll"
  value), so a lost same-scroll capture is no longer permanently suppressed
  (`src/tmux/copy_mode.rs:263`–`:265`).
- Component 2's normal-pane recapture is gated `Without<CopyModeState>` (or an
  equivalent run condition) so the two paths never fight over the grid.

### 3.4 Component 4 (B — insurance) — non-black default background

Defence-in-depth so a momentarily-unpainted pane is never alarming pure black,
and so the "delta carries no `default_bg`" gap (§2.4) is mitigated:

- Map an unset / `[0,0,0]` background to the configured **theme terminal
  background** in the material `bg_padding_color` computation
  (`crates/ozma_tty_renderer/src/material.rs:606`) — **not** at `TerminalGrid`
  creation time. `apply_snapshot` overwrites `grid.default_bg` from
  `snap.default_bg` (`crates/ozma_tty_renderer/src/grid.rs:36`), which is itself
  `[0,0,0]` whenever OSC 11 is unset
  (`crates/ozma_tty_renderer/src/schema/frame.rs`), so a creation-time init is
  undone by the first real snapshot. The material-fallback form is the robust
  one and also covers the "delta carries no `default_bg`" gap (§2.4).
- This does **not** fix the root cause; it converts a pure-black flash into a
  neutral terminal-background colour while Components 1–3 do the real work.

## 4. Data Flow (after the fix)

```
capture-pane reply ─► PaneOutput
   └─ route_tmux_output
        ├─ entity+handle+grid ready ─► advance + flush_emit ─► snapshot ─► grid painted
        └─ not ready ─────────────► PendingPaneOutput[pane]  (buffered, bounded)
                                         └─ next frame, when ready ─► replay ─► painted

structural-rescue system (debounced, Without<CopyModeState>)
   └─ grid_needs_full_seed(grid, handle_geometry)  [dims-vs-handle OR cells.len()!=rows]
        ─► capture-pane (authoritative, in-flight suppressed, ordered to skip the
           resize transient) ─► … ─► painted ─► sentinel false ─► quiet

copy-mode capture (With<CopyModeState>)
   └─ capture-in-flight age ≥ threshold ─► resend; last_scroll recorded on reply
```

## 5. Error Handling & Edge Cases

- **Pane that never attaches / dies before ready:** `PendingPaneOutput` is
  bounded and aged out with a warning; no unbounded growth.
- **Legitimately empty/cleared pane:** an empty screen still produces a
  `capture-pane` reply whose snapshot sets `cells.len() == rows` with blank
  cells, so the sentinel `cells.len() != rows` is false — the rescue does not
  misfire on a genuinely blank pane.
- **Rapid resizes / window drag:** existing `RECAPTURE_SETTLE_FRAMES` debounce
  plus Component 2's debounce + in-flight suppression bound the request rate.
- **Copy-mode enter on an already-empty grid:** Component 3's resend plus the
  `last_scroll`-on-reply fix guarantee eventual paint without relying on a
  user scroll.
- **`capture-pane` is read-only and idempotent** (the seed prepends
  clear-screen), so re-requesting is always safe — already proven by
  `recapture_settled_panes`.

## 6. Testing Strategy

Favour pure decision/helpers testable without GPU/PTY/`App` (repo idiom — see
`.claude/rules/rust.md` "System composition"):

- **Output buffer:** bytes stashed when handle absent; replayed on attach;
  bounded + warns on overflow.
- **Rescue sentinel:** the pure helper `grid_needs_full_seed(grid,
  handle_geometry)` — fires on dims-vs-handle mismatch (the `0×0` case) and on
  `cells.len() != rows`; does **not** misfire on a genuinely blank captured pane
  (snapshot `rows_data.len() == rows` ⇒ `cells.len() == rows`); does **not**
  misfire on the resize transient (dims written before the deferred snapshot
  flush — covered by the §3.2 ordering constraint); debounce honoured; in-flight
  suppression prevents duplicate requests; system goes quiet once consistent.
- **CopyMode:** capture resend after the age threshold; `last_scroll` no longer
  suppresses recapture after a lost same-scroll capture.
- **Regression:** snapshot-then-delta stays painted; a dropped snapshot is
  healed within `N` frames; `Without<CopyModeState>` gating keeps the two
  capture paths from colliding.

## 7. Out of Scope (YAGNI)

- The perpetual `last_seq`-based reconciler (A-ii) — rejected.
- Webview/CEF black panes (§2.5) — separate concern.
- Reworking the coalescer / emit path.

## 8. Open Questions

- Where the theme terminal background lives (config vs `src/theme.rs`) for
  Component 4, and whether to thread it through `FrameSnapshot.default_bg`
  defaults or only the material fallback.
- Concrete values for the debounce interval `N` and the `PendingPaneOutput`
  byte/age caps (tune during implementation).
- **Resolved (review):** Component 2's "local repaint first" is **dropped** — it
  masks hole-1 recovery (§3.2). Rescue is authoritative-only.
- Whether to **fold Component 2 into the existing `recapture_settled_panes` /
  `PaneRecaptureState`** (2026-06-24 spec) by adding a re-arm condition (reset
  `done = false` when `grid_needs_full_seed` is true), reusing its settle
  debounce, dims tracking, and in-flight suppression. This keeps the state
  machine in one place, but requires reading the renderer-crate `TerminalGrid`
  from `ozmux_tmux` (which stays renderer-free) — so the structural check would
  live in `src/tmux/` and feed the crate via a marker/event.
- Whether **Component 1's byte buffer can be dropped** once Component 2 is
  authoritative-only: lost bytes would be recovered by re-fetching from tmux
  rather than buffered/replayed, collapsing Components 1+2 into one mechanism.
  Trade-off: loses the exact-byte replay fast path and leans entirely on
  `capture-pane` for the lost-bytes case (valid **only** with local-first also
  removed — keeping local-first while dropping Component 1 is broken).
