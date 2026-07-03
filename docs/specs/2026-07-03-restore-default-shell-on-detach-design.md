# Restore the Default shell on tmux detach

Date: 2026-07-03
Status: approved (design), pending implementation plan

## Problem

When the user runs `tmux -CC` in the Default-mode shell, that terminal entity is
adopted as the tmux control-mode gateway. On detach (`%exit`), the current
teardown (`src/mode/tmux/adopt.rs::teardown`) despawns the gateway entity,
which kills the still-alive shell process via the PTY's `Drop`, and a brand-new
shell is spawned on the return to `AppMode::Default`. Scrollback, running jobs,
cwd, and shell history are lost — even though on a plain detach the shell
process survives naturally (`tmux -CC` exits like any command and the prompt
returns).

## Goal

Full continuity across the `Default → Tmux → Default` round-trip via detach:

- The same shell entity and process come back into the Default view.
- Scrollback up to the `tmux -CC` command is preserved (the VT state was never
  destroyed — feeding was only paused).
- The post-detach bytes — tmux's `[detached (from session …)]` line and the
  shell prompt — are fed into the VT, so the terminal reads exactly as if
  `tmux -CC` had run in a regular terminal (iTerm2-style).
- Running `tmux -CC` again from the restored shell re-adopts it; round-trips
  work indefinitely.

Out of scope / unchanged:

- **Death path**: if the gateway's child process actually exits during tmux
  mode (`TerminalChildExit`), behavior is unchanged — the gateway is
  despawned and `on_child_exit` (`src/mode/default/exit.rs`) fires `AppExit`.
- **Replace-while-live**: a second `tmux -CC` handshake while a connection is
  live still despawns the old gateway (defensive path in
  `on_control_mode_detected`).

## Approach

Approach chosen: **un-adopt in teardown** (adoption unchanged; only the
`%exit` teardown changes from despawn to restore). Alternatives considered and
rejected:

- *Hide-don't-despawn* (keep `DefaultModeUi` alive but hidden during tmux
  mode): trivial restore, but churns the adoption path and leaves a hidden
  live subtree participating in the world for the whole tmux session.
- *Engine-owned symmetry* (the tty engine detects end-of-control-mode and
  auto-reverts): the engine lacks protocol line-structure context; terminator
  detection belongs in `tmux_control`'s parser.

An external Codex review independently confirmed the chosen approach and
contributed the `tmux_session` cleanup, `Node` restore, atomic captured-byte
drain, `GatewaySize` reset, and post-exit `feed()` contract requirements
folded in below.

## Architecture — four work areas

### 1. `crates/tmux_control` — protocol end-of-stream contract

`ProtocolClient::feed` currently ignores a bare DCS terminator line (`ESC \`)
and would misparse bytes after it. New contract:

- Consuming the DCS terminator transitions the client to an **ended** state.
- Every byte after the terminator — in the same chunk or in later `feed()`
  calls — accumulates in a **residual buffer** instead of being line-parsed.
  This covers residue spanning multiple PTY chunks and post-terminator bytes
  that arrive with no trailing newline (the current line-oriented handling
  would strand them in `line_buf`).
- `take_residual()` drains the buffer; `is_ended()` queries the state.
- `feed()` after end produces no further events.

This is a behavioral change: existing tests that feed a terminator and expect
parsing to continue (e.g. `feed_strips_dcs_wrapper`) must be updated to the
new ended-state semantics. tmux only emits the terminator at end of control
mode, so ending unconditionally on it is correct.

### 2. `crates/ozma_tty_engine` — un-adopt API

A new `EntityEvent` — `ReleaseControlMode { entity, residual: Vec<u8> }` —
with an engine-side observer that, atomically at command-flush time (observers
run with exclusive world access, so no bytes can race past):

1. Feeds `residual` into the terminal's VT emulator.
2. Feeds-and-drains whatever is still buffered on
   `AdoptedControlMode.captured` — bytes that arrived after the `%exit` drain
   but before the release lands. Ordering: residual first (it was drained from
   `captured` earlier), then the late-captured bytes. This is the inverse of
   the existing adoption-side in-loop drain
   (`crates/ozma_tty_engine/src/lib.rs:191`), which handles the mirror-image
   race.
3. Removes `AdoptedControlMode`.
4. Re-inserts `ControlModeWatch`, so a later `tmux -CC` re-adopts.

Requires exposing a VT ingest path to the observer (the ingest method on the
handle is currently crate-private, which is fine — the observer lives in the
same crate).

### 3. `crates/tmux_session` — gateway release cleanup

A new `EntityEvent` — `TmuxGatewayReleased` — with an observer registered by
`TmuxSessionPlugin` that strips the connection components today's despawn
removes for free: `TmuxClient`, `TmuxAttached`, and `EnumerationState`.
`EnumerationState` is crate-private (auto-required by `TmuxClient`), which is
why this cleanup must live in `tmux_session`, not the binary.

Existing `TmuxConnectionReset` handling (projection despawn, copy-query clear,
event-batch clear in `observers.rs::on_connection_reset`) is reused unchanged.

### 4. `src/mode/tmux/adopt.rs` — teardown split

`teardown` splits into two paths:

**Detach (`%exit`, via `teardown_on_exit_notification`) → restore:**

1. `take_residual()` from the gateway's protocol client.
2. Trigger `TmuxGatewayReleased { gateway }` (tmux_session strips connection
   components).
3. Trigger `ReleaseControlMode { gateway, residual }` (engine re-feeds bytes,
   removes `AdoptedControlMode`, re-arms `ControlModeWatch`).
4. Restore the UI: insert the full-size absolute `Node` (same layout as
   `OzmaTerminalBundle::spawn` — adoption *overwrote* the `Node` with
   `Display::None` + defaults, so flipping `display` back is not enough),
   re-insert `KeyboardFocused` (keyboard routing requires exactly one focused
   terminal — `src/input/keyboard.rs`), spawn a fresh `DefaultModeUi`
   container under `UiRoot`, and reparent the gateway into it.
5. Reset the `GatewaySize` dedup resource so a later re-adoption at the same
   window size still emits its full-window `TerminalResize`.
6. Trigger `TmuxConnectionReset` + `TmuxConnectionClosed` as today
   (`TmuxConnectionClosed` drives the return to `AppMode::Default`).

**Death (`TerminalChildExit`, via `on_gateway_child_exit`) → unchanged:**
despawn + `TmuxConnectionReset` + `TmuxConnectionClosed`; `on_child_exit`
fires `AppExit`.

No work needed elsewhere:

- **Window title** recovers automatically — it reads the focused terminal's
  title (`src/window_title.rs`).
- **Webview surface token** stays valid — the entity is preserved, and GC only
  fires on `TerminalHandle` removal
  (`crates/ozma_webview/src/control_plane.rs`).
- **`DefaultShell` marker** never leaves the entity.

## Detach data flow (frame by frame)

1. User detaches. tmux writes `%exit`, then the DCS terminator `ESC \`, then
   the client prints `[detached (from session …)]`, exits, and the shell
   prompt returns — all over the same gateway PTY.
2. The engine pump appends those bytes to `AdoptedControlMode.captured` (VT
   feeding is still paused).
3. `drain_tmux_transport` takes `captured` and feeds the protocol client: the
   `Exit` event lands in `TmuxEventBatch`, the terminator flips the client to
   *ended*, and everything after it is stashed as residual.
4. `teardown_on_exit_notification` (after `TmuxProjectionSet`, same frame)
   sees `%exit` in the batch and runs the restore path (steps 1–6 above).
5. At command flush: tmux_session strips the connection components; the engine
   observer feeds residual + late-captured bytes into the VT, removes
   `AdoptedControlMode`, re-arms `ControlModeWatch`; `on_connection_reset`
   clears projection and batch; `TmuxConnectionClosed` sets
   `NextState(AppMode::Default)`.
6. Next frame: `AppMode::Default` applies. `ensure_default_mode_ui`'s `run_if`
   (`not(any_with_component::<DefaultModeUi>)`) sees the restored container,
   so no second shell spawns. Default-mode layout resizes the terminal back
   from the full-window gateway size; the grid shows scrollback + detach
   message + live prompt.

## Edge cases

- **Re-adoption**: the restored shell runs `tmux -CC` again → the re-armed
  `ControlModeWatch` fires the normal adoption path. `GatewaySize` was reset,
  so the full-window resize re-emits even at an identical window size.
  `TmuxClient::new_adopted()` is constructed fresh per adoption, so the old
  *ended* protocol state cannot leak. Round-trips work indefinitely.
- **Double teardown / idempotency**: the `With<TmuxClient>` guard holds — the
  component is removed at flush, so a second trigger finds no client; the
  `%exit`-in-batch scan is additionally neutralized by the batch clear in
  `on_connection_reset`.
- **`%exit` and child-exit racing in the same frame**: whichever lands first
  wins; the other no-ops via the `With<TmuxClient>` guard. Worst case
  (restore first, then child exit) degrades to today's behavior: `AppExit`.
- **Late residual chunks**: after `ControlModeWatch` is re-armed, normal VT
  feeding handles further PTY bytes; only the flush-time window is covered by
  the atomic observer drain. A fresh `tmux -CC` introducer within those bytes
  is caught by the re-armed watch like at any other time.

## Testing

- **`tmux_control`**: ended-state contract — residual after terminator in the
  same chunk; split across chunks; no-newline residue; `feed()` after end
  produces no events; `take_residual()` drains once.
- **`ozma_tty_engine`**: release observer mirrors the existing adoption-race
  test — bytes staged on `captured` at release time reach the VT; residual
  ordering (residual before late-captured); `ControlModeWatch` re-armed;
  `AdoptedControlMode` removed.
- **`tmux_session`**: `TmuxGatewayReleased` strips `TmuxClient`,
  `TmuxAttached`, and `EnumerationState` without despawning; the existing
  re-adoption test gains a component-strip (non-despawn) teardown variant.
- **`src/mode/tmux/adopt.rs`**: detach teardown restores — entity alive,
  under a fresh `DefaultModeUi`, `KeyboardFocused` and full-size `Node`
  present, `GatewaySize` reset; death teardown still despawns.
- **`src/mode/default.rs`**: `default_shell_survives_mode_roundtrip` extended
  to the adopted round-trip — adopt → `%exit` → the same entity is restored
  and no second shell spawns.
