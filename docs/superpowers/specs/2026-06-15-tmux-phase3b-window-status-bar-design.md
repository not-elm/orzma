# tmux migration — Phase 3b: window status bar + action cleanup

Design spec — 2026-06-15
Parent spec: `docs/superpowers/specs/2026-06-14-tmux-multiplexer-migration-design.md` (Phase 3 of the migration phasing)
Sibling spec: `docs/superpowers/specs/2026-06-15-tmux-phase3a-keyboard-input-design.md` (Phase 3a, already on this branch)
Worktree/branch: `tmux-phase3` (extends PR #122 into `tmux-migration`).

## Goal

Give the ozmux GUI a tmux-style **window status bar** at the bottom of the
window, mirroring tmux's native `status-line` layout, and remove the
pane/window keyboard actions that became dead once Phase 3a forwards all keys to
tmux. After 3b the user can **see their tmux windows and click to switch**
(`select-window`), the GUI's overall layout matches tmux (terminal area above a
one-row status bar), and the obsolete ozmux-native pane-op action/binding
machinery is gone.

## Decisions settled during brainstorming

1. **tmux owns all keyboard bindings (forward-only).** Phase 3a forwards every
   key via `send-keys -K -c <client>`, so tmux's own bindings (`C-b %`,
   `C-b "`, `C-b o`, `C-b n`, …) already act and the resulting
   `%layout-change` drives the projection. ozmux adds **no** parallel
   pane-op keyboard chords. Consequently the pane-op action **handlers**
   (`split_pane`/`focus_pane`/`swap_pane`/`close_pane`, plus the workspace
   New/Focus actions) have no trigger and are **deleted**, not re-targeted.
2. **The window UI is a bottom status bar, not top tabs.** It mirrors tmux's
   `status-line` structure: a single full-width row at the very bottom.
3. **Status bar content: session name + window list only.** Left: `[<session
   name>]`. Then one entry per window, `<window_index>:<window_name>`, with the
   active window highlighted. **No** clock, host, or per-window flag glyphs
   (`-`/`Z`/`#`) — only the active highlight. Clicking a window entry switches
   to it.
4. **Layout reservation: rely on tmux's own status-line reservation (method
   B).** tmux's `status` option is left **on** (its default). tmux reserves the
   status row in its layout math, so the panes it reports occupy `H-1` rows;
   ozmux renders its own status bar in that bottom row. ozmux does **not**
   subtract a row in `sync_client_size`. ⚠️ This depends on `tmux -CC`
   actually reserving the status row in the layout it reports — a **verify-live
   unknown** (see Risks). Method A (status off + `sync_client_size` sends
   `rows-1`) is the documented fallback if the reservation does not happen.
5. **Switching is command-echo.** A window-entry click sends
   `select-window -t @<id>`; ozmux does not mutate the projection directly —
   the resulting tmux notification flips the active window and the bar
   re-highlights. Same model as 3a reply routing and the master spec's
   "tmux owns state; ozmux mirrors."
6. **Naming.** New tmux-facing code uses "window" throughout. The old
   `ozmux_multiplexer` "workspace"/"surface" vocabulary is **not** renamed —
   that crate is removed wholesale in Phase 5; renaming doomed code is wasted
   churn.

## Background (verified against the codebase)

- `crates/tmux_session/src/model.rs` — `WindowModel { id: WindowId, active:
  bool, name: String, panes: Vec<PaneModel> }` and `ProjectionModel { session:
  Option<SessionId>, windows: Vec<WindowModel>, active_pane: Option<PaneId> }`.
  **Neither carries a window index, and `session` is a `SessionId` (a number),
  not the session name.** Both must be added (§Architecture).
- `crates/tmux_session/src/enumerate.rs` — `LIST_WINDOWS_FORMAT =
  "#{window_active}\t#{window_id}\t#{window_layout}\t#{window_visible_layout}\t#{window_name}"`
  (no `#{window_index}`); `WindowRow { id, active, name, layout }`;
  `parse_window_rows`. This is where the index field is added. The module also
  hosts the typed command builders (`list_windows_command`,
  `refresh_client_command`, `client_name_command`); the new
  `select_window_command` / `session_name_command` builders join it.
- `crates/tmux_session/src/components.rs` — `TmuxSession { id }`,
  `TmuxWindow { id, active, name }`, `TmuxPane { id, … }`. `TmuxWindow` gains
  `index`.
- `crates/tmux_session/src/plugin.rs` — `drain_tmux_events` sends
  `client_name_command()` on the Attached transition and now **retries** it
  while unresolved (3a fix). The session-name query follows the **same
  pattern** (attach send + retry + `take_*` reducer). `connection.set_client_name`
  caches the client name on `TmuxConnection`; a `session_name` slot is added
  alongside it (or on `ProjectionModel`).
- `crates/tmux_session/src/reconcile.rs` — syncs `TmuxWindow`/`TmuxPane`
  entities (parented by `ChildOf`) to `ProjectionModel`. It carries the new
  `index` onto `TmuxWindow`.
- `src/tmux_render.rs` — `attach_tmux_window_container` parents the tmux window
  container under `WorkspaceUiRoot` (`src/ui`); `layout_tmux_panes` positions
  panes from `TmuxPane.dims` × cell metrics; `sync_client_size` sends
  `refresh-client -C <cols>,<rows>` computed from the primary window's physical
  size (full height — **unchanged** under method B). The status bar mounts as
  another child of `WorkspaceUiRoot`, pinned to the bottom.
- `src/action.rs` + `src/action/{split_pane,focus_pane,swap_pane,close_pane,
  workspace}.rs` — the pane/window action handlers, all built on
  `ozmux_multiplexer` (`MultiplexerCommands`). Verified that the only non-test
  triggers of these `*ActionEvent`s were the deleted-in-3a `dispatch_focused_key`
  and the test-only `NewWorkspaceActionEvent` at `src/ui/workspace.rs:591`.
  Under forward-only they have **zero** production triggers → deleted.
- `crates/configs/src/shortcuts.rs` — `Bindings` (under `#[serde(rename_all =
  "kebab-case", default, deny_unknown_fields)]`) still carries `close_pane`,
  `focus_pane_*`, `split_pane_*`, `swap_pane_*`, `new_workspace`,
  `focus_workspace_*` and the 6 already-deprecated surface keys. The pane/window
  keys join the deprecated (accept-and-ignore) set; `Bindings::iter()`,
  `lookup()`, `Default`, and conflict-validation drop them.
- `src/ui/tab_input.rs` + `src/ui/workspace.rs` — the **old-mux** top-tab chrome.
  Dormant in tmux mode (the tmux reconcile never populates old-mux workspaces).
  Left in place for Phase 5 removal unless it renders a visible empty strip.

## Architecture

```
attach ──▶ display-message '#{session_name}'  (send + retry, like client_name)
              ──▶ ProjectionModel.session_name : Option<String>
       ──▶ list-windows -F (… #{window_index})
              ──▶ WindowRow{index,…} ──▶ WindowModel{index,name,active,…}
              ──▶ reconcile ──▶ TmuxWindow{ index, name, active }  (ChildOf session)

status-bar rebuild (on projection change)
   reads session_name + TmuxWindow{index,name,active}
   renders:  [<session_name>]   <i>:<name>   <i>:<name>*   …      (active highlighted)

click a window entry ──▶ select_window_command(id) ──▶ connection.handle().send
   ──▶ tmux switches ──▶ %window-pane-changed / active flip ──▶ projection
   ──▶ TmuxWindow.active reconciled ──▶ bar re-highlights        (command-echo)
```

### Projection schema additions (`crates/tmux_session`)

- `LIST_WINDOWS_FORMAT`: append `\t#{window_index}`. `WindowRow` gains
  `index: u32`; `parse_window_rows` parses the new trailing field (update the
  field-count and the doc comment). `WindowModel` gains `index: u32`
  (reducer `seed_from_rows` / `apply_event` carry it). `TmuxWindow` gains
  `index: u32` (reconcile sets it).
- Session name: `ProjectionModel` gains `session_name: Option<String>`. A new
  pure reducer `take_session_name(pending, events)` mirrors `take_client_name`
  (`crates/tmux_session/src/event_pump.rs`). `EnumerationState` gains
  `session_name_pending: Option<CommandId>`. `drain_tmux_events` sends
  `session_name_command()` on attach and **retries** while
  `session_name.is_none() && pending.is_none()` (identical structure to the 3a
  client-name retry). Builder `session_name_command() -> "display-message -p
  '#{session_name}'"` (`pub(crate)`, positional form — same family as
  `client_name_command`, NOT `-F`).
- Builder `select_window_command(id: WindowId) -> String` →
  `select-window -t @<id>` (`pub`, exported for the binary's click system;
  reuse the existing arg-quoting helper). The `@<id>` target is structurally
  safe (numeric) like the `%<pane>` form in reply routing.

### Status bar UI (`src/ui/tmux_status_bar.rs`, new)

- `StatusBar` marker on a Bevy UI node: child of `WorkspaceUiRoot`, full width,
  height = one cell row (`TerminalCellMetricsResource` line height), pinned to
  the bottom (`position_type: Absolute`, `bottom: 0`, or last flex child —
  whichever matches the existing root layout). Background from the ozmux theme
  (`src/theme`); the look is ozmux-themed, the *structure* mirrors tmux.
- A rebuild system, gated `run_if(resource_exists_and_changed::<ProjectionModel>)`
  (or change-detected on `TmuxWindow`/session-name), despawns and rebuilds the
  bar's children: a left `[<session_name>]` text node (empty/elided until
  `session_name` resolves), then one `WindowEntry` button per window in
  `index` order, labelled by a pure `window_label(index, name) -> String`
  (`"<index>:<name>"`). The active window's entry uses a highlight
  (font weight + background) consistent with the active-pane treatment.
- A click system (in `InputPhase::Dispatch`, like `drive_tab_clicks`, so the
  switch is visible the same frame): on a `WindowEntry`
  `Interaction::Pressed`, send `select_window_command(entry.window_id)` via
  `connection.client().handle()`, warn on send error. No direct projection
  mutation (command-echo). A hover-cursor system (pointer over entries) mirrors
  `tab_hover_cursor`.
- `OzmuxTmuxStatusBarPlugin` registers the spawn, rebuild, click, and hover
  systems and is added in `src/main.rs` near the other tmux plugins.

### Layout reservation (method B)

tmux `status` stays on (default). tmux reserves the status row in its layout, so
the pane layout it reports sums to `H-1` rows; ozmux's `layout_tmux_panes`
positions those panes from the top, leaving the bottom cell row free, where the
`StatusBar` node is drawn. `sync_client_size` is **unchanged** (sends the full
height; tmux does the `-1`). No tmux option is mutated by ozmux.

- **Fallback (method A), if the verify-live check shows tmux -CC does not
  reserve the row:** set the session `status off` on attach, and change
  `sync_client_size` to compute `rows` from `physical_height - line_height`
  (reserve one row) so the panes fit above the bar. This is a localized change
  behind the same `StatusBar` node; the plan keeps the bar's pixel reservation
  independent of which method is active.

### Cleanup (deletions)

- Delete `src/action/{split_pane,focus_pane,swap_pane,close_pane}.rs` and the
  workspace New/Focus action handlers in `src/action/workspace.rs`; remove their
  sub-plugin registrations from `src/action.rs` (and drop now-empty plugins).
  Delete the `ShortcutAction` variants `SplitPane`/`FocusPane`/`SwapPane`/
  `ClosePane`/`NewWorkspace`/`FocusWorkspace` and their supporting enums
  (`Direction`/`SplitDirection`/`SwapOffset`/`WorkspaceOffset`) **iff** they have
  no remaining referent.
- In `crates/configs/src/shortcuts.rs`: move `close_pane`, `focus_pane_*`,
  `split_pane_*`, `swap_pane_*`, `new_workspace`, `focus_workspace_*` into the
  deprecated **accept-and-ignore** set (`#[serde(default, skip_serializing,
  deserialize_with = "deser_chord_or_unbind")]`, set to `None` in `Default`,
  excluded from `iter()`), exactly like the 3a surface keys; add a back-compat
  test. After this, `Bindings::iter()`/`lookup()` may be empty or near-empty —
  if **no** active binding remains, delete the now-dead `lookup`/conflict
  machinery too (and its callers) rather than leave dead code.
- The old-mux top-tab chrome (`src/ui/workspace.rs`, `src/ui/tab_input.rs`) is
  **not** deleted in 3b (Phase 5 removes the old multiplexer). If it renders a
  visible empty strip in tmux mode, hide its root node; otherwise leave it.

### Naming

New status-bar code and the new builders use "window". The `ozmux_multiplexer`
crate keeps "workspace"/"surface" until Phase 5.

## Testing

- **Pure units (`ozmux_tmux`):** `parse_window_rows` with the new
  `#{window_index}` field (and a malformed/short row); `select_window_command`
  / `session_name_command` builders; `take_session_name` (matching id, failed
  reply, empty/whitespace output — mirrors the `take_client_name` tests).
- **Pure unit (binary):** `window_label(index, name)` formatting (incl. an empty
  name, a name needing no escaping).
- **Bevy headless (binary):** seed a `ProjectionModel` (session_name + two
  windows, one active) → run the rebuild system → assert the bar has a session
  node + two `WindowEntry`s with the right labels and the active highlight on
  the right one; simulate a `WindowEntry` press → assert
  `select-window -t @<id>` was sent (recording/fake connection seam, as the
  existing tmux render tests do).
- **Config back-compat (`ozmux_configs`):** a config carrying the removed
  pane/window keys still parses and they don't enter `iter()`.
- **Gated real-tmux (`crates/tmux_session/tests/real_tmux_window.rs`, ignored):**
  attach → assert `session_name` resolves and the window list carries indices →
  create a second window (`new-window`) → click-equivalent
  `select-window -t @<id>` → observe the active window flip in the projection.
  Mirrors the existing `real_tmux_*` gated pattern.
- **Layout (manual / gated):** confirm panes occupy the area above the bar and
  the bar is fully visible (method B reservation). This is the verify-live gate.

## Risks / unknowns (verify-live)

- **tmux -CC status reservation (method B):** whether `tmux -CC` with `status
  on` reports a pane layout that excludes the status row. If it does not, panes
  overlap the bar and we switch to method A (status off + `rows-1` in
  `sync_client_size`). Confirm by manual GUI check / the gated layout test
  before relying on it.
- **session_name via `display-message` in -CC:** same class as the
  already-proven `client_name` query; low risk. Session rename mid-session
  updates only on the next attach/retry (acceptable; live rename tracking
  deferred).
- **Empty/near-empty `Bindings`:** confirm removing the dead binding machinery
  doesn't break the `/configs/shortcuts` HTTP wire shape or the daemon binding
  count consumer (if any) — grep before deleting `lookup`.

## Deferred scope (later phases)

- Per-window flags (`-` last, `Z` zoomed, `#` activity), status-left/right
  customization, clock/host — cosmetic, off the critical path.
- Live session-rename / window-rename tracking beyond attach-time + retry.
- Click-to-focus on **panes** + focus/dim (Phase 3c).
- Removal of the old `ozmux_multiplexer` crate and its dormant chrome (Phase 5).
- `list-keys` keybind mirror (display/awareness only).
