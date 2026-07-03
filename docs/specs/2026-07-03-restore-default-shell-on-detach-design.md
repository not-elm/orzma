# Restore the Default shell on tmux detach

Date: 2026-07-03
Status: approved (design), pending implementation plan

> **Path note:** this doc's file paths reflect the module layout at the time
> it was written. A later merge of main's #234 ("dissolve `src/mode` into
> feature-first modules") moved them: `src/mode/tmux/adopt.rs` →
> `src/session/tmux/adopt.rs`; `src/mode/default.rs` →
> `src/ui/default_mode.rs`; `src/mode/default/spawn.rs` →
> `src/session/default/spawn.rs`; `src/mode/default/exit.rs` →
> `src/session/default/exit.rs`; `src/mode/default/layout.rs` →
> `src/session/default/layout.rs`.

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
- The post-detach bytes (the returning shell prompt) are fed into the VT, and
  ozmux synthesizes a `[detached (from session …)]` line from the parsed
  `%exit` reason — in control mode tmux never writes that message to the PTY
  (it is the non-control branch of tmux's `client_main`), so the iTerm2-style
  readback must be fabricated locally.
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
folded in below. A follow-up parallel spec review (Codex + Claude agent, the
latter verifying against tmux's `client.c`) corrected two protocol facts (no
`[detached …]` line and no trailing newline after the terminator in control
mode), closed the fresh-introducer-in-residue hole via `Handover::scan`
routing, consolidated the release events into one, and added the `TmuxClient`
passthrough and `mode::default` restore-helper requirements.

## Architecture — four work areas

### 1. `crates/tmux_control` — protocol end-of-stream contract

`ProtocolClient::feed` currently ignores a bare DCS terminator line (`ESC \`)
and would misparse bytes after it. New contract:

- The terminator is detected as a **byte prefix** of the unparsed buffer after
  a completed line, NOT as a newline-delimited line: on a real detach tmux
  writes `%exit <reason>\n` followed immediately by `ESC \` with **no
  trailing newline** (verified against tmux's `client.c`), so a whole-line
  equality check — like today's `content == DCS_TERMINATOR` — would never
  fire on a real stream. Detection must handle the two terminator bytes split
  across chunks (a 1-byte `ESC` carry).
- Consuming the terminator transitions the client to an **ended** state.
- Every byte after the terminator — in the same chunk or in later `feed()`
  calls — accumulates in a **residual buffer** instead of being line-parsed.
- `take_residual()` drains the buffer; `is_ended()` queries the state.
- `feed()` after end produces no further events.

This is a behavioral change: existing tests that feed a terminator and expect
parsing to continue must be updated. `feed_strips_dcs_wrapper` models the
terminator as its own CRLF-delimited line, which does not match the
source-verified stream shape; it must be rewritten to the glued no-newline
form. tmux only emits the terminator at end of control mode, so ending
unconditionally on it is correct.

`TmuxClient` (in `tmux_session`) gains passthrough accessors for the new
surface — `take_residual()` (and `is_ended()` as needed) — because
`ProtocolClient` is a private field the binary cannot reach;
`teardown_on_exit_notification`'s query widens to `&mut TmuxClient`. New
event/API types are exported from their crates' public surfaces.

### 2. `crates/ozma_tty_engine` — un-adopt API

A new `EntityEvent` — `ReleaseControlMode { entity, residual: Vec<u8> }` —
with an engine-side observer that, atomically at command-flush time (observers
run with exclusive world access, so no bytes can race past):

1. Takes any bytes still buffered on `AdoptedControlMode.captured` — bytes
   that arrived after the `%exit` drain but before the release lands — then
   removes `AdoptedControlMode` and re-inserts `ControlModeWatch`. This is
   the inverse of the existing adoption-side in-loop drain
   (`crates/ozma_tty_engine/src/lib.rs:191`), which handles the mirror-image
   race.
2. Routes the event's `residual` bytes, then the late-captured bytes (in that
   order — residual was drained from `captured` earlier), through the same
   `Handover::scan` path the normal PTY pump uses — NOT a raw VT feed.
   `NotYet` bytes reach the VT; a `Detected` result re-adopts immediately and
   fires `ControlModeDetected`. A fresh `tmux -CC` introducer can
   legitimately sit inside these bytes (e.g. `tmux -CC && tmux -CC`), and a
   raw feed would lose it and corrupt the restored terminal.
3. Ingests VT-bound bytes via the same flush-or-arm contract as normal PTY
   chunks (`ingest_and_flush_or_arm`) and drains terminal control events
   afterwards — a bare VT feed would leave the restored prompt invisible
   until the next PTY chunk arrives and would drop OSC title/cwd/webview
   events parsed from the residue.

The observer lives in `ozma_tty_engine`, so the crate-private ingest/scan
machinery stays crate-private.

### 3. `crates/tmux_session` — gateway release cleanup

No separate event: `TmuxSessionPlugin` registers its own observer on the
engine's `ReleaseControlMode` (`tmux_session` already depends on
`ozma_tty_engine`) that strips the connection components today's despawn
removes for free: `TmuxClient`, `TmuxAttached`, and `EnumerationState`.
`EnumerationState` is crate-private (auto-required by `TmuxClient`), which is
why this cleanup must live in `tmux_session`, not the binary. The engine and
tmux_session observers touch disjoint component sets, so their relative order
is irrelevant.

Existing `TmuxConnectionReset` handling (projection despawn, copy-query clear,
event-batch clear in `observers.rs::on_connection_reset`) is reused unchanged.

### 4. `src/mode/tmux/adopt.rs` — teardown split

`teardown` splits into two paths:

**Detach (`%exit`, via `teardown_on_exit_notification`) → restore:**

1. `take_residual()` from the gateway's `TmuxClient`, and read the `%exit`
   reason from the batch's `Exit` event to synthesize the
   `[detached (from session …)]` line prepended to the residual.
2. Trigger `ReleaseControlMode { gateway, residual }` — one event, two
   observers: tmux_session strips the connection components; the engine
   re-feeds bytes, removes `AdoptedControlMode`, re-arms `ControlModeWatch`.
3. Restore the UI via a restore helper exposed by `crate::mode::default` (the
   canonical full-size `Node` shape and the `DefaultShell` marker are
   module-private to Default mode, so the restore surface lives there, not ad
   hoc in `adopt.rs`): insert the full-size absolute `Node` (same layout as
   `OzmaTerminalBundle::spawn` — adoption *overwrote* the `Node` with
   `Display::None` + defaults, so flipping `display` back is not enough),
   re-insert `KeyboardFocused` (keyboard routing requires exactly one focused
   terminal — `src/input/keyboard.rs`), spawn a fresh `DefaultModeUi`
   container under `UiRoot`, and reparent the gateway into it.
4. Reset the `GatewaySize` dedup resource so a later re-adoption at the same
   window size still emits its full-window `TerminalResize`.
5. Trigger `TmuxConnectionReset` + `TmuxConnectionClosed` as today
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

1. User detaches. tmux writes `%exit <reason>\n`, then the DCS terminator
   `ESC \` with no trailing newline, and the client exits; the shell prompt
   then returns over the same gateway PTY. tmux writes no `[detached …]`
   line in control mode — ozmux synthesizes it (step 4).
2. The engine pump appends those bytes to `AdoptedControlMode.captured` (VT
   feeding is still paused).
3. `drain_tmux_transport` takes `captured` and feeds the protocol client: the
   `Exit` event lands in `TmuxEventBatch`, the terminator flips the client to
   *ended*, and everything after it is stashed as residual.
4. `teardown_on_exit_notification` (after `TmuxProjectionSet`, same frame)
   sees `%exit` in the batch and runs the restore path (steps 1–5 above),
   synthesizing the detach line from the `Exit` reason.
5. At command flush, both `ReleaseControlMode` observers run: tmux_session
   strips the connection components; the engine routes the synthesized line +
   residual + late-captured bytes through `Handover::scan` into the VT
   (flush-or-arm + control-event drain), removes `AdoptedControlMode`, and
   re-arms `ControlModeWatch`; `on_connection_reset` clears projection and
   batch; `TmuxConnectionClosed` sets `NextState(AppMode::Default)`.
6. Next frame: `AppMode::Default` applies. `ensure_default_mode_ui`'s `run_if`
   (`not(any_with_component::<DefaultModeUi>)`) sees the restored container,
   so no second shell spawns. No resize is needed: the Default layout system
   keeps even the hidden gateway sized to the full window throughout tmux
   mode (`src/mode/default/layout.rs`), and the gateway sync uses the
   identical size, so the dedup guards absorb it. The grid shows scrollback +
   synthesized detach line + live prompt.

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
  the atomic observer drain. A fresh `tmux -CC` introducer is caught in both
  places: inside the release-fed bytes because the release observer routes
  them through `Handover::scan` (a raw feed would lose it), and in later
  chunks by the re-armed watch.
- **One-frame double-focus window**: teardown re-inserts `KeyboardFocused` on
  the gateway at flush while the tmux active pane still carries it; the tmux
  surfaces are only despawned by `DespawnOnExit(AppMode::Tmux)` on the
  next-frame state transition, so keyboard dispatch's `.single()` no-ops for
  one frame. Harmless — but tests must not assert single-focus on that same
  frame.

## Testing

- **`tmux_control`**: ended-state contract — the PRIMARY case is the
  source-verified stream shape: `%exit <reason>\n` + `ESC \` glued directly
  to residual bytes with no newline anywhere after `%exit`; also terminator
  split across chunks (1-byte `ESC` carry); residual accumulating over later
  `feed()` calls; `feed()` after end produces no events; `take_residual()`
  drains once; `feed_strips_dcs_wrapper` rewritten to the real stream shape.
- **`ozma_tty_engine`**: release observer mirrors the existing adoption-race
  test — bytes staged on `captured` at release time reach the VT; ordering
  (residual before late-captured); rendered without a subsequent PTY chunk
  (flush-or-arm honored); control events drained; `ControlModeWatch`
  re-armed; `AdoptedControlMode` removed; an introducer inside the
  release-fed bytes re-adopts immediately (fires `ControlModeDetected`).
- **`tmux_session`**: the `ReleaseControlMode` observer strips `TmuxClient`,
  `TmuxAttached`, and `EnumerationState` without despawning; `take_residual`
  passthrough; the existing re-adoption test gains a component-strip
  (non-despawn) teardown variant.
- **`src/mode/tmux/adopt.rs`**: detach teardown restores — entity alive,
  under a fresh `DefaultModeUi`, `KeyboardFocused` and full-size `Node`
  present, `GatewaySize` reset, synthesized detach line prepended to the
  release bytes; death teardown still despawns.
- **`src/mode/default.rs`**: `default_shell_survives_mode_roundtrip` extended
  to the adopted round-trip — adopt → `%exit` → the same entity is restored
  and no second shell spawns.
