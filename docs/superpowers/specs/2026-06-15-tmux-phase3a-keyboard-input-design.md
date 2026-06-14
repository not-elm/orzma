# tmux migration — Phase 3a: Keyboard input + GUI chords + reply routing

Design spec — 2026-06-15
Parent spec: `docs/superpowers/specs/2026-06-14-tmux-multiplexer-migration-design.md` (Phase 3 of the migration phasing)
Worktree/branch: `tmux-phase3` (off `tmux-migration`, which already contains Phase 1 + Phase 2a/2b).

## Goal

Make a tmux pane **interactive from the keyboard**. Phase 2 renders panes and tracks the active pane but forwards no input. Phase 3a captures focused keyboard input and forwards it to tmux so that tmux's key tables (prefix + all bindings) act, intercepting only a small fixed set of ozmux GUI chords. It also forwards terminal replies (DSR/DA) back to tmux and removes keybindings/actions that have no meaning under tmux.

## Decisions settled during brainstorming

1. **Keys route through tmux key tables.** Each forwarded key is sent as `send-keys -K` (process as if typed) so tmux's prefix and key bindings act — ozmux behaves like a real tmux client. (Master-spec decision 4: "tmux owns bindings; ozmux mirrors.")
2. **Phase 3 is split into 3a / 3b / 3c**, each independently mergeable:
   - **3a** (this spec) — keyboard forwarding + GUI-chord interception + reply routing + delete N/A actions.
   - **3b** — re-target the pane/window action handlers to tmux commands (command-echo), renaming the "workspace" concept to "window" to match tmux.
   - **3c** — click-to-focus (`select-pane`) + focus/dim of the active pane.
3. **ozmux "workspace" ≡ tmux "window."** (Switching tmux *sessions* is the session picker, Phase 4.) The rename lands in 3b.
4. **Copy mode forwards to tmux** (`Cmd+U` unbound → users use tmux's `prefix-[`). Clipboard **paste** is kept as a GUI chord; ozmux selection-**copy** is dropped (tmux copy mode yanks).

## Background (verified against the codebase)

- `crates/ozma_tty_engine/src/input_codec.rs` — `encode_key(key, mods, app_cursor) -> Option<Vec<u8>>` is a pure VT-byte encoder for the *old PTY* path. It is **not** reused for tmux: tmux needs key *names* via `send-keys -K`, not VT bytes (master spec §"Keystroke routing").
- `crates/tmux_session/src/model.rs` — `ProjectionModel.active_pane: Option<PaneId>` (set by `%window-pane-changed`). Used for the **paste** GUI chord's `-t` target; forwarded keys go via `send-keys -K` (client-routed, no pane), and replies target their own originating pane.
- `crates/tmux_control/src/transport.rs` — `TmuxHandle::send(&str) -> TmuxResult<CommandId>` sends one control-mode command. `ProtocolClient::send` rejects embedded `\n`/`\r` and frames each write as one command, so each `send-keys` is one line.
- `crates/tmux_session/src/enumerate.rs` — already hosts typed command builders (`refresh_client_command`, `list_windows_command`); the new key/send builders join it. `tmux_control` stays pure transport.
- `crates/ozma_tty_engine` — `TerminalHandle::take_replies() -> Vec<u8>` drains alacritty `PtyWrite` replies; `route_tmux_output` (`src/tmux_render.rs`) currently drains-and-drops them (Phase-2a `// TODO:`).
- Legacy input lives in `src/input.rs` (`OzmuxShortcutPlugin`, `dispatch_focused_key` → config `[shortcuts]` lookup → `ShortcutAction` → `EntityEvent` into `src/action/*`). The config binding/action types are in `crates/configs/src/shortcuts.rs` and `src/action/*`.

## Architecture

A new binary-side plugin owns keyboard handling for the tmux backend; the legacy `dispatch_focused_key` config-binding→multiplexer path is removed (superseded). `ozmux_tmux` stays renderer-free and gains pure key-name + command builders.

```
KeyboardInput (focused) ─▶ gui_chord_intercept ──(GUI chord)──▶ ozmux handler (picker / quit / paste / release-inline-focus)
                                  │
                                  └─(everything else)─▶ bevy_key_to_tmux_name(key, mods)
                                                        ─▶ batch this frame's names
                                                        ─▶ send-keys -K -c <client> <names…>
route_tmux_output ─▶ handle.advance / flush_emit ─▶ handle.take_replies() ─▶ send-keys -H -t <pane> <hex…>
```

### Key-name mapping & command builders (`ozmux_tmux::input`)

- `bevy_key_to_tmux_name(key, mods) -> Option<String>` — pure. Printable chars pass through (with tmux quoting/escaping where needed); named keys map to tmux names (`Enter`, `Escape`, `Tab`, `BSpace`, `Up`/`Down`/`Left`/`Right`, `Home`, `End`, `PgUp`/`PgDn`, `IC`/`DC` for Insert/Delete, `Space`, `F1`–`F12`); modifiers prefix as `C-`/`M-`/`S-`. NOTE: tmux has no Command/Super modifier — `Super`/`Cmd` is treated as GUI-only: a `Super`-modified key is either a known GUI chord (intercepted) or dropped+logged, never forwarded. Do NOT prepend `S-` for character keys (the logical key already yields the shifted glyph; `S-` is only for non-character keys like `S-Up`). Returns `None` for keys with no tmux representation (dropped, logged at debug).
- `send_keys_command(client, names: &[String]) -> String` — builds `send-keys -K -c <client> <name> <name> …` (one batched command per frame).
- `send_bytes_command(pane, bytes: &[u8]) -> String` — builds `send-keys -H -t <pane> <hex> …` for raw bytes (used by reply routing).
- These live in a new `tmux_session::input` module (keyboard command construction is a distinct concern from the `enumerate.rs` list/refresh helpers); a shared tmux-argument quoting helper handles client names, key names, and target ids.

The mapper is driven off Bevy's `KeyboardInput.logical_key` (`Key`) for printable text + named keys, falling back to `key_code` for keys with no logical form; modifiers are read from `ButtonInput<KeyCode>` (as `dispatch_focused_key` does today). The fixed GUI chords match on physical `key_code` (layout-stable).

### Client identity

`send-keys -K` routes through a client's key tables; tmux infers the current client if `-c` is omitted, but whether that resolves to the control client under `tmux -CC` is unverified, so ozmux sends `-c <client>` explicitly. The client name is captured connection-scoped (re-queried on reconnect) and stored on the tmux connection/session state rather than a standalone global resource — sourced either via `display-message -p '#{client_name}'` on attach OR from the `%client-session-changed` notification tmux emits on attach (if the parser surfaces it; otherwise the query is the lower-effort path). ⚠️ **Unverified:** the exact `-K`/`-c` behavior under `tmux -CC` (and whether `-c` is required vs. a single-client default) is the same class of unknown as the Phase-2b `refresh-client` round-trip — it must be confirmed by a gated live-`tmux -CC` integration test before relying on it.

### tmux input plugin (`src/`)

A new `OzmuxTmuxInputPlugin` reads the focused keyboard stream each frame:
1. **GUI-chord interception** — a fixed, hardcoded set, intercepted and NOT forwarded: **open/switch picker** (provisional `Cmd+Shift+P`), **quit** (`Cmd+Q`), **paste** (`Cmd+V` → OS clipboard into the active pane via tmux `paste-buffer`/`send-keys`), **release-inline-focus** (`Ctrl+Shift+Esc`, kept for inline webviews). These are hardcoded for 3a; the `list-keys` keybind mirror stays deferred (chords are a fixed set, not data-driven yet).
2. **Forward** — every other key is mapped via `bevy_key_to_tmux_name`, batched into one `send-keys -K -c <client>` command, and sent through the connection handle. NOTE: `-K` routes to the *client* (tmux's own active pane decides the destination) — it does NOT name a pane, so `active_pane` is not consulted on the forward path. Consecutive keys in a frame batch into a single command to avoid one round-trip per key.

### Reply routing

`route_tmux_output` forwards each pane's `take_replies()` to that pane via `send_bytes_command(pane, &replies)` (`send-keys -H -t <pane> <hex>`), so capability probes (DSR/DA) receive their answers. Closes the Phase-2a TODO. Reply byte runs are chunked under a max command length so a flooded reply queue can't build one huge command. (Paste, by contrast, injects the OS clipboard via `send-keys -l -t <pane> -- <text>` for a one-shot, or `load-buffer` + `paste-buffer -t <pane>` for large/bracketed pastes. Forwarding always uses `-K` key names, never the parent spec's `-H` hex form, which would bypass the key tables.)

## Cleanup in 3a (delete — no tmux meaning)

- **Surface actions + modules:** `NewTerminalSurface`, `CloseSurface`, `FocusSurface`, `BreakSurfaceToPane`, `RenameSurface`, `ListSurfaces` (tmux is one terminal per pane — no "surface"). Delete `src/action/{new_terminal_surface,close_surface,focus_surface}.rs` and the corresponding `ShortcutAction` variants + config `[shortcuts]` fields + `dispatch_focused_key` arms.
- **Copy** action + **EnterCopyMode** binding (Cmd+U forwards to tmux; `CopyModePlugin` left dormant for Phase 5).
- The legacy `dispatch_focused_key` config-binding→multiplexer dispatch path (superseded by the tmux input plugin).
- The orphaned `src/tmux_boot.rs` (dead, not in the module tree — re-introduced by the Phase-2 merge).

## Known gaps carried into 3a (deferred)

- **IME commit** still routes through `read_ime_events → forward_to_active_terminal → TerminalKeyInput` (the old PTY observer), so IME-composed text does NOT reach tmux in 3a (IME revalidation is deferred — parent spec).
- **Mouse wheel/buttons** still query the old multiplexer + `PtyHandle`; nonfunctional under tmux until 3c/later.
- The legacy **clipboard paste** observer writes to `PtyHandle`; the 3a paste GUI chord must use a tmux path (`send-keys -l` / `paste-buffer`), not that observer.

## Left for 3b/3c (NOT touched in 3a)

- The **pane/window actions** (`split_pane`, `focus_pane`, `close_pane`, `swap_pane` modules; `ResizePane`/`ZoomPane` are enum-only with no module; and the workspace→window actions) stay in place but **dormant**: this holds only because the legacy `dispatch_focused_key` is removed and the new plugin forwards their chords to tmux — it is not automatic from leaving the modules in place. 3b re-targets their handlers to tmux commands (command-echo: send `split-window`/`select-pane`/…, let `%layout-change` drive the projection), renames workspace→window, and changes the event schema to target the active pane + direction.
- Click-to-focus and focus/dim are 3c.

## Testing

- **Pure units (`ozmux_tmux`):** `bevy_key_to_tmux_name` table-driven (text, `C-`/`M-`/`S-` combos, named keys, F-keys, unmapped→`None`); `send_keys_command`/`send_bytes_command` (batching, escaping, hex form).
- **Bevy (binary):** headless test that a focused key produces one batched `send-keys` (assert the command string via a fake/recording connection seam) and that a GUI chord is intercepted (not forwarded).
- **Gated real-tmux integration:** attach → forward a key via `send-keys -K -c <client>` → observe the resulting `%output`; plus the `display-message` client-name query. Mirrors the existing `real_tmux_*` gated pattern.

## Deferred scope (unchanged from parent spec)

- `list-keys` keybind mirror (display/awareness) — off the critical path.
- Mouse wheel/scroll, drag-resize; IME revalidation under tmux; hyperlink hover.
- detach/reconnect + idle overlay (Phase 4).
