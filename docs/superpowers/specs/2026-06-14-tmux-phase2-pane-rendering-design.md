# tmux migration — Phase 2: Pane rendering

Design spec — 2026-06-14
Parent spec: `docs/superpowers/specs/2026-06-14-tmux-multiplexer-migration-design.md`
(Phase 2 of the migration phasing)

## Goal

Make a tmux pane visibly render its `%output` on the GPU grid. Phase 1
projects `TmuxSession` / `TmuxWindow` / `TmuxPane` entities from control-mode
events but renders nothing. Phase 2 attaches a **PTY-less** `TerminalHandle`
plus the existing render bundle to each `TmuxPane`, routes `%output` into it,
positions panes by tmux cell geometry, and tells tmux the GUI window's cell
size via `refresh-client`.

The migration stays runnable at each phase boundary and the old multiplexer
stays in the tree (dormant) until Phase 5.

## Decisions settled during brainstorming

1. **Phase 2 is split into 2a and 2b**, each independently mergeable, matching
   the 1a/1b/1c cadence.
   - **2a** — PTY-less engine API + a single tmux pane rendering end-to-end
     (`%output` → grid).
   - **2b** — multi-pane absolute cell-dim layout + `refresh-client` window
     sizing.
2. **Immediate emit per frame, coalesced per pane.** tmux already coalesces
   output, so tmux panes carry no `Coalescer` / deadline-flush machinery. Group
   a frame's `PaneOutput` messages by `PaneId`, `advance` all of a pane's
   chunks, then call `flush_emit` ONCE per pane that frame — calling
   `flush_emit` per message would trigger multiple FrameSnapshots/Deltas for
   one entity where only the last is visible.
3. **Always auto-connect; the `auto_connect` config option is removed.** tmux
   always owns the window. The old multiplexer bootstrap surface is
   unconditionally suppressed (kept in-tree, dormant, deleted in Phase 5).
   NOTE: config uses `#[serde(deny_unknown_fields)]`, so simply deleting the
   field makes an existing `[tmux] auto_connect = …` TOML fail to parse (a hard
   load error, not silent). Either keep the key as a deprecated / ignored alias
   for one release or document the breakage; update the `raw.rs` tests that
   write `auto_connect = true`.
4. **tmux-unavailable shows the error dialog and renders no terminal** (no
   fallback to the old single-terminal bootstrap), matching parent-spec
   decision 2. The standalone single-terminal mode is gone for good.

## Architecture

### A. `ozma_tty_engine` — PTY-less entrypoint

The VT bridge is already PTY-independent at its core: `TerminalHandle::advance(&[u8])`
is public and PTY-free (`crates/ozma_tty_engine/src/handle.rs:174`). Phase 2
widens the construction/emission path:

1. **`TerminalHandle::detached(cols, rows, gate) -> Self`** — mirrors
   `TerminalBundle::spawn` (`crates/ozma_tty_engine/src/bundle.rs:50`) minus the
   PTY: builds the reply/control channels + `TermListener` internally, with no
   `PtyHandle` and no child process. Today `TerminalHandle::new` is `pub(crate)`
   (`handle.rs:118`); `detached` is the public PTY-less constructor.
2. **`flush_emit(&mut self, commands, entity)`** — a coalescer-free emit:
   collect damage from the current `Term` state, emit `FrameSnapshot` /
   `FrameDelta`, reset. Handles the first-emit bootstrap (a blank Initial
   snapshot on a freshly-spawned pane, mirroring `needs_bootstrap_emit` /
   `force_bootstrap_damage` at `handle.rs:581,588`). The existing `emit` /
   `finalize_emit` (`handle.rs:652,899`) are refactored so the `Coalescer`
   argument is no longer required on this path — this is a real refactor, not a
   thin wrapper: `emit`, `finalize_emit`, and `abort_emit_with_no_damage` all
   currently take `&mut Coalescer` and call `disarm()`; the coalescer-dependent
   bookkeeping must be extracted from the damage-collect + emit + reset core.
   This is the "immediate emit" path from decision 2.
3. **`resize_grid_only(&mut self, cols, rows)`** *(2b)* — exposes the existing
   private `resize_grid` (`handle.rs:960`): alacritty grid resize **only**, no
   `pty.resize`. NOTE: bare `resize_grid` does NOT stage damage (the public
   `resize` stages it separately via `stage_full_damage_and_arm`), so
   `resize_grid_only` must additionally collect + stage full damage for the new
   geometry to reach the renderer. Must never echo size back to tmux (tmux owns
   pane sizes; echoing would loop).
4. **`take_replies(&self) -> Vec<u8>`** — drains alacritty `PtyWrite` replies
   (DSR / DA answers) currently reachable only via the `pub(crate)`
   `drain_replies_into` (`handle.rs:644`). In 2a these replies are **drained and
   dropped**; Phase 3 (input) routes them back to tmux as pane input.

tmux panes carry **no** `Coalescer` and **no** `PtyHandle`.

### B. Crate-boundary seam

`ozmux_tmux` stays renderer-free. It keeps projecting entities and now also
**surfaces `%output` as a Bevy message** `PaneOutput { pane: PaneId, data: Vec<u8> }`.
Today `drain_tmux_events` (`crates/tmux_session/src/plugin.rs:45`) drops
`%output` because `ProjectionModel::apply_event` returns `false` for it
(`crates/tmux_session/src/model.rs:132`, by design — see the change-detection
note there).

The **binary (`src/`) owns rendering**, exactly as it does for the old surfaces
in `src/ui/terminal.rs`. This keeps "`ozmux_tmux` is the only crate that knows
tmux exists" intact and keeps `Assets<TerminalUiMaterial>` wiring in the binary.

Per-frame ordering (chained in `Update`, after `ozmux_tmux`'s drain + reconcile):

1. `reconcile_projection` (`reconcile.rs`) — spawns / despawns `TmuxPane`.
2. `attach_tmux_pane_terminal` (new, `src/`) — attaches the detached handle +
   render bundle + `Node` onto each `TmuxPane` lacking `TerminalHandle`.
3. `route_tmux_output` (new, `src/`) — reads `PaneOutput`, maps
   `PaneId` → `Entity` via the public `TmuxProjection.panes`
   (`reconcile.rs:16`), calls `advance` + `flush_emit`.

The same-frame chain works because `.chain()` inserts an `ApplyDeferred`
between systems and `commands.trigger` defers the snapshot to the next sync
point — so `attach`'s `TerminalGrid` insert lands before `route`'s emit is
observed. This requires a NEW public ordering seam (does not exist yet):
`ozmux_tmux` adds a public `TmuxProjectionSet` SystemSet wrapping its
`(drain_tmux_events, reconcile_projection).chain()` (today `reconcile_projection`
is `pub(crate)`), and the binary registers `attach`/`route` as
`.after(TmuxProjectionSet).chain()`. Without that explicit edge the invariant
is not guaranteed.

## Phase 2a — single pane renders end-to-end

- **`ozmux_tmux`:** emit `PaneOutput` from `drain_tmux_events` for each
  `ControlEvent::Output`. Structural events keep flowing into `ProjectionModel`
  as today.
- **`attach_tmux_pane_terminal` (`src/`):** watches `TmuxPane` lacking
  `TerminalHandle`; attaches
  `TerminalHandle::detached(dims.width, dims.height, gate_off)` +
  `TerminalRenderBundle::new(material)` + a full-window absolute `Node`. The
  grid is sized from the **already-projected `TmuxPane.dims`** so it matches
  tmux's pane size. (`TmuxPane` already carries `dims: CellDims` — see
  `reconcile.rs:83`.)
- **`route_tmux_output` (`src/`):** `advance` + `flush_emit` per `PaneOutput`.
- The engine's existing `TerminalHandlePlugin` systems (`drain_pty_chunks`,
  `check_deadline_flush`) query `&mut PtyHandle` / `&mut Coalescer`, which tmux
  panes lack, so Bevy's archetype filter excludes them automatically — no
  `Without<TmuxPane>` guard is needed.
- **Always auto-connect:** delete `TmuxConfig.auto_connect`
  (`crates/configs/src/tmux.rs:14`) and its `raw.rs` + `src/tmux_boot.rs`
  plumbing; boot always connects.
- **Suppress old bootstrap:** `OzmuxBootstrapPlugin` no longer seeds its
  session / pane / surface (tmux owns the window). Code stays for Phase 5.
- Single-pane only; full-window `Node`; no pixel-accurate geometry yet.

## Phase 2b — multi-pane layout + window sizing

- **Absolute cell-dim layout (`src/`):** from `TerminalCellMetricsResource`
  (`crates/ozma_tty_renderer/src/lib.rs:15`) cell pixel size, position each
  `TmuxPane` `Node`:
  `left = xoff·cell_w`, `top = yoff·cell_h`, `width = width·cell_w`,
  `height = height·cell_h`. Panes are absolute siblings under the window node —
  no Bevy flex split tree. Driven by changed `TmuxPane.dims`.
- **Grid-only resize:** on `dims` change call `resize_grid_only(cols, rows)`
  (no tmux echo).
- **`refresh-client -C <cols>x<rows>`:** GUI window px → cells → a typed command
  builder in `tmux_session` (target-id / size escaping discipline);
  `tmux_control` stays pure transport. The bare client-size form accepts BOTH
  `W,H` and `WxH` (tmux parses `%u,%u` then `%ux%u`); default to `W,H`, the
  older / most-compatible form — no version floor needed. A version floor is
  only required for the window-targeted `@ID:WxH` form, which Phase 2b does not
  use. Debounced on window resize.
- Send `refresh-client -C` only when the integer `(cols, rows)` actually
  changes, not on every pixel resize (sub-cell pixel changes cannot alter tmux
  layout).
- The "tmux emits `%layout-change` after `refresh-client`" behavior is
  **unverified** and must be confirmed by the gated real-tmux integration test
  before relying on it. 2b pane geometry is sourced ONLY from `LayoutChange`
  (`model.rs:121`); if the live test shows tmux does NOT re-emit layout after a
  control-client size change, fall back to re-querying `list-windows` after
  sizing to refresh pane rects.

## Testing strategy

- **2a:**
  - `ozma_tty_engine` unit test: `detached` + `advance` + `flush_emit` produces
    a `FrameSnapshot`.
  - Headless Bevy test: feed a canned `PaneOutput` → assert a `FrameSnapshot`
    (and `TerminalGrid` update) on the pane entity.
- **2b:**
  - Table-driven `CellDims` → `Node` rect math given fixed cell metrics.
  - `resize_grid_only` unit test (grid resized, no PTY touch).
  - `refresh-client` command-builder test (escaping, `WxH` vs `W,H`).
  - Gated real-tmux integration: resize → `%layout-change` (mirrors the existing
    `real_tmux_*` gated tests in `crates/tmux_session/tests/`).

## Deferred scope (unchanged from parent spec)

- **Input / reply routing, focus + dim, click-to-focus** → Phase 3. Replies
  (DSR / DA) are dropped in 2a; some programs that block on a DA reply may not
  fully initialize until Phase 3 wires replies to tmux input — acceptable for a
  rendering proof, revisit in Phase 3.
- **OSC 5379 inline webviews** — the webview gate stays off on tmux panes.
- **Mouse wheel / scroll, drag-resize, IME, hyperlink hover** — first cut is
  rendering only.
- **Multi-window / tab strip UI** — windows are projected; only the active
  window renders.
