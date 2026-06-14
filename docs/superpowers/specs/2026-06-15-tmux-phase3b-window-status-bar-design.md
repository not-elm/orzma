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
4. **Layout reservation: ozmux reserves the row (method A).** `tmux -CC` does
   **not** reserve a status row for control clients — verified live against
   `tmux 3.6b -CC`: `refresh-client -C 80,24` reports panes at the full
   `80x24` regardless of the `status` option. So ozmux owns the reservation:
   it reserves the bottom cell row in its Bevy layout for the status bar and
   makes `sync_client_size` send `rows-1` (one fewer row than the window holds)
   so tmux lays out panes into the area above the bar. The tmux `status`
   option is irrelevant in `-CC` and is left untouched. (Method B — relying on
   tmux to reserve the row — was the original choice but is non-viable; the
   live probe is decisive.)
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
  **`WindowModel` carries no window index** (added in §Architecture). `session`
  is a `SessionId` (a number), not the session name — but the session **name**
  is already available: `ProjectionModel::apply_event` (model.rs ~97-100)
  already matches `ControlEvent::SessionChanged { session, name }` on the
  `%session-changed` notification tmux emits at attach, and currently
  **discards `name`**. Capturing it there yields `session_name` with no new
  command, pending slot, reducer, or retry — and it auto-updates on live
  rename (§Architecture).
- `crates/tmux_session/src/enumerate.rs` — `LIST_WINDOWS_FORMAT =
  "#{window_active}\t#{window_id}\t#{window_layout}\t#{window_visible_layout}\t#{window_name}"`
  (no `#{window_index}`); `WindowRow { id, active, name, layout }`;
  `parse_window_rows`. This is where the index field is added. The module also
  hosts the typed command builders (`list_windows_command`,
  `refresh_client_command`, `client_name_command`); the new
  `select_window_command` builder joins it.
- `crates/tmux_session/src/components.rs` — `TmuxSession { id }`,
  `TmuxWindow { id, active, name }`, `TmuxPane { id, … }`. `TmuxWindow` gains
  `index`.
- `crates/tmux_session/src/plugin.rs` — `drain_tmux_events` sends
  `client_name_command()` on the Attached transition and now **retries** it
  while unresolved (3a fix). The session name does **not** need this machinery
  — it comes from the `%session-changed` notification (above), captured in the
  existing `apply_event` arm. No new command/pending/retry is added.
- `crates/tmux_session/src/reconcile.rs` — syncs `TmuxWindow`/`TmuxPane`
  entities (parented by `ChildOf`) to `ProjectionModel`. It carries the new
  `index` onto `TmuxWindow`.
- `src/tmux_render.rs` — `attach_tmux_window_container` parents the tmux window
  container under `WorkspaceUiRoot` (`src/ui`); `layout_tmux_panes` positions
  panes from `TmuxPane.dims` × cell metrics; `sync_client_size` sends
  `refresh-client -C <cols>,<rows>` computed from the primary window's physical
  size — changed under method A to reserve one row for the bar (send `rows-1`).
  The status bar mounts under `UiRoot` (the Column parent of `WorkspaceUiRoot`),
  **not** inside `WorkspaceUiRoot` (see §Status bar UI).
- `src/action.rs` + `src/action/{split_pane,focus_pane,swap_pane,close_pane,
  workspace}.rs` — the pane/window action handlers, all built on
  `ozmux_multiplexer` (`MultiplexerCommands`). `dispatch_focused_key` was
  already deleted in 3a (only a doc-comment reference remains in
  `tmux_input.rs`). The remaining `*ActionEvent` triggers are **all test-only**:
  `NewWorkspaceActionEvent`/`FocusWorkspaceActionEvent` in
  `src/action/workspace.rs` tests and `src/ui.rs:815`, and
  `SplitPaneActionEvent` in a `mouse_buttons.rs` test (~:1171). Under
  forward-only they have **zero** production triggers → deleted (along with
  those test triggers).
- `crates/configs/src/shortcuts.rs` — `Bindings` (under `#[serde(rename_all =
  "kebab-case", default, deny_unknown_fields)]`) still carries `close_pane`,
  `focus_pane_*`, `split_pane_*`, `swap_pane_*`, `new_workspace`,
  `focus_workspace_*` and the 6 already-deprecated surface keys. The pane/window
  keys join the deprecated (accept-and-ignore) set; `Bindings::iter()`,
  `lookup()`, `Default`, and conflict-validation drop them. ⚠️ `iter().count()`
  is asserted in `crates/configs/tests/load.rs` (3 sites) and
  `validate_no_conflicts` has a **live** caller at `crates/configs/src/raw.rs:63`
  — both must be updated, not orphaned, if `iter()` empties out (§Cleanup).
- The shortcut-support enums `Direction` / `SplitDirection` / `SwapOffset` /
  `WorkspaceOffset` exist in **two** crates: `crates/configs/src/shortcuts.rs`
  (deletable here) **and** `ozmux_multiplexer` (`SwapOffset`, `PaneDirection`,
  …, still used internally — kept until Phase 5). Delete only the `shortcuts.rs`
  copies.
- `src/ui/status_bar.rs` + `src/ui/status_bar_sync.rs` — an **existing**
  status bar (`StatusBarRoot`, old-mux workspace chips) that mounts under
  `UiRoot` (a `FlexDirection::Column` node, `src/ui/root.rs` ~43-50) and
  rebuilds on `WorkspaceMarker`/`AttachedWorkspace` change. Dormant in tmux
  mode (renders empty) but still occupies a `UiRoot` row — the new tmux window
  bar **reuses/replaces** it rather than adding a second bar (§Architecture).
- `src/ui/tab_input.rs` + `src/ui/workspace.rs` — the **old-mux** top-tab chrome.
  Dormant in tmux mode. Left in place for Phase 5 removal unless it renders a
  visible empty strip.

## Architecture

```
attach ──▶ %session-changed $id <name>  ──▶ apply_event captures name
              ──▶ ProjectionModel.session_name : Option<String>
       ──▶ list-windows -F (… #{window_index})
              ──▶ WindowRow{index,…} ──▶ WindowModel{index,name,active,…}
              ──▶ reconcile ──▶ TmuxWindow{ index, name, active }  (ChildOf session)

status-bar rebuild (on projection change), mounted under UiRoot (Column)
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
- Session name: `ProjectionModel` gains `session_name: Option<String>`,
  populated by capturing the `name` field in the **existing**
  `ControlEvent::SessionChanged { session, name }` arm of
  `ProjectionModel::apply_event` (model.rs ~97-100), which currently sets only
  `session` and discards `name`. No new command, `EnumerationState` slot,
  reducer, or retry loop — and it tracks live `%session-changed` renames for
  free. (Verify `tmux_control_parser`'s `SessionChanged` exposes `name`; it does
  per the existing match arm.)
- Builder `select_window_command(id: WindowId) -> String` →
  `select-window -t @<id>` (`pub`, exported for the binary's click system;
  reuse the existing arg-quoting helper). The `@<id>` target is structurally
  safe (numeric) like the `%<pane>` form in reply routing.

### Status bar UI

The new window bar **reuses/replaces** the existing `StatusBarRoot` bar
(`src/ui/status_bar.rs` / `status_bar_sync.rs`) rather than adding a parallel
subsystem — there must be exactly **one** bar row. Concretely: the bar node
mounts under **`UiRoot`** (the `FlexDirection::Column` node in
`src/ui/root.rs`, where the existing bar already lives — **not**
`WorkspaceUiRoot`, whose `Absolute` 100%-height pane children would occlude it),
as a fixed-height (one cell row, `TerminalCellMetricsResource` line height),
full-width Column child after `WorkspaceUiRoot`. The old-mux rebuild
(`status_bar_sync`) is either retargeted to the tmux projection or gated off in
tmux mode so it does not also render.

- A rebuild system, gated `run_if(resource_exists_and_changed::<ProjectionModel>)`
  (or change-detected on `TmuxWindow`/`session_name`), despawns and rebuilds the
  bar's children: a left `[<session_name>]` text node (empty/elided until
  `session_name` resolves), then one `WindowEntry` button per window in `index`
  order, labelled by a pure `window_label(index, name) -> String`
  (`"<index>:<name>"`). The active window's entry uses a highlight (font weight +
  background) consistent with the active-pane treatment.
- A click system (in `InputPhase::Dispatch`, like `drive_tab_clicks`, so the
  switch is visible the same frame): on a `WindowEntry` `Interaction::Pressed`,
  send `select_window_command(entry.window_id)` via
  `connection.client().handle()`, warn on send error. No direct projection
  mutation (command-echo). A hover-cursor system (pointer over entries) mirrors
  `tab_hover_cursor`.
- The plugin registering the spawn/rebuild/click/hover systems is added in
  `src/main.rs` near the other tmux plugins. (Whether this is a new
  `OzmuxTmuxStatusBarPlugin` or an extension of the existing status-bar plugin
  is a plan-level decision; the constraint is one bar under `UiRoot`.)

### Layout reservation (method A)

`tmux -CC` does not reserve a status row (verified — §Decisions 4), so ozmux
owns the reservation. The bar node occupies the bottom cell row of `UiRoot`;
`WorkspaceUiRoot` (which holds the tmux panes) gets the remaining area.
`sync_client_size` (`src/tmux_render.rs`) changes to compute `rows` from
`physical_height - line_height` (one row reserved for the bar) so tmux lays out
panes into the area above the bar, matching the pixels `WorkspaceUiRoot`
actually occupies. ozmux does **not** mutate the tmux `status` option (it is
cosmetically irrelevant in `-CC`). Keep the existing `cols`/`rows` clamping and
the send-dedupe in `sync_client_size`; only the row count changes.

### Cleanup (deletions)

- Delete `src/action/{split_pane,focus_pane,swap_pane,close_pane}.rs` and the
  workspace New/Focus action handlers in `src/action/workspace.rs`; remove their
  sub-plugin registrations from `src/action.rs` (and drop now-empty plugins).
  Also remove the **test-only** triggers that reference these events
  (`NewWorkspaceActionEvent`/`FocusWorkspaceActionEvent` in
  `src/action/workspace.rs` tests and `src/ui.rs:815`, `SplitPaneActionEvent` in
  the `mouse_buttons.rs` test) so nothing dangles. Delete the `ShortcutAction`
  variants `SplitPane`/`FocusPane`/`SwapPane`/`ClosePane`/`NewWorkspace`/
  `FocusWorkspace` and the **`shortcuts.rs` copies** of
  `Direction`/`SplitDirection`/`SwapOffset`/`WorkspaceOffset` **iff** no referent
  remains. Do **not** touch the identically-named `ozmux_multiplexer` enums
  (still used internally; removed in Phase 5).
- In `crates/configs/src/shortcuts.rs`: move `close_pane`, `focus_pane_*`,
  `split_pane_*`, `swap_pane_*`, `new_workspace`, `focus_workspace_*` into the
  deprecated **accept-and-ignore** set (`#[serde(default, skip_serializing,
  deserialize_with = "deser_chord_or_unbind")]`, set to `None` in `Default`,
  excluded from `iter()`), exactly like the 3a surface keys; add a back-compat
  test. After this, `Bindings::iter()`/`lookup()` empties out — handle the live
  consumers, do not orphan them: `validate_no_conflicts` is called at
  `crates/configs/src/raw.rs:63` (keep it callable — a no-op over an empty set is
  fine — or remove the call), and `iter().count()` is asserted in
  `crates/configs/tests/load.rs` (3 sites) which must be updated to the new
  count. If `lookup()` then has no caller (3a moved keyboard dispatch to
  `tmux_input.rs`'s own `GuiChord`), delete it rather than leave dead code.
- The old-mux top-tab chrome (`src/ui/workspace.rs`, `src/ui/tab_input.rs`) is
  **not** deleted in 3b (Phase 5 removes the old multiplexer). If it renders a
  visible empty strip in tmux mode, hide its root node; otherwise leave it.

### Naming

New status-bar code and the new builders use "window". The `ozmux_multiplexer`
crate keeps "workspace"/"surface" until Phase 5.

## Testing

- **Pure units (`ozmux_tmux`):** `parse_window_rows` with the new
  `#{window_index}` field (and a malformed/short row); the
  `select_window_command` builder; `apply_event` on a `%session-changed` event
  sets `ProjectionModel.session_name` (and a later rename updates it).
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
  the bar is fully visible (method A: `sync_client_size` sends `rows-1`). This
  is the verify-live gate.

## Risks / unknowns (verify-live)

- **Layout (method A):** confirm that sending `rows-1` makes panes occupy the
  area above the bar with no overlap and no off-by-one (the bar's pixel height
  must equal exactly the reserved cell row). Manual GUI check + the gated layout
  test. (The underlying tmux behavior — no status-row reservation in `-CC` — is
  already verified, so this is just ozmux's own row math.)
- **session_name from `%session-changed`:** confirmed tmux emits it on attach;
  low risk. If a future tmux omits it on some attach path, the bar shows an
  empty `[]` until the next notification — acceptable.
- **Empty `Bindings`:** confirm removing the dead binding machinery doesn't
  break the `/configs/shortcuts` HTTP wire shape; keep `validate_no_conflicts`
  callable for `raw.rs:63` and update the `load.rs` count assertions (§Cleanup).

## Deferred scope (later phases)

- Per-window flags (`-` last, `Z` zoomed, `#` activity), status-left/right
  customization, clock/host — cosmetic, off the critical path.
- Live session-rename / window-rename tracking beyond attach-time + retry.
- Click-to-focus on **panes** + focus/dim (Phase 3c).
- Removal of the old `ozmux_multiplexer` crate and its dormant chrome (Phase 5).
- `list-keys` keybind mirror (display/awareness only).
