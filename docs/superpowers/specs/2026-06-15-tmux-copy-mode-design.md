# tmux-driven copy mode (control-mode)

Design spec — 2026-06-15
Worktree/branch: `copy-mode` (off `main`, which already contains the tmux control-mode backend, Phase 3a keyboard input #126/#127/#129).

## Goal

Make tmux's **copy mode usable** inside ozmux, with **every copy-mode keybinding sourced from the user's `tmux.conf`**. The user navigates scrollback, makes selections, searches, and yanks exactly as they have configured tmux (vi or emacs `mode-keys`), and the yanked text lands on the macOS system clipboard.

tmux remains the single source of truth: ozmux **drives** the real server-side copy mode (relays `-X` commands derived from the user's key tables) and **mirrors** it for display (rebuilds the pane grid from `capture-pane` snapshots + cursor/selection overlay). ozmux never reimplements a copy-mode motion.

## Decisions settled during brainstorming

1. **Drive the *real* server-side tmux copy mode** — not an ozmux-native reimplementation. The user gets tmux's actual search, jumps, copy-pipe, paste buffers, and any custom `-X` bindings, behaving identically to a normal tmux client. (User chose this over native emulation, accepting the per-command capture round-trip it costs.)
2. **All copy-mode keybindings come from `tmux.conf`** — read via `list-keys -T copy-mode-vi` and `list-keys -T copy-mode` on attach; the active table is chosen by the `mode-keys` option. ozmux looks up each key in that table and relays the bound `-X` command.
3. **Rendering: live handle keeps streaming + a captured copy-mode grid renders on top.** `%output` does **not** halt during copy mode (verified — see Background), so the pane's live alacritty `TerminalHandle` keeps tracking the program the whole time. While in copy mode the *rendered* `TerminalGrid` is driven from a `capture-pane` of the scrolled mode view (parsed through a scratch VT) with a cursor/selection overlay; on exit the renderer simply switches back to the already-current live grid — no re-snapshot, and `route_tmux_output` is **never** suppressed (suppressing it would drop real output that arrives while scrolled back).
4. **Full first-version scope:** core navigation, visual selection, page/half-page scroll, **jump-to-char (`f F t T`)**, **search (`/ ? n N`)**, **mouse-wheel scroll**, and **mouse drag-select** are all in scope. NOTE: jump-to-char and search are **not** free relays — their default bindings are `command-prompt`-wrapped (they read a character / regex first), so both route through ozmux's prompt overlay (see Prompt-driven bindings).
5. **Clipboard bridge (binding-aware):** copy commands fall into two shapes. `copy-pipe*`/`pipe*` bindings already pipe to an external command (e.g. `pbcopy`) and need no bridge. `copy-selection*` bindings fill a tmux paste buffer **unless** `-P` is given; for those ozmux reads the top buffer (`show-buffer`) and writes it to the macOS clipboard via the existing `Clipboard` resource. The dispatcher classifies which shape a binding is rather than assuming `show-buffer` always holds fresh content.

## Background (verified against the codebase)

- `src/tmux_render.rs` — each `TmuxPane` gets a **display-only** alacritty `TerminalHandle` (`TerminalHandle::detached`). `route_tmux_output` feeds tmux `%output` bytes into the handle (`handle.advance` + `flush_emit`), which builds the `TerminalGrid` the GPU renderer draws. Replies are drained and discarded (tmux already answers device queries). `layout_tmux_panes` sizes the grid from `TmuxPane.dims`.
- `crates/ozma_tty_engine/src/handle.rs` — `TerminalHandle` exposes a full vi/selection/scroll toolkit (`enter_vi_mode`, `exit_vi_mode`, `vi_motion(ViMotion)`, `scroll`, `scroll_page_up/down`, `scroll_to_bottom`, `selection_start_at`, `selection_update_to`, `selection_clear`, `selection_type`, `take_replies`, `emit_pending`/`flush_emit`, `read_geometry`, `resize_grid_only`). This is the engine the **old** `src/ui/copy_mode.rs` drove for the pre-tmux multiplexer.
- `src/ui/copy_mode.rs` — `CopyModePlugin` + `CopyModeState` marker + `EnterCopyModeActionEvent` / `ExitCopyMode` observers exist and are wired in `src/main.rs`, but **nothing triggers them under the tmux backend**. `CopyModeIndicatorPlugin` (`src/ui/copy_mode_indicator.rs`) lights an indicator from `CopyModeState`.
- `crates/tmux_session/src/keybindings.rs` — `KeyBindings` resource holds only the `root` and `prefix` tables (parsed from `list-keys`); `Table` enum has only `Root`/`Prefix` (`parse_binding_line` returns `None` for any other table). `plan_forward(prefix_pending, bindings, key_names) -> Vec<Forwarded>` dispatches: bound key → `Forwarded::Run(command)`, unbound → `Forwarded::Keys` (forwarded to the pane); the prefix key and unmatched prefix keys are swallowed. `list_keys_command(table)` builds `list-keys -T <table>`.
- `crates/tmux_session/src/plugin.rs` — on attach, sends `list-keys -T root`, `list-keys -T prefix`, and `display-message -p '#{prefix} #{prefix2}'`; installs the parsed bindings into `KeyBindings`.
- `src/tmux_input.rs` — `forward_keys_to_tmux` is the single keyboard system. It intercepts a fixed set of GUI chords (`Cmd+Shift+P` picker, `Cmd+Q` quit, `Cmd+V` paste), maps remaining keys to tmux names (`bevy_key_to_tmux_name`), runs `plan_forward`, and sends `Forwarded::Run` commands verbatim / `Forwarded::Keys` as `send-keys -t %pane`. The active pane is `Single<&TmuxPane, With<ActivePane>>`; target string is `format!("%{}", active.id.0)`. Commands go through `connection.client().handle().send(&cmd)`.
- `src/input/mouse_wheel.rs`, `src/input/mouse_buttons.rs`, `src/input/ime.rs` — already query `Query<(), With<CopyModeState>>` to gate behavior; today these gates never fire under tmux because nothing inserts `CopyModeState`. `mouse_wheel.rs` drives alacritty-grid host scrollback.
- `crates/tmux_control/src/transport.rs` — `TmuxHandle::send(&str) -> CommandId` sends one control-mode command; embedded `\n`/`\r` are rejected (one command per line). Command replies arrive as `%begin/%end` blocks surfaced by `tmux_control` (the event pump in `crates/tmux_session/src/event_pump.rs` already correlates a pending `list-keys` command id with its reply lines — `parse_list_keys`).
- `src/clipboard.rs` — `Clipboard` resource (`read()` / write) plus `build_paste_bytes`. Inserted by `CopyModePlugin`.

### Why copy mode is unusable today (root cause)

In `tmux -CC` control mode, copy mode is a **server-side mode whose overlay is never streamed to the client**: tmux does **not** push the copy-mode rendering (cursor, selection, search highlight, scrolled viewport) over the control channel. (It does **not** stop `%output` — verified: a program keeps producing output and tmux keeps streaming `%output` while `pane_in_mode=1`. The data plane is independent of the copy-mode overlay.) Today ozmux's `[` binding resolves to the `copy-mode` command in the `prefix` table, which `forward_keys_to_tmux` runs verbatim — so tmux enters copy mode server-side, but ozmux's rendered grid keeps following the **live tail** (the display-only handle is fed by `%output`) while the user scrolls a server-side viewport that ozmux never reflects; the pane therefore looks frozen, and ozmux's forwarded keys (`send-keys -t %pane`, which target the *program*, not the mode) do nothing visible. This is exactly why iTerm2 and every other `-CC` client replaces tmux copy mode with a client-side mechanism. ozmux's chosen path keeps tmux as the engine but supplies the missing rendering itself.

### Verified mechanism (tmux 3.6a, isolated-socket experiments)

While a pane is in server-side copy mode, the following are observable from the control channel (confirmed empirically):

- **Mode state:** `#{pane_in_mode}` = `1`, `#{pane_mode}` = `copy-mode`.
- **Cursor (visible coords):** `#{copy_cursor_x}` = column, `#{copy_cursor_y}` = row in `0..pane_height` (0 = top of the visible viewport).
- **Selection:** `#{selection_present}` / `#{selection_active}`; `#{selection_start_x}` / `#{selection_end_x}` are visible columns; `#{selection_start_y}` / `#{selection_end_y}` are **absolute grid lines** (history-relative), NOT visible rows.
- **Scroll:** `#{scroll_position}` = lines the viewport is scrolled back from the live bottom; `#{history_size}`, `#{pane_height}`.
- **Content:** `capture-pane -e -p` returns the pane content with SGR escapes. **Plain `capture-pane -p` does NOT follow the scrolled viewport** — it returns the live bottom. To capture the visible copy-mode region:

  ```
  capture-pane -e -p -S {-scroll_position} -E {pane_height - 1 - scroll_position}
  ```

  (Verified: at `scroll_position=12, pane_height=8` this returns the exact 8 scrolled lines.)

- **Coordinate mapping (verified):** the visible viewport's top line is absolute line `history_size - scroll_position`. Therefore:

  ```
  visible_y = absolute_y - (history_size - scroll_position)
  absolute_y = visible_y + (history_size - scroll_position)
  ```

  Selection `*_y` (absolute) maps to a visible row via the first equation; rows outside `0..pane_height` are off-screen and clip. `copy_cursor_y` is already visible — no conversion. (Cross-checked: `history_size=53, scroll_position=3`, `selection_end_y=57 → row 7` = `copy_cursor_y`.)

- **`%output` keeps streaming (verified, corrects an earlier assumption):** driving a real `tmux -CC` client under a PTY, a background writer kept emitting and tmux streamed `%output` the entire time `pane_in_mode=1`. Copy mode is a client-side viewport overlay only; it does not gate the data plane. ⇒ the live handle stays current; nothing to "resume" on exit, and `route_tmux_output` must NOT be suppressed.
- **`%pane-mode-changed` (verified):** tmux emits `%pane-mode-changed %<pane>` in control mode on **both** entry and exit (since 2.5). Payload is just the pane id, so a `#{pane_in_mode}` read still follows it — but it is a reliable event-driven trigger, replacing frame polling.
- **`#{mode-keys}` (verified):** a valid format (`vi`/`emacs`), equal to `show-options -gv mode-keys`; selects `copy-mode-vi` vs `copy-mode`. Fold it into the on-attach `display-message` batch.
- **`capture-pane -M` (documented, to evaluate):** tmux exposes `-M` to capture the **mode screen** when a pane is in a mode — potentially removing the manual scroll-offset math. Try `capture-pane -M -e -p` first; the verified `-S/-E` offset formula above is the proven fallback.

## Architecture

A new binary-side plugin owns copy-mode driving + rendering; `ozmux_tmux` (`crates/tmux_session`) stays renderer-free and gains the copy-mode key tables + a pure copy-mode dispatcher. `CopyModeState` becomes the tmux-driven marker (the existing component + indicator are reused; the enter/exit observers are repurposed so they no longer drive alacritty's own vi-mode).

```
                       ┌─ not in copy mode ─▶ forward_keys_to_tmux (existing: root/prefix dispatch, send-keys)
KeyboardInput ─────────┤      │
                       │      └─(resolved command begins "copy-mode")─▶ ENTER: run `copy-mode[-u]`, insert CopyModeState
                       │
                       └─ in copy mode ─▶ copy_mode_dispatch(key, copy table)
                                            ├─ bound command ─▶ run the bound `send-keys -X …` VERBATIM (target = active pane) — tmux does the work
                                            ├─ command-prompt binding (search / jump) ─▶ open ozmux prompt, then run the binding's inner `send-keys -X …` with the typed arg
                                            └─ cancel / …-and-cancel ─▶ relay, then EXIT

mouse wheel / drag (in copy mode) ─▶ copy_mode_dispatch ─▶ -X scroll-up/down / begin-selection + cursor deltas

%output keeps streaming into the live handle the whole time (copy mode does NOT pause it; route_tmux_output is never suppressed).
on %pane-mode-changed / after each relayed command (coalesced per frame), refresh_copy_mode:
   display-message -p (mode-keys, scroll_position, pane_height, history_size, copy_cursor_*, selection_*, rectangle_toggle, pane_in_mode)
   capture-pane -e -p (-M, or -S/-E offsets)   ─▶ scratch VT parse ─▶ rendered TerminalGrid cells
   cursor + selection ─▶ TerminalGrid vi-cursor/selection fields (coordinate-mapped)
   if a copy-selection* binding ran ─▶ show-buffer ─▶ Clipboard   (copy-pipe* needs no bridge)
   if pane_in_mode == 0 ─▶ EXIT (remove CopyModeState; renderer switches back to the live grid)
```

### Key tables & dispatch (`crates/tmux_session`)

- Extend `Table` with `CopyMode` and `CopyModeVi`; extend `parse_binding_line` to accept `-T copy-mode` / `-T copy-mode-vi`.
- Extend `KeyBindings` with the two copy-mode tables and the active `mode-keys` selection (`copy-mode` vs `copy-mode-vi`).
- On attach (`plugin.rs`), additionally send `list-keys -T copy-mode`, `list-keys -T copy-mode-vi`, and read `mode-keys` via `#{mode-keys}` folded into the existing `display-message` batch (verified a valid format; `show-options -gv mode-keys` not needed); correlate replies in the event pump as the existing list-keys path does.
- New pure dispatcher `copy_mode_dispatch(key_name, &KeyBindings) -> CopyAction`, mirroring `plan_forward`'s style and unit-tested the same way. **The bound copy-table command is already a complete tmux command** — `list-keys` yields e.g. `send-keys -X cursor-down`, and `parse_binding_line` keeps that tail verbatim (`keybindings.rs:169`). So ozmux **runs it verbatim** against the active pane exactly as the root/prefix `Forwarded::Run` path does; it must **NOT** wrap it in another `send-keys -X` (that would emit `send-keys -X send-keys -X cursor-down`). The active pane is kept tmux-current via `select-pane`, so a verbatim run targets the right pane. `CopyAction` only classifies the **side effects** ozmux must add; everything else relays raw (no full tmux-command parser):
  - `CopyAction::Relay` — run the bound command verbatim (the common case: all motions, `begin-selection`, `rectangle-toggle`, scroll, `*`/`#` word search, `;`/`,` jump-again, and any unrecognized-but-bound command).
  - `CopyAction::Prompt { kind, inner }` — the binding is `command-prompt`-wrapped: **search** (`command-prompt -T search … { send-keys -X search-forward … }`) or **jump-to-char** (`command-prompt -1 … { send-keys -X jump-forward … }`). ozmux opens its own prompt and runs the inner `send-keys -X` with the typed text substituted for tmux's `%%`/`%%%` placeholder (see Prompt-driven bindings). Kind/direction parsed from the inner command.
  - `CopyAction::Exit` — the bound command is (or ends in) `cancel`.
  - `CopyAction::Copy { pipes }` — the bound command contains `copy-selection*`/`copy-pipe*`/`pipe*`; `pipes=true` for `copy-pipe*`/`pipe*` or any `-P` form (no clipboard bridge), `false` for buffer-filling `copy-selection*` (bridge via `show-buffer`). `-and-cancel` ⇒ also exit. Classification is shallow (leading command + flags), not a full parse.
  - A key not bound in the copy table is ignored (tmux ignores it too).
- Relay reuses the existing `connection.client().handle().send` path (same as `Forwarded::Run`); the only new builder is the `command-prompt` inner-`send-keys -X` substitution for `CopyAction::Prompt`.

### Entry / exit (`src/tmux_input.rs`)

- **Entry:** when `forward_keys_to_tmux` dispatches a `root`/`prefix` binding whose command **begins with `copy-mode`** (the user's prefix-`[`, a root `WheelUpPane` binding, etc.), intercept it: send the `copy-mode` command (honoring flags like `-u`) to tmux **and** trigger `EnterCopyModeActionEvent` on the active pane (inserts `CopyModeState`). Do not also `Run` it through the normal path (that path already sends it; the change is to *also* set the marker and stop further forwarding).
- **In copy mode:** an early branch in `forward_keys_to_tmux` (active pane has `CopyModeState`) routes keys through `copy_mode_dispatch` instead of `plan_forward`. GUI chords (`Cmd+V`, `Cmd+Q`, picker) still intercept first. Keys are mapped to tmux names by the existing `bevy_key_to_tmux_name`.
- **Exit:** a `CopyAction::Exit` / `…-and-cancel` copy relays, then removes the marker. Exit is **driven by the `%pane-mode-changed` notification** (verified: tmux emits it in control mode on entry AND exit, since 2.5) followed by a `#{pane_in_mode}` read — this also catches mode exits ozmux did not initiate (the user's `q`, a self-cancelling command) without polling every frame. Removing `CopyModeState` switches the renderer back to the live grid; because **`%output` was never paused**, the live grid is already current — nothing to resume and no re-snapshot. (`%pane-mode-changed` must be surfaced through `tmux_control_parser` → event pump; see Open question 4.)

### Rendering & coordinate mapping (`src/tmux_copy_mode.rs`, new)

`%output` keeps streaming into the pane's live handle throughout copy mode (verified), so `route_tmux_output` is **never suppressed** — suppressing it would drop program output that arrives while the user is scrolled back. The copy-mode view is a separate *render* path: a `refresh_copy_mode` system (triggered by `%pane-mode-changed` and after each relayed command, coalesced per frame — not a free-running poll) drives what the renderer shows for that pane:

1. Query the format variables in one `display-message -p` round-trip: `mode-keys pane_in_mode scroll_position pane_height history_size copy_cursor_x copy_cursor_y selection_present rectangle_toggle selection_start_x selection_start_y selection_end_x selection_end_y`.
2. If `pane_in_mode == 0` → exit (above).
3. Capture the mode view. **Prefer `capture-pane -M -e -p -t %pane`** (tmux's documented mode-screen capture — avoids scroll-offset math); if `-M` does not reproduce the scrolled view with attributes, fall back to the **verified** `capture-pane -e -p -S {-scroll_position} -E {pane_height-1-scroll_position} -t %pane`. (Confirm `-M` on the live-tmux test.) Skip the capture entirely when `scroll_position` and the captured region are unchanged since the last refresh (pure cursor/selection motions only move the overlay).
4. Rebuild the rendered `TerminalGrid` from the captured bytes via a **single app-shared scratch VT** (one detached `TerminalHandle`, reset + `advance(capture)` per refresh — refreshes are serial, so one VT suffices; not one per pane) so SGR colors survive; copy its grid into the pane's `TerminalGrid`. The pane's *live* handle is untouched and keeps streaming.
5. Map the cursor + selection into the **existing `TerminalGrid` vi-cursor / selection fields** the renderer already draws (Codex [code-verified]; confirm the exact field names — add new schema fields only if rectangle mode forces it): cursor at `(copy_cursor_x, copy_cursor_y)`; selection from start/end mapped to visible rows via the verified equation, clipped to `0..pane_height`, line vs block from `#{rectangle_toggle}`.

**Reply correlation.** Per-key `display-message` / `capture-pane` / `show-buffer` replies need routing the existing event pump does not provide — its pending fields are fixed (`enumerate.rs:143-160`: root/prefix/prefix-options/seed). Add a **copy-mode transaction map keyed by `CommandId`** carrying `{kind, pane, generation}`, so a reply superseded by a newer refresh is dropped rather than applied out of order, and the `show-buffer` clipboard read is sequenced after its copy relay. This map lives with `refresh_copy_mode`.

> **Rendering decision (settled by review):** do NOT add a bespoke `CopyOverlay` schema unless rectangle mode forces it — populate the `TerminalGrid` vi-cursor/selection fields the renderer already composites. The live handle stays untouched and keeps streaming; the renderer chooses captured-grid vs live-grid per pane on the `CopyModeState` marker. (The earlier A/B fork is resolved: live handle untouched, render from capture, reuse existing grid fields.)

### Prompt-driven bindings — search + jump-to-char (`src/ui/` — copy-mode prompt input)

tmux's `command-prompt` (used by the default `/`, `?`, `f`, `F`, `t`, `T` bindings) is a status-line prompt that is **not streamed** to a control client, so ozmux supplies it. The dispatcher returns `CopyAction::Prompt { kind, inner }` for any `command-prompt`-wrapped binding:

- **Search** (`/`, `?` → `command-prompt -T search … { send-keys -X search-forward … }`): ozmux opens a one-line input (reusing the palette/picker UI under `src/ui/`); on submit it runs the binding's **inner** command with the typed regex substituted for tmux's `%%`/`%%%` placeholder — so the user's exact inner command (including flags/quoting) runs. On cancel, nothing. `n`/`N` relay verbatim as `search-again`/`search-reverse`. The word-search `*`/`#` bindings (`send-keys -FX search-… "#{copy_cursor_word}"`) carry no prompt — they relay verbatim.
- **Jump-to-char** (`f`, `F`, `t`, `T` → `command-prompt -1 … { send-keys -X jump-forward … }`): identical mechanism with a **single-character** prompt; `;`/`,` (`jump-again`/`jump-reverse`) relay verbatim.
- The prompt owns the keyboard while open (the existing `picker.open` / IME gating pattern in `forward_keys_to_tmux` is the model). **Out of scope for v1:** incremental search highlight-as-you-type (would need a relay + refresh per keystroke); v1 runs the inner command on submit. (Noted explicitly so the omission is not read as "covered".)

### Mouse — wheel + drag-select (`src/input/mouse_wheel.rs`, `src/input/mouse_buttons.rs`)

- **Wheel:** while `CopyModeState` is set, the alacritty-grid scrollback path in `mouse_wheel.rs` is suppressed; wheel notches relay the copy table's `WheelUpPane`/`WheelDownPane` bindings (typically `scroll-up`/`scroll-down`, possibly `halfpage-*`) via `copy_mode_dispatch` → `send-keys -X`. Entry via a root-table `WheelUpPane` → `copy-mode -e` binding flows through the normal copy-mode entry interception.
- **Drag-select:** there is **no `-X` primitive to set the copy cursor to an absolute `(x,y)`**, so ozmux positions it by **delta motions off the read-back cursor**, run as a small **async state machine** (the control connection is request/reply with later-arriving replies — issuing the next delta off a stale cursor races tmux's clamping). On click/drag-target `(col,row)`: issue `send-keys -X -N {Δcol} cursor-{left|right}` + `-N {Δrow} cursor-{up|down}` (Δ from the last cursor read), **await the `copy_cursor_*` readback** through the transaction map, then recompute against the newest pointer position and repeat until converged (short lines clamp `copy_cursor_x`, so never assume the requested delta landed). For the vertical axis, evaluate `send-keys -X -N {abs} goto-line` — one command to an absolute history line via `absolute_y = visible_y + (history_size - scroll_position)` — instead of N `cursor-up/down`; confirm `goto-line`'s argument semantics on the live-tmux test. Sequence: position → `begin-selection` → drag repositions → release relays `copy-selection` (+ clipboard bridge per the binding shape). Click maps to a visible cell via the same cell metrics `layout_tmux_panes` uses.

### Clipboard bridge (`src/tmux_copy_mode.rs`)

After a `CopyAction::Copy { pipes: false }` relay (a buffer-filling `copy-selection*` without `-P`), ozmux runs `show-buffer` and writes the result to the `Clipboard` resource. `copy-pipe*`/`pipe*` bindings (`pipes: true`) already pipe to an external command (e.g. `pbcopy`) and are **not** bridged — `show-buffer` would return stale content or nothing. `-P` (suppress buffer) is treated as `pipes`-like (no bridge). `Cmd+V` paste (existing GUI chord) then pastes whatever landed on the system clipboard. NOTE: `show-buffer` returns the *top* buffer, so the bridge read must be **sequenced after the copy relay's reply** (via the transaction map), not raced against it.

### Module decomposition

| Unit | Responsibility |
|---|---|
| `crates/tmux_session/keybindings.rs` | `Table::{CopyMode,CopyModeVi}`, parse copy tables, `KeyBindings` copy tables + `mode-keys`, pure `copy_mode_dispatch` → `CopyAction`. Renderer-free; unit-tested. |
| `crates/tmux_session/plugin.rs` | Fetch `list-keys -T copy-mode[-vi]` + `mode-keys` on attach; install into `KeyBindings`. |
| `crates/tmux_session/{input,enumerate}.rs` | `send-keys -X -t %pane` builder; `capture-pane`/`show-buffer`/`display-message` copy-mode command builders (pure). |
| `src/tmux_input.rs` | Copy-mode key branch + `copy-mode` entry interception + exit triggering. |
| `src/tmux_copy_mode.rs` (new) | `refresh_copy_mode` (triggered by `%pane-mode-changed` + post-relay, coalesced): format-var query → scratch-VT grid rebuild → `TerminalGrid` cursor/selection → binding-aware clipboard bridge → exit. Owns the shared scratch VT + the **`CommandId`→`{kind,pane,generation}` transaction map** for per-key reply correlation. |
| `src/ui/copy_mode.rs` | `CopyModeState` as tmux-driven marker. **Do NOT reuse the `EnterCopyModeActionEvent`/`ExitCopyMode` observers** — they query `(&mut TerminalHandle, &mut Coalescer)` and tmux panes have no `Coalescer` (`tmux_render.rs` attaches only `TerminalHandle` + render bundle), so they silently no-op. Add tmux-specific systems that only insert/remove `CopyModeState`. |
| `src/ui/` (prompt input) | One-line copy-mode prompt overlay (search regex + jump single-char). |
| grid schema (existing `TerminalGrid`) | Reuse the renderer's existing vi-cursor/selection fields for the copy overlay; add new fields only if rectangle selection requires them. |
| `src/input/mouse_*.rs` | Suppress alacritty wheel scrollback while in copy mode; route wheel + drag to the copy-mode relay. |

## Testing strategy

- **Pure unit tests** (no tmux), in the style of the existing `keybindings.rs` / `tmux_render.rs` tests:
  - `parse_binding_line` accepts `-T copy-mode` / `-T copy-mode-vi`.
  - `copy_mode_dispatch`: **verbatim `Relay`** of a bound `send-keys -X cursor-down` (asserting it is NOT re-wrapped into `send-keys -X send-keys -X …`); `Prompt` from a `command-prompt -T search { send-keys -X search-forward … }` and from a `command-prompt -1 … { send-keys -X jump-forward … }` (with placeholder substitution); `Exit` from `cancel`; `Copy{pipes:false}` from `copy-selection-and-cancel` vs `Copy{pipes:true}` from `copy-pipe … pbcopy`; ignore for unbound.
  - Coordinate mapping: `visible_y = absolute_y - (history_size - scroll_position)`, capture `-S/-E` offset derivation, clipping of off-screen selection rows — table-driven against the verified numbers.
  - Entry interception: a dispatched command beginning `copy-mode` yields the enter path, not a verbatim-run-only.
- **Gated real-tmux integration test** (`#[ignore]`, matching `display_only_pane_does_not_inject_phantom_device_replies` in `src/tmux_render.rs`): spawn `tmux -CC`, enter copy mode, relay `cursor-up`/`begin-selection`/`cursor-down`, run a `refresh_copy_mode` cycle, and assert (a) the rebuilt `TerminalGrid` matches the scrolled `capture-pane`, (b) the overlay cursor/selection match the format-variable coordinates, (c) `copy-selection` lands text in the `Clipboard`.

## Open questions / risks

1. **Refresh latency.** One `display-message` + one `capture-pane` round-trip per key over the local control socket. Expected sub-few-ms; acceptable for navigation. If perceptible, coalesce multi-key frames into one refresh (already the plan) and consider skipping refresh for pure-motion keys until the frame settles.
2. **`mode-keys` source — RESOLVED.** `#{mode-keys}` is a valid format (verified = `vi`); read it in the on-attach `display-message` batch (no `show-options`). It can change at runtime (`set -g mode-keys`), so re-read on `%pane-mode-changed`/entry rather than caching once.
3. **Rectangle selection rendering.** Confirm `#{rectangle_toggle}` reflects block-mode state so block selections highlight correctly; fall back to line selection if the existing `TerminalGrid` selection field cannot express a rectangle in v1.
4. **`%pane-mode-changed` notification — RESOLVED, now load-bearing.** Verified: tmux emits `%pane-mode-changed %<pane>` in control mode on entry AND exit (since 2.5). Payload is only the pane id, so a `#{pane_in_mode}` read still follows for the new state, but it replaces frame polling as the refresh/exit trigger. RISK: confirm `tmux_control_parser` surfaces it (it may currently fall into `ControlEvent::Unknown`) — if not, parser work is a prerequisite.
5. **History backfill.** `capture-pane -S` can read tmux's full server-side history (deeper than ozmux's alacritty scrollback), which is a benefit of driving real tmux — but very large `-S` ranges are unnecessary since we only capture the visible window. No backfill work needed.
6. **Multiple panes in copy mode.** Each pane carries its own `CopyModeState`; the transaction map is keyed per pane (the shared scratch VT is reused serially). v1 only the active pane is keyboard-driven, but rendering must handle any pane left in copy mode.
7. **`capture-pane -M` vs `-S/-E`.** `-M` is tmux's documented mode-screen capture and would remove the scroll-offset math; the `-S/-E` formula is empirically verified. Try `-M` first on the live-tmux test (does it carry the scrolled position + cell attributes?); keep `-S/-E` as the proven fallback.
8. **Reply-correlation refactor.** The `CommandId`-keyed transaction map is new infrastructure on top of the event pump's fixed pending fields (`enumerate.rs`). Confirm the pump can host a generic keyed map (with generation-based stale-drop) without disturbing the existing typed-command correlation — this is the largest plumbing risk in the plan.
