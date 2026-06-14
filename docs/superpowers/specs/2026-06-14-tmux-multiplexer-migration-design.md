# Migrate the ozmux multiplexer backend to tmux

Design spec — 2026-06-14
Source issue: https://github.com/not-elm/ozmux/issues/114

## Goal

Replace ozmux's builtin ECS-native multiplexer with an architecture driven
by `tmux` control mode (`tmux -CC`). tmux becomes the sole source of truth
for sessions, windows, panes, and layout. The ozmux ECS world becomes a
deterministic projection of tmux state, rebuilt from control-mode events.

## Foundational decisions

These were settled during brainstorming and are fixed for this migration:

1. **Backend model — tmux replaces everything.** Once attached, tmux is the
   only source of truth. The native Workspace/Pane/Surface multiplexer
   (`MultiplexerCommands`, the binary split tree, per-surface PTY ownership)
   is removed. There is no non-tmux multiplexing fallback.
2. **Boot flow — auto-connect at startup.** On launch ozmux starts the tmux
   server if needed and attaches automatically. The standalone
   single-terminal bootstrap mode is **shelved** (deferred), so there is no
   pre-tmux terminal state in this migration.
3. **Attach target — auto-attach to the most-recently-used session,
   picker on demand.** If zero sessions exist, create one. If one or more
   exist, attach to the MRU session immediately; the session-selection
   popup is opened on demand via a connect/switch shortcut (it becomes a
   switcher).
4. **Keybinds — tmux owns bindings, ozmux mirrors.** tmux is authoritative.
   ozmux reads tmux's key tables (`list-keys`) on attach and keeps a synced
   copy for awareness and display. Most keys forward to the focused pane;
   ozmux intercepts only a small fixed set of GUI-level chords.

## Architecture

### Crate structure

New crate **`crates/tmux_session` (`ozmux_tmux`)** — a Bevy plugin crate
sitting above the existing library crates `tmux_control` (transport:
`TmuxClient`/`TmuxHandle`, owns the `tmux -CC` process) and
`tmux_control_parser` (parsing: `ControlEvent`, `WindowLayout` cell tree,
block assembler). It is the only place in the app that knows tmux exists;
the rest of the app continues to see ECS entities and frame events.

Responsibilities:

- Own the `TmuxClient`/`TmuxHandle` as a Bevy `Resource`; pump
  `TransportEvent`/`ClientEvent` once per frame via a non-blocking channel
  drain (the same pattern as `ozma_tty_engine`'s `drain_pty_chunks`).
- Maintain the **projection**: tmux `SessionId`/`WindowId`/`PaneId` map to
  Bevy entities, kept in sync by control events. tmux is authoritative; the
  ECS never mutates tmux state except by sending commands and applying the
  resulting echo event.
- Translate `%layout-change` into pane geometry, route `%output` into
  per-pane `TerminalHandle`s, route input back out as tmux commands.

### Removed

- `crates/multiplexer` entirely (Workspace/Pane/Surface entities,
  `MultiplexerCommands`, the binary split tree, the `layout` / `resize` /
  `direction` modules, the dangling-reference observers).
- The `src/multiplexer` module.
- The Surface-based tab layer: `src/ui/tab_bar.rs`, `src/ui/tab_input.rs`,
  and the multi-surface-per-pane model (one surface per pane only).
- Action observers that call `MultiplexerCommands` (rewired or removed).

### Kept unchanged

- `ozma_tty_engine` — the VT emulator, reused with a **small new PTY-less
  API**. `TerminalHandle::advance(&[u8])` is public and fully
  PTY-independent (`crates/ozma_tty_engine/src/handle.rs:174`), but the
  surrounding construction/emission path is PTY-coupled today:
  `TerminalHandle::new` is `pub(crate)` (`handle.rs:118`),
  `TerminalBundle::spawn` always opens a PTY (`bundle.rs:37,50`), and frame
  emission (`ingest_chunk`/`emit`) is `pub(crate)` and driven by
  `drain_pty_chunks`, which queries `PtyHandle` (`plugin.rs:40`). This
  migration adds a PTY-less entrypoint to `ozma_tty_engine`: a detached
  constructor, a public external-chunk ingest that stages damage + emits,
  and a grid-only resize (below). `TerminalBundle` is NOT reused verbatim
  for tmux panes.
- `ozma_tty_renderer` — GPU grid rendering. It consumes
  `FrameSnapshot`/`FrameDelta`, which fire regardless of the byte source.

### Source-of-truth invariant

**tmux state is the source of truth; the ECS world is a deterministic
projection of it, rebuilt from control events.** ozmux never mutates the
projection optimistically — it sends a tmux command and lets the resulting
control event drive the change. One code path serves both "user typed the
tmux prefix-split binding" and "ozmux UI requested a split."

## Connection lifecycle

A `ConnectionState` resource (`Idle` / `Connecting` / `Attached{session}` /
`Detached` / `Error`) gates which UI overlay shows and which shortcuts are
live.

**Boot (auto-connect):**

1. Resolve the tmux binary (config-overridable path). If not
   found/executable, show an **error dialog** (Bevy UI overlay) and stay
   idle; a retry/reconnect shortcut re-runs the probe.
2. Spawn `tmux -CC` via the existing `TmuxServer`/`TmuxClient` transport
   (starts the server if none is running).
3. Query `list-sessions`. Zero sessions → create one (`new-session`) and
   attach. One or more → attach to the **most-recently-used** session
   immediately. NOTE: the current `SessionInfo`
   (`crates/tmux_control/src/session.rs:7`) carries only
   `id/name/windows/attached/created` — no activity field. MRU requires
   either extending `SessionInfo`/`list-sessions` to parse
   `session_activity`, or falling back to `attached` then `created` order.

**Session picker (on demand):** a connect/switch shortcut opens a popup
listing sessions (from a fresh `list-sessions`) plus a "new session" entry.
Selecting one detaches the current attachment and attaches the chosen one.

**Detach:** a detach shortcut sends the control-mode detach. tmux emits
`%client-detached` / the client exits; ozmux tears down the projection
(despawns pane/window/session entities) and shows a **"detached" overlay**
with a reconnect hint.

**Reconnect:** a separate reconnect shortcut re-runs the attach flow
(boot steps 2–3).

**Connection loss / `%exit`:** if the transport reports `Closed` or `%exit`
unexpectedly, treat it like detach but surface an error state; the reconnect
shortcut recovers.

## State projection model

### Entities

Three projected entity kinds, each carrying its tmux id as a component:

- `TmuxSession { id: SessionId }` — the attached session (one attached at a
  time).
- `TmuxWindow { id: WindowId }` — child of the session. A tmux window owns a
  layout tree; only the **active** window renders. Windows map naturally to
  a future window/tab strip.
- `TmuxPane { id: PaneId }` — child of a window; the leaf holding a PTY-less
  `TerminalHandle` + grid render bundle and an absolute-positioned `Node`.

There is no `Surface` entity in the tmux path: `TmuxPane` is itself the
terminal-host entity (carrying the PTY-less `TerminalHandle` + render
bundle). The old multi-surface-per-pane abstraction is gone, so nothing
sits between pane and terminal.

### Event → projection (the reducer)

Each `ClientEvent::Notification` mutates the world:

| Control event | Projection action |
|---|---|
| `WindowAdd` / `WindowClose` | spawn / despawn `TmuxWindow` (+ cascade panes) |
| `LayoutChange { window, layout }` | reconcile that window's pane set & geometry against the cell tree (add/remove/move panes, set rects) |
| `Output { pane, data }` | look up the pane entity, `TerminalHandle::advance(&data)` |
| `WindowPaneChanged` / `SessionWindowChanged` | update active-pane / active-window pointers (focus + dim) |
| `Pause` / `Continue` | flow control: mark pane paused/resumed; a paused busy background pane must be resumed (`resume-pane` / re-subscribe) or its scrollback desyncs |
| `SessionChanged` / `SessionsChanged` / `*Renamed` | update session entity / names; refresh picker data |
| `Exit` / `ClientDetached` | tear down projection (see Connection lifecycle) |

`UnlinkedWindow*` events are intentionally dropped (single-attached-session
projection); revisit when a multi-session view lands.

### Reconciliation, not rebuild

`%layout-change` is diffed against current pane entities by `PaneId`:
existing panes keep their `TerminalHandle` (and scrollback) and are only
repositioned; new ids spawn panes; vanished ids despawn. This is what makes
the issue's "efficient incremental re-rendering" real — no teardown of live
grids on every layout event.

The reducer keeps indexed state — `HashMap<PaneId, Entity>` and
`HashMap<WindowId, Entity>` — and flattens each `WindowLayout` to a
`(PaneId, CellDims)` leaf list once per `%layout-change`, then 3-way
set-diffs against the live pane set (O(leaves)). The per-tick pump runs in
two phases: drain transport events into a `Vec<ClientEvent>`, then apply
mutations in deterministic order (avoids `World` borrow conflicts and allows
a per-frame drain cap if a pane floods output).

### Command echo model

When ozmux initiates a change (split, focus, resize), it sends the tmux
command and lets the resulting control event drive the projection. It never
mutates the ECS optimistically.

## Layout & rendering

### Geometry

tmux `WindowLayout` gives every cell `CellDims { width, height, xoff, yoff }`
in **character cells**. ozmux computes one cell's pixel size from the active
font metrics (available in `ozma_tty_renderer`'s glyph plugin), then
positions each pane `Node` with absolute coordinates:
`left = xoff * cell_w`, `top = yoff * cell_h`, `width = width * cell_w`,
`height = height * cell_h`. There is no Bevy flex split tree — panes are
absolutely placed siblings under the window node. This is faithful to tmux
and avoids flex-weight rounding drift.

### Bidirectional sizing

The GUI window has a pixel size; ozmux converts it to a cell grid and tells
tmux via `refresh-client -C <cols>x<rows>` (control-mode client size; the
`WxH` form requires tmux ≥ 3.2 — older tmux accepts only `W,H`, so set a
version floor). tmux re-lays-out and is expected to emit `%layout-change`
with dims that fit — this emit-after-resize behavior is **unverified** and
must be confirmed by a live `tmux -CC` integration test. The GUI window
size drives tmux's cell budget; tmux's resulting layout drives pane rects —
one authoritative direction per concern.

### Per-pane rendering

Each `TmuxPane` carries the same `TerminalRenderBundle` (grid +
`MaterialNode<TerminalUiMaterial>`) the old surfaces used. `%output` feeds a
**new PTY-less ingest system** (not `drain_pty_chunks`, which requires
`PtyHandle`): it calls the staged-damage ingest + `emit` to produce the
existing `FrameSnapshot`/`FrameDelta` events the renderer already consumes.
Pane resize from `%layout-change` updates cols/rows via a **grid-only**
resize path — `resize_terminals_to_node` / `TerminalHandle::resize` are NOT
reused, since both touch `PtyHandle` and call `pty.resize`
(`src/ui/terminal.rs:172`, `handle.rs:230`). The grid-only resize must not
echo size back to tmux (tmux already owns pane sizes; echoing would loop).

### Focus & dimming

The active pane comes from `%window-pane-changed`; inactive panes get the
existing dim overlay/material factor. This reuses `src/ui/workspace.rs` dim
logic, driven by the new active-pane pointer instead of `ActivePane`.

### Borders/gaps

tmux reserves 1 cell between panes for borders. ozmux can render its own
pane borders in those gutters (cosmetic, from the cell geometry) — detail
deferred to implementation.

## Input & keybind sync

### Keystroke routing

A `tmux -CC` control client's stdin is a **command channel, not a keystroke
channel**: every line is parsed as a tmux command (an empty line detaches),
and the repo's `ProtocolClient::send` rejects embedded `\n`/`\r` and frames
each write as one command (`crates/tmux_control/src/protocol.rs:52`). Raw VT
bytes therefore cannot be forwarded. Keys are issued as `send-keys`:
`send-keys -K -c <client>` to route a chord through tmux's key tables (so
tmux's prefix + bindings act), and `send-keys -t <pane>` for direct pane
input, with `-H <hex>` for bytes > 0x7f (send-keys UTF-8-re-encodes high
bytes otherwise). This needs a new Bevy-key → tmux-key-name mapping; the
existing `bevy_to_terminal_key` VT-byte encoder cannot be forwarded
verbatim. Consecutive keys in a frame are batched into one
`send-keys -H <hex> <hex> …` to avoid one command round-trip per key.

### GUI-chord interception

A small fixed set of ozmux-level chords is handled before forwarding and
never reaches tmux: connect/switch (open picker), detach, reconnect, and
quit. Everything else forwards.

### Keybind mirror (`list-keys`)

On attach, ozmux runs `list-keys` (and re-runs on relevant config events),
parses the key tables into a synced in-memory model, and uses it for two
things: (1) knowing which chords are meaningful tmux actions, and (2)
displaying/echoing bindings in ozmux UI (status hints, menus). ozmux does
**not** independently execute these — tmux remains the actor; the mirror is
for awareness/display, matching the issue's "retrieve and synchronize"
intent.

- Parser scope: run `list-keys -F <format>` with an explicit tab-separated
  format → `{ table, key-chord, command }` rows, avoiding a hand-rolled
  parser over tmux's human-readable `bind-key` syntax.
- This mirror is cosmetic (display/awareness only); it is **deferred off the
  critical path** — GUI-chord interception is a fixed hardcoded set and does
  not depend on it.
- High-risk commands (`refresh-client -C`, `send-keys`, `select-pane`,
  `split-window`) get small typed command-builders in `tmux_session` for
  target-id/key-name escaping discipline; `tmux_control` stays pure
  transport.
- Re-sync trigger: on attach and on `%config-error` / manual reload.

### Mouse

Click-to-focus → `select-pane -t <pane>` (geometry hit-test against the pane
rects we already compute). Wheel/scroll and drag-resize are deferred; the
first cut handles click-focus only.

## Migration phasing

Sequenced so the app stays runnable at each phase boundary. Each phase is
independently mergeable. The strategy is to **build the tmux path alongside
the old multiplexer (Phases 0–4), prove it, then delete the old one
(Phase 5)** — never a big-bang switch.

- **Phase 0 — Bevy integration scaffold.** New `crates/tmux_session` with
  the `TmuxClient` resource, the per-frame event pump
  (`TransportEvent`/`ClientEvent` drain), and a `ConnectionState` resource.
  No projection yet — just log events. The app still runs the old
  multiplexer.
- **Phase 1 — Connection lifecycle + projection skeleton.** Auto-connect at
  boot, server spawn, tmux-missing error dialog, MRU attach / create-if-none.
  Spawn `TmuxSession`/`TmuxWindow`/`TmuxPane` entities from the initial
  layout. No rendering yet (entities only, asserted in tests).
- **Phase 2 — Pane rendering.** PTY-less `TerminalHandle` per pane fed by
  `%output`; absolute cell-dim layout; `refresh-client` window sizing; reuse
  the render bundle. A tmux pane visibly renders. Behind a feature flag /
  alternate boot path so the old multiplexer still exists in parallel.
- **Phase 3 — Input + focus + reconcile.** Forward keys to the active pane,
  GUI-chord interception, click-to-focus, `%layout-change` reconciliation
  (incremental add/move/remove), focus/dim. Now it is an interactive tmux
  client.
- **Phase 4 — Session UX + keybind mirror.** Session-picker popup
  (switcher), detach/reconnect + idle overlay, `list-keys` parse & mirror
  for display.
- **Phase 5 — Remove the old multiplexer.** Delete `crates/multiplexer`,
  `src/multiplexer`, the Surface/tab layer; rewire/remove action observers;
  drop the bootstrap native terminal. Flip tmux to the only boot path. This
  is the irreversible cut — done last, once Phases 2–4 prove the tmux path
  covers the daily-use surface.

## Testing strategy

- **Parser/reducer (pure, no tmux):** the event-reducer is a pure function
  `(projection_state, ControlEvent) → projection_state`. Unit-test it with
  synthetic `ControlEvent` streams (the parser crates already have rich
  fixtures) — covers add/close/layout-change/reconcile without spawning
  tmux. This is the bulk of correctness coverage.
- **Layout math:** `WindowLayout` cell tree → pane rects, table-driven
  against known tmux layout strings.
- **Integration (real tmux, gated):** a handful of tests behind a
  `#[cfg]`/env gate that spawn a real `tmux -CC` (CI may skip if absent),
  asserting attach → pane spawn → output render → split echo. Mirrors the
  existing real-host E2E gating pattern in the repo.
- **Bevy systems:** headless `App` ticks feeding canned `ClientEvent`s,
  asserting entity spawn/despawn and component state.

## Deferred scope

Explicitly out of scope for this migration; tracked for later:

- **Standalone single-terminal bootstrap mode** — shelved per decision 2.
- **Copy mode** — tmux has its own; ozmux's `CopyModePlugin` interplay is
  deferred. First cut: tmux copy mode runs as normal pane output.
- **Inline webviews (OSC 5379)** — whether `%output` carries the OSC through
  cleanly and how mount/unmount survives tmux-owned panes needs its own
  investigation. Deferred; not removed.
- **Mouse wheel/scroll & drag-resize**, **IME** revalidation under tmux,
  **hyperlink hover** — first cut does click-to-focus + keyboard only.
- **Multi-window/tab strip UI** — windows are projected but a window
  switcher UI is a follow-up.
- **`%subscription-changed` Option-field fix** — the existing TODO in the
  parser (`crates/tmux_control_parser/src/event.rs`), verified against a
  live session when integration tests land.
