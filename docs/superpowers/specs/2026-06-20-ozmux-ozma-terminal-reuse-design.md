# Reuse `ozma_terminal` for Ozmux-mode Panes — Mouse Unification + App Mouse-Report Forwarding — Design

**Date:** 2026-06-20
**Status:** Design (pre-plan)
**Crates:** `ozma_terminal`, `ozma_tty_engine` (+ host `src/tmux/*`)

## Goal

In `AppMode::Ozmux` (the tmux `-CC` backend), make each tmux pane a
first-class `OzmaTerminal` entity so that **text selection, clipboard copy,
hyperlink hover/open, and mouse-report forwarding are driven by the shared
`ozma_terminal` systems** instead of the bespoke arbiter in `src/tmux/mouse.rs`.
This removes a large block of duplicated terminal-interaction logic (the
`ozma_terminal` multi-terminal mouse stack was itself modelled on the tmux
arbiter — see `docs/superpowers/specs/2026-06-20-ozma-terminal-multi-terminal-mouse-design.md`
lines 59-66 — so the two converge onto one implementation).

Keyboard input continues to pass through to tmux unchanged; only the
terminal-interaction (mouse) layer is unified. As a deliberate capability gain,
mouse-aware programs inside panes (vim, htop, …) receive mouse reports, matching
iTerm2's tmux integration.

## Scope

**In scope:**

- Attach `OzmaTerminal` to each projected `TmuxPane`
  (`src/tmux/render.rs::attach_tmux_pane_terminal`).
- A backend-agnostic input sink seam in the crate: make the mouse apply
  observer tolerate a PTY-less terminal and emit a new `TerminalForwardInput`
  `EntityEvent` instead of writing to a `PtyHandle`.
- A host observer (`src/tmux`) that routes `TerminalForwardInput` to the pane
  via `send_bytes_command` (`send-keys -H`).
- A host gate maintainer for `AppMode::Ozmux` that sets `KeyboardDisabled`
  (always) and `MouseDisabled` (modal / copy-mode / webview) per pane.
- Delete the now-duplicated local-selection / multi-click copy / hover /
  hyperlink-open code from `src/tmux/mouse.rs`; keep tmux-specific gestures.

**Out of scope (unchanged):**

- Keyboard forwarding to tmux (`src/tmux/input.rs`,
  `send_pane_keys_command`) — kept as-is; ozma_terminal's keyboard dispatch is
  fully disabled on panes.
- The tmux control connection, projection, layout, window bar, dialogs.
- `ozma_tty_engine`'s `drain_pty_writes` (VT auto-replies). For panes these are
  already drained-and-discarded (`src/tmux/render.rs` "tmux panes have no
  PtyHandle" NOTE around lines 190-205) and must stay discarded — tmux already
  answered the program's DSR/DA queries.
- tmux copy-mode selection itself (still driven by `send-keys -X`).

## Background — current state

Two parallel terminal-interaction stacks exist:

- **Ozma mode**: one `OzmaTerminal` entity owning a real PTY
  (`ozma_tty_engine::TerminalHandle` + `PtyHandle` + `Coalescer`).
  `ozma_terminal` owns input dispatch, mouse (selection / copy / wheel /
  hyperlink), and clipboard. The host (`src/ozma_input.rs`) maintains
  per-entity `KeyboardDisabled` / `MouseDisabled` for modal suppression.
- **Ozmux mode**: each `TmuxPane` gets a **PTY-less**
  `TerminalHandle::detached(...)` (no `PtyHandle`, no `Coalescer`) plus the GPU
  render bundle (`src/tmux/render.rs::attach_tmux_pane_terminal`); tmux
  `%output` is fed via `handle.advance(&data)` (`route_tmux_output`). Input and
  mouse are reimplemented in `src/tmux/input.rs` (~1000 lines) and
  `src/tmux/mouse.rs` (~1480 lines): keyboard → `send-keys`; mouse → pane
  `select-pane`, native VT text selection, multi-click word/line select+copy,
  divider-drag `resize-pane`, copy-mode `send-keys -X`, inline-webview (CEF)
  mouse, and wheel (copy-mode scroll / alt-screen cursor keys / local VT
  scroll). It does **not** forward mouse reports to in-pane apps.

`ozma_terminal`'s mouse systems (`dispatch_mouse_buttons`,
`dispatch_mouse_wheel`, `OzmaMousePlugin`, `crates/ozma_terminal/src/mouse.rs`
~867) are gated only by `run_if(on_message::<MouseButtonInput|CursorMoved|MouseWheel>)`,
**not** by `AppMode`. They already run in Ozmux mode but currently match no
entities (panes lack `OzmaTerminal`). Attaching `OzmaTerminal` to panes makes
them participate with **no extra system registration**.

### Why the apply path does not "just work"

The mouse apply observer requires a PTY:

```
fn on_terminal_mouse_effects(
    ev: On<TerminalMouseEffects>,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer), With<OzmaTerminal>>,
)
```

A pane has neither `PtyHandle` nor `Coalescer`, so `get_mut(ev.entity)` returns
`Err` and the observer skips the pane entirely — dropping even local selection
and copy. The same is true of `on_terminal_key_input`
(`crates/ozma_tty_engine/src/plugin.rs:161`), but keyboard never reaches it on
panes (they are `KeyboardDisabled`). The seam therefore only needs to cover the
**mouse** apply path.

## Research summary — forwarding mouse reports to in-pane apps over `tmux -CC`

Confirmed by parallel investigation (tmux master source + man page, iTerm2
source, this codebase). High confidence.

- **`send-keys -M` cannot be used.** It is hard-gated on a real mouse event
  already present on the command-queue item (`cmd-send-keys.c`; `cmd.c`
  `if (!m->valid) return NULL` → `"no mouse target"`). A control client cannot
  synthesize arbitrary coordinates through it. Man page: "only valid if bound
  to a mouse key binding".
- **`send-keys -H` (hex) / `-l` (literal) work.** Bytes flow
  `cmd_send_keys_inject_string → window_pane_key → input_key → input_key_write
  → bufferevent_write`, reaching the pane's child PTY verbatim. tmux's `input.c`
  parses pane **output**, never injected input, so the raw SGR sequence is not
  re-interpreted as a mouse event.
- **iTerm2 precedent.** iTerm2's `-CC` integration generates SGR/X10 bytes
  itself and writes them with `send -t`/`send -H -t` (`TmuxGateway.m
  sendKeys:toWindowPane:`); it uses `-M` nowhere and never sets tmux's `mouse`
  option. It mirrors each pane's in-app mouse mode via `#{mouse_*_flag}`.
- **ozmux already owns the parts.** SGR/X10 encoder
  `encode_protocol_event` (`crates/ozma_tty_engine/src/mouse_encode.rs`,
  1-indexed coords); raw-byte injector `send_bytes_command` → `send-keys -H -t
  %id <hex>` (`crates/tmux_session/src/input.rs:107`); per-pane mouse mode is
  observable on the PTY-less handle because `%output`'s DECSET 1000/1002/1003/1006
  is tracked by the display VT and exposed via `TerminalHandle::current_modes()`
  (`crates/ozma_tty_engine/src/handle.rs:374`).
- **Pitfalls (handled by this design):** only forward when the pane's app has
  mouse mode on — already guaranteed because `ButtonAction::route`
  (`crates/ozma_tty_engine/src/buttons.rs`) only emits `WriteToPty` when
  `app_captured = modes.intersects(MOUSE_MODE)` and Shift is not held; keep
  tmux's own `mouse` option off (ozmux never sets it); SGR coords are 1-origin,
  pane-relative (`cell_at_local`, `src/tmux/pane_hit.rs:47`, already 1-indexed);
  forward based on the app's mouse mode, not alt-screen.

## Design decisions (from brainstorming)

1. **End state = de-duplication / unification refactor**, not an incremental
   add. Panes become `OzmaTerminal`; the overlapping `src/tmux/mouse.rs` logic
   is deleted; tmux-specific gestures remain.
2. **Keyboard** stays on the existing tmux forward path; panes carry
   `KeyboardDisabled` so `ozma_terminal`'s `dispatch_input` ignores them.
3. **Mouse routing = app-forward (option B).** Reuse `ozma_terminal`'s
   `decide_button` / `decide_wheel` and the SGR encoder unchanged; a
   mouse-mode-on pane forwards reports to the program. Behaviour is a strict
   superset of today: shells (mouse off) still get local selection; Shift forces
   local selection; vim/htop gain working mouse.
4. **Sink seam = optional `PtyHandle` + `TerminalForwardInput` EntityEvent**
   (selected over a host parallel-observer approach and over a
   `TerminalInputSink` trait). The crate stays host-agnostic — it emits
   "backend-bound bytes"; the host decides the destination.

## Architecture

### 1. Panes become `OzmaTerminal`

`attach_tmux_pane_terminal` adds `OzmaTerminal` to the inserted bundle. The
crate's mouse / hyperlink systems (already running, `AppMode`-independent) then
hit-test and drive panes via the existing topmost-`stack_index` logic.

**Render-bundle reconciliation (required):** `OzmaTerminalPlugin` injects a
`TerminalRenderBundle` via an `On<Add, OzmaTerminal>` observer
(`crates/ozma_terminal/src/spawn.rs:99-109`), and `attach_tmux_pane_terminal`
already inserts one (`src/tmux/render.rs:140-148`). Adding `OzmaTerminal` as-is
double-inserts. Resolve by either (a) dropping the tmux-side
`TerminalRenderBundle` insert and letting the `Add` observer own it, or (b)
making the `Add` observer skip entities that already carry `TerminalGrid` / a
render bundle. Option (a) is preferred (single owner) unless the tmux side needs
bespoke material control.

### 2. The sink seam — `TerminalForwardInput`

New crate `EntityEvent`:

```
#[derive(EntityEvent)]
pub struct TerminalForwardInput {
    #[event_target]
    pub entity: Entity,
    pub bytes: Vec<u8>,
}
```

Change `on_terminal_mouse_effects` to make the PTY optional:

```
mut terminals: Query<(&mut TerminalHandle, Option<&mut PtyHandle>, Option<&mut Coalescer>, ...), With<OzmaTerminal>>,
```

- `MouseEffect::Write(bytes)`: if `PtyHandle` present → `handle.write(&mut pty,
  &bytes)` (today's path, with the existing scroll-to-bottom when a `Coalescer`
  is present); else → `commands.trigger(TerminalForwardInput { entity:
  ev.entity, bytes })`.
- Local effects (`SelStart` / `SelUpdate` / `SelClear` / `Copy` / `OpenUri` /
  `Scroll`) apply to `TerminalHandle` / `Clipboard` regardless of `PtyHandle`.
  **Implementer note:** the current `apply_effect` calls the *coalescer-bound*
  methods (`selection_start_at` / `selection_update_to` / `selection_clear` /
  `scroll`, all `&mut Coalescer`). A PTY-less pane has no `Coalescer`, so the
  PTY-less branch MUST use the `*_vt_only` variants (`selection_start_at_vt_only`
  / `selection_update_to_vt_only` / `selection_clear_vt_only` / `scroll_vt_only`,
  `crates/ozma_tty_engine/src/handle.rs:343-358,561-610`) followed by one
  `flush_emit` after the effect list. Track whether any local state changed and
  `flush_emit` once (the observer gains `Commands`).

The crate provides no handler for the PTY-less branch. The **host** registers:

```
fn forward_pane_input(ev: On<TerminalForwardInput>, panes: Query<&TmuxPane>, conn: NonSend<TmuxConnection>) {
    let Ok(pane) = panes.get(ev.entity) else { return };   // no-op for non-pane entities
    let Some(client) = conn.client() else { return };
    let _ = client.handle().send(&send_bytes_command(&pane_target(pane), &ev.bytes));
}
```

`on_terminal_key_input` is left unchanged (panes are `KeyboardDisabled`, so it
never fires for them). The host observer forwards whatever bytes the crate
emitted and MUST NOT add its own mouse-mode gate: `decide_*` emits `Write` for
mouse-captured buttons **and** for the alt-screen wheel-arrow fallback
(`crates/ozma_tty_engine/src/wheel.rs:173-184`), so a mouse-mode-only gate would
drop legitimate alt-screen scroll. Forwarding is already gated at the decision
site.

### 3. Residual tmux gestures and arbitration

Kept in `src/tmux` (tmux-specific, no `ozma_terminal` equivalent):

- pane focus on press (`select-pane`),
- divider drag → `resize-pane`,
- copy-mode mouse (`send-keys -X`),
- inline-webview (CEF) mouse.

Deleted from `src/tmux/mouse.rs` (now owned by `ozma_terminal`): local VT
selection state machine, multi-click word/line select+copy, hover-underline,
hyperlink open.

Arbitration is host-side via `MouseDisabled` and system ordering; the crate
stays generic. Concretely, a single **pre-gate system runs
`.before(OzmaTerminalMouseSet)`** and inserts/removes `MouseDisabled` per pane
(both reviewers flagged that independent `MessageReader` cursors mean
`ozma_terminal` sees every press unless suppressed before its set runs):

- **copy-mode**: a pane in `CopyModeState` is marked `MouseDisabled` so
  `ozma_terminal` yields and the existing tmux copy-mode mouse path runs.
- **divider**: the grab-tolerance band is geometrically near-disjoint from pane
  interiors; when a press lands in the band the tmux divider system claims it and
  suppresses `ozma_terminal` for that press (a transient claim flag /
  `MouseDisabled`-equivalent on the candidate pane).
- **inline-webview**: an existing webview hit claims the press (existing
  `NonInteractive` / focus path) and suppresses `ozma_terminal`.
- **pane focus** coexists with `ozma_terminal` selection — a press both focuses
  the pane and may arm a selection, which is the expected terminal behaviour.

### 4. Wheel

- Normal pane (no mouse mode, not copy mode): `ozma_terminal` scrolls the local
  VT scrollback — same as today.
- Mouse-mode-on pane: `ozma_terminal` emits a wheel SGR `Write` → forwarded via
  `TerminalForwardInput`.
- Copy-mode pane: `MouseDisabled` → existing tmux `send-keys -X scroll-up|down`.
- **Alt-screen scroll is owned by `ozma_terminal`, gated on `ALTERNATE_SCROLL`
  (DECSET 1007), not on "no mouse mode".** `WheelAction::route` already forwards
  SS3 arrows (`ESC O A` / `ESC O B`) for an alt-screen pane that set 1007
  (`crates/ozma_tty_engine/src/wheel.rs:173-184`); those bytes flow through the
  same `TerminalForwardInput` sink. The tmux `alt_screen_scroll_command`
  (key-name `Up`/`Down`, `src/tmux/input.rs:473`) MUST therefore be removed or
  narrowed to the residual `is_in_alt_screen() && !ALTERNATE_SCROLL &&
  !MOUSE_MODE` case and gated so it never runs alongside the crate's wheel for
  the same pane — keeping both un-gated double-sends (SS3 vs DECCKM-encoded `Up`).

### 5. Gate maintainer (Ozmux)

New host system `maintain_tmux_input_gates`, `run_if(in_state(AppMode::Ozmux))`,
`.before(OzmaTerminalInputSet)` / `.before(OzmaTerminalMouseSet)` (mirrors
`src/ozma_input.rs::maintain_input_gates`):

- every pane: `KeyboardDisabled` always (keys forwarded by tmux).
  `dispatch_input`'s `.single()` over `Without<KeyboardDisabled>` then matches
  zero panes and drains — correct here (all keys go to tmux).
- `MouseDisabled` for a pane when: modal (picker / IME / webview-focused /
  window-unfocused), or `CopyModeState`, or a webview is under the cursor.

## Alternatives considered

- **Sink approach 2 — host parallel apply observer + a `pub apply_local_effect`
  helper.** Rejected: the host re-drives the apply loop (duplicates the effect
  dispatch) and panes become host-special-cased rather than first-class crate
  citizens — weaker dedup.
- **Sink approach 3 — `TerminalInputSink` trait/enum in `ozma_tty_engine`.**
  Rejected: `handle.write(&mut pty, …)` is `PtyHandle`-typed across every call
  site; abstracting it is the largest change, and a world-touching sink trait
  fits the ECS / EntityEvent idiom poorly — over-engineered for one extra sink.
- **Mouse option A — local selection only (no app forward).** Rejected after
  research showed app-forward is feasible, iTerm2-standard, and the *cleaner*
  dedup (reuse routing + encoder unchanged, only swap the sink). Option A would
  need a new per-terminal "never forward" override in the crate, which is more
  special-casing for a strictly worse result.

## Change surface

**Crate `ozma_tty_engine` / `ozma_terminal`:**

| File | Change |
| --- | --- |
| `ozma_terminal/src/mouse.rs` | New `TerminalForwardInput` event; `on_terminal_mouse_effects` query → `Option<&mut PtyHandle>` / `Option<&mut Coalescer>`; `Write` → `pty.write` or `trigger(TerminalForwardInput)`; local effects PtyHandle-independent |
| `ozma_terminal/src/lib.rs` | Export `TerminalForwardInput` |
| `ozma_tty_engine` | No change — `TerminalForwardInput` is defined in `ozma_terminal` (the apply observer's crate, the only emitter/consumer); `on_terminal_key_input` / `drain_pty_writes` unchanged |

**Host `src/tmux/`:**

| File | Change |
| --- | --- |
| `render.rs` | `attach_tmux_pane_terminal` adds `OzmaTerminal` |
| `mouse.rs` | Delete local-selection / multi-click copy / hover / hyperlink-open; keep `select-pane`, divider resize, copy-mode mouse, inline-webview mouse; add the claim-suppression for divider/webview |
| `input.rs` | Wheel: keep only copy-mode (`send-keys -X scroll`) and alt-screen (cursor-key) special cases; normal/mouse-mode wheel handled by `ozma_terminal` |
| new module | `forward_pane_input` observer (`On<TerminalForwardInput>` → `send_bytes_command`) + `maintain_tmux_input_gates` |

## Testing

- **Pure (existing, reused):** `encode_protocol_event` SGR bytes; `cell_at_local`
  1-origin coordinates.
- **Sink seam:** a PTY-less `OzmaTerminal` receiving a `Write` effect triggers
  `TerminalForwardInput` with the encoded bytes; the host observer builds the
  expected `send-keys -H -t %id <hex>` (assert against `send_bytes_command`).
- **Regression:** a mouse-off pane still does local selection + clipboard copy;
  a copy-mode pane is `MouseDisabled` and the tmux copy-mode mouse path runs;
  existing `crates/tmux_session/tests/real_tmux_*` stay green.
- **Integration (real tmux, if feasible):** a mouse-mode-on pane forwards a
  click and the in-pane app observes it.
- **DECSET-in-`%output` (closes the one [unverified] premise):** a real-tmux
  test that runs a mouse-enabling program in a pane and asserts the pane's
  detached `TerminalHandle::current_modes()` reflects `MOUSE_MODE` / `SGR_MOUSE`
  (i.e. tmux `%output` carries the app's DECSET `?1000/?1002/?1003/?1006`).
- **Control-channel batching:** multiple `Write` effects in one frame coalesce
  into a single `send-keys -H` (avoid flooding the serialized `-CC` channel).

## Open sub-decisions (confirm during spec review)

- Whether to keep tmux paste-buffer mirroring (`set-buffer`) now that
  `ozma_terminal`'s `Copy` writes the system clipboard and GUI paste already
  uses `send_bytes_command`.
- Divider / webview claim is now part of the design (the pre-gate system in
  §"Residual tmux gestures and arbitration"); the remaining detail is only
  whether the transient per-press claim uses a flag resource or a one-frame
  `MouseDisabled` on the candidate pane.
- Whether the inline-webview claim can reuse the existing suppression path
  unchanged.

## Constraints (repo coding rules)

- Rust 2024 / toolchain 1.95. No `mod.rs`. Comments only `// TODO:` / `// NOTE:`
  / `// SAFETY:`, English. Every externally-`pub` item `///`-documented; module
  files `//!`.
- Imports in one top block; no inline fully-qualified paths.
- Bevy: mutable `SystemParam`s before immutable; whole-system change gates via
  `run_if` not in-body early return; `Plugin::build` one method chain; `Query`
  params descriptive nouns (no `_q`); no manual `set_changed()` /
  `bypass_change_detection()`.
- Visibility minimized; private items last in a block; `#[expect(reason=…)]`
  over `#[allow]`.
- A gather system that ends in `commands.trigger(...)` plus an apply observer is
  the repo idiom — `TerminalForwardInput` follows it.
