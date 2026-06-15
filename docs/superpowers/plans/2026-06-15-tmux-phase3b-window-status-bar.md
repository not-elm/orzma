# tmux Phase 3b — Window Status Bar + Action Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a tmux-style bottom status bar (session name + window list, click → `select-window`) to the ozmux GUI and delete the pane/window keyboard actions that became dead under forward-only key routing.

**Architecture:** The projection gains a window `index` and a `session_name` (captured from the existing `%session-changed` notification). A status bar mounts under `UiRoot` (replacing the dormant old-mux bar), rebuilds from the tmux projection, and routes window-entry clicks to `select-window` (command-echo). `sync_client_size` reserves one row for the bar (method A — `tmux -CC` does not reserve a status row, verified). The obsolete pane/window action modules, `ShortcutAction` variants, and `[shortcuts]` binding fields are deleted (bindings kept accept-and-ignore for config back-compat).

**Tech Stack:** Rust (edition 2024), Bevy 0.18 ECS/UI, tmux `-CC` control mode. Crates: `ozmux_tmux` (`crates/tmux_session`), the binary (`src/`), `ozmux_configs` (`crates/configs`).

**Spec:** `docs/superpowers/specs/2026-06-15-tmux-phase3b-window-status-bar-design.md`

**Conventions (enforced):** no `mod.rs`; comments only `// TODO:`/`// NOTE:`/`// SAFETY:`; `//!` on new module files, `///` on `pub` items; contiguous imports; mutable params first; private items last; whole-system change guards via `run_if`. Comments in English. `docs/` is gitignored — commit plan/spec with `git add -f`. Commit messages end with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. The `ozmux-gui` binary test suite segfaults under parallel threads (CEF) — run gui tests with `-- --test-threads=1` or filtered.

---

## File Structure

| File | Responsibility | Task |
| --- | --- | --- |
| `crates/tmux_session/src/enumerate.rs` | add `#{window_index}` to the list-windows format + `WindowRow.index` + `select_window_command` | T1, T3 |
| `crates/tmux_session/src/model.rs` | `WindowModel.index`; `ProjectionModel.session_name`; capture index + session name | T1, T2 |
| `crates/tmux_session/src/components.rs` | `TmuxWindow.index` | T1 |
| `crates/tmux_session/src/reconcile.rs` | carry `index` onto `TmuxWindow` | T1 |
| `crates/tmux_session/src/lib.rs` | export `select_window_command` | T3 |
| `src/ui/tmux_window_bar.rs` (new) | the tmux window status bar: marker, rebuild, label fn | T4, T5 |
| `src/ui/tmux_window_bar_input.rs` (new) | window-entry click → `select-window`, hover cursor | T6 |
| `src/ui/status_bar_sync.rs` | gate the old-mux bar off in tmux mode | T5 |
| `src/tmux_render.rs` | `sync_client_size` reserves one row (method A) | T7 |
| `src/main.rs` / `src/ui.rs` | register the window-bar plugin | T5, T6 |
| `src/action/*`, `src/action.rs` | delete dead pane/window action handlers + test triggers | T8 |
| `crates/configs/src/shortcuts.rs` | move pane/window keys to accept-and-ignore; drop dead enums/lookup | T8 |
| `crates/configs/src/raw.rs`, `crates/configs/tests/load.rs` | fix `validate_no_conflicts` caller + count asserts | T8 |
| `crates/tmux_session/tests/real_tmux_window.rs` (new) | gated integration | T9 |

---

## Task 1: Window index in the projection

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs` (`LIST_WINDOWS_FORMAT`, `WindowRow`, `parse_row`)
- Modify: `crates/tmux_session/src/model.rs` (`WindowModel`, `seed_from_rows`)
- Modify: `crates/tmux_session/src/components.rs` (`TmuxWindow`)
- Modify: `crates/tmux_session/src/reconcile.rs` (set `index` on `TmuxWindow`)

- [ ] **Step 1: Write the failing test** — in `crates/tmux_session/src/enumerate.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn parse_row_captures_window_index() {
        // Format order: active \t id \t index \t layout \t visible \t name
        let line = "1\t@2\t3\tb25d,80x24,0,0,0\tb25d,80x24,0,0,0\tmy-win";
        let rows = parse_window_rows(&[line.to_string()]).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 3);
        assert_eq!(rows[0].name, "my-win");
        assert!(rows[0].active);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_tmux parse_row_captures_window_index`
Expected: FAIL — `WindowRow` has no field `index` (compile error).

- [ ] **Step 3: Add the format field, the struct field, and the parse**

In `crates/tmux_session/src/enumerate.rs`, put `#{window_index}` as the **third** field (keep `#{window_name}` last — names can contain spaces; index is numeric and safe mid-row):

```rust
pub const LIST_WINDOWS_FORMAT: &str =
    "#{window_active}\t#{window_id}\t#{window_index}\t#{window_layout}\t#{window_visible_layout}\t#{window_name}";
```

Add to `WindowRow` (after `active`):

```rust
    /// tmux display index (`#{window_index}`), e.g. 0, 1, 2.
    pub index: u32,
```

In `parse_row`, change `splitn(5, '\t')` to `splitn(6, '\t')` and parse the new field between `id` and `layout_field`:

```rust
    let mut fields = line.splitn(6, '\t');
    let active = fields.next().is_some_and(|f| f == "1");
    let id = fields
        .next()
        .and_then(parse_window_id)
        .ok_or_else(|| format!("bad window id in row: {line}"))?;
    let index = fields
        .next()
        .and_then(|f| f.parse::<u32>().ok())
        .ok_or_else(|| format!("bad window index in row: {line}"))?;
    let layout_field = fields
        .next()
        .ok_or_else(|| format!("missing layout in row: {line}"))?;
```

and add `index` to the returned `WindowRow { .. }`. Update the doc comment on `parse_window_rows`/`parse_row` that describes the row shape to the 6-field order.

- [ ] **Step 4: Run the parser test to verify it passes**

Run: `cargo test -p ozmux_tmux parse_row_captures_window_index`
Expected: PASS. (Existing `parse_window_rows` tests that build 5-field lines will now FAIL — fix them in Step 5.)

- [ ] **Step 5: Thread `index` through the model, component, and reconcile**

`crates/tmux_session/src/model.rs` — add to `WindowModel` (after `active`):

```rust
    /// tmux display index (`#{window_index}`).
    pub index: u32,
```

and set it in `seed_from_rows`:

```rust
            .map(|row| WindowModel {
                id: row.id,
                active: row.active,
                index: row.index,
                name: row.name.clone(),
                panes: pane_leaves(&row.layout),
            })
```

`crates/tmux_session/src/components.rs` — add to `TmuxWindow`:

```rust
    /// tmux display index (`#{window_index}`).
    pub index: u32,
```

`crates/tmux_session/src/reconcile.rs` — wherever `TmuxWindow { .. }` is constructed/updated from a `WindowModel`, set `index: model.index` (and update the field if it changed, mirroring how `name`/`active` are synced — guard the `Mut` write so an unchanged value doesn't trigger change detection).

Fix every existing test in `model.rs`, `enumerate.rs`, `reconcile.rs`, and `crates/tmux_session/tests/*` that builds a `WindowRow`/`WindowModel`/`TmuxWindow` literal or a 5-field list-windows line: add the `index` field / the 6th `\t<index>` column. Grep: `cargo build -p ozmux_tmux --tests 2>&1 | grep -n "missing field\|window_index"` until clean.

- [ ] **Step 6: Run the crate tests**

Run: `cargo test -p ozmux_tmux 2>&1 | tail -5`
Expected: all pass; the 5 `real_tmux_*` gated tests stay ignored.

- [ ] **Step 7: Commit**

```bash
git add crates/tmux_session/src/{enumerate.rs,model.rs,components.rs,reconcile.rs} crates/tmux_session/tests
git commit -m "$(printf 'feat(tmux): carry window index through the projection\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 2: Session name from %session-changed

**Files:**
- Modify: `crates/tmux_session/src/model.rs` (`ProjectionModel.session_name`, the `SessionChanged` arm)

- [ ] **Step 1: Write the failing test** — in `crates/tmux_session/src/model.rs` tests (next to `session_changed_sets_session`):

```rust
    #[test]
    fn session_changed_sets_session_name() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::SessionChanged {
            session: SessionId(3),
            name: "main".to_string(),
        });
        assert_eq!(m.session_name.as_deref(), Some("main"));
        // A later rename updates it.
        m.apply_event(&ControlEvent::SessionChanged {
            session: SessionId(3),
            name: "renamed".to_string(),
        });
        assert_eq!(m.session_name.as_deref(), Some("renamed"));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_tmux session_changed_sets_session_name`
Expected: FAIL — `ProjectionModel` has no field `session_name`.

- [ ] **Step 3: Add the field and capture the name**

`ProjectionModel` — add (after `session`):

```rust
    /// The attached session's name, from `%session-changed`. `None` until the
    /// first such notification.
    pub session_name: Option<String>,
```

Change the `SessionChanged` arm from discarding the name to capturing it:

```rust
            ControlEvent::SessionChanged { session, name } => {
                self.session = Some(*session);
                self.session_name = Some(name.clone());
                true
            }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux_tmux session_changed_sets_session_name`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/model.rs
git commit -m "$(printf 'feat(tmux): capture session name from %%session-changed\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 3: `select_window_command` builder

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs` (new builder)
- Modify: `crates/tmux_session/src/lib.rs` (export)

- [ ] **Step 1: Write the failing test** — in `enumerate.rs` tests:

```rust
    #[test]
    fn select_window_command_targets_at_id() {
        assert_eq!(select_window_command(WindowId(4)), "select-window -t @4");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux_tmux select_window_command_targets_at_id`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement the builder**

In `enumerate.rs` (next to `client_name_command`), with the other `pub fn` builders (above any private helpers):

```rust
/// Builds `select-window -t @<id>` to switch the client's active window.
pub fn select_window_command(id: WindowId) -> String {
    format!("select-window -t @{}", id.0)
}
```

(`WindowId(pub u32)` — the `@<id>` target is structurally safe like the `%<pane>` form in reply routing, so no quoting is needed. Confirm `WindowId` is already imported in `enumerate.rs`; if not, add it to the existing `use` block.)

In `crates/tmux_session/src/lib.rs`, add `select_window_command` to the `pub use enumerate::{ ... }` list (keep it sorted/contiguous).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux_tmux select_window_command_targets_at_id`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/{enumerate.rs,lib.rs}
git commit -m "$(printf 'feat(tmux): add select_window_command builder\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 4: `window_label` pure function

**Files:**
- Create: `src/ui/tmux_window_bar.rs` (start the module with the pure label fn + its test)
- Modify: `src/ui.rs` (declare `mod tmux_window_bar;`)

- [ ] **Step 1: Create the module with the label fn and a failing test**

`src/ui/tmux_window_bar.rs`:

```rust
//! The tmux window status bar: a bottom row showing the session name and the
//! window list (`<index>:<name>`), with the active window highlighted and each
//! entry clickable to `select-window`.

use bevy::prelude::*;

/// Formats one window list entry, e.g. `0:zsh`.
fn window_label(index: u32, name: &str) -> String {
    format!("{index}:{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_label_formats_index_and_name() {
        assert_eq!(window_label(0, "zsh"), "0:zsh");
        assert_eq!(window_label(12, ""), "12:");
    }
}
```

In `src/ui.rs`, add `mod tmux_window_bar;` with the other `mod` declarations (do not `pub` it unless a sibling needs it).

- [ ] **Step 2: Run it to verify it passes (label fn is trivial)**

Run: `cargo test -p ozmux-gui window_label_formats_index_and_name -- --test-threads=1`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/ui/tmux_window_bar.rs src/ui.rs
git commit -m "$(printf 'feat(ui): tmux window-bar module + window_label\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 5: Status bar UI — render under `UiRoot`, replace the old-mux bar

**Files:**
- Modify: `src/ui/tmux_window_bar.rs` (marker components, spawn, rebuild system, plugin)
- Modify: `src/ui/status_bar_sync.rs` (gate the old-mux bar off in tmux mode)
- Modify: `src/main.rs` (register the plugin)

**Read first (patterns to mirror, do NOT reinvent):**
- `src/ui/status_bar.rs` + `src/ui/status_bar_sync.rs` — the existing `StatusBarRoot` bar: how it spawns a bar node under `UiRoot` and rebuilds its children on a change. Mirror its node/anchor style.
- `src/ui/root.rs` — `UiRoot` (the `FlexDirection::Column` parent) and `WorkspaceUiRoot`. The bar is a fixed-height Column child of `UiRoot`, after `WorkspaceUiRoot`.
- `crates/tmux_session` exports: `ProjectionModel`, `TmuxWindow`, `TmuxSession`.
- `src/theme` — colors for the bar background, normal vs active entry.

- [ ] **Step 1: Write the failing headless test** — in `tmux_window_bar.rs` tests:

```rust
    #[test]
    fn rebuild_renders_session_and_window_entries_with_active_highlight() {
        use ozmux_tmux::{ProjectionModel, WindowModel};
        use tmux_control_parser::{PaneId, WindowId, CellDims};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // The bar plugin's rebuild system + the StatusBar root, but no CEF/render.
        app.add_plugins(OzmuxTmuxWindowBarPlugin);
        let mut model = ProjectionModel::default();
        model.session_name = Some("main".into());
        model.windows = vec![
            WindowModel { id: WindowId(1), active: false, index: 0, name: "zsh".into(), panes: vec![] },
            WindowModel { id: WindowId(2), active: true,  index: 1, name: "vim".into(), panes: vec![] },
        ];
        app.insert_resource(model);
        app.update(); // spawn + first rebuild
        app.update(); // settle

        // Collect WindowEntry labels + which is active.
        let world = app.world_mut();
        let mut q = world.query::<(&WindowEntry, &WindowEntryActive)>();
        let entries: Vec<_> = q.iter(world).map(|(e, a)| (e.index, a.0)).collect();
        assert!(entries.contains(&(0, false)), "win 0 inactive: {entries:?}");
        assert!(entries.contains(&(1, true)), "win 1 active: {entries:?}");
    }
```

(Adapt the exact assertion shape to your component design — the contract is: after a rebuild over a seeded `ProjectionModel`, there is one entry per window carrying its `index` and whether it is the active window, plus a session-name node. If the existing `status_bar.rs` test harness is reusable, follow it instead.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux-gui rebuild_renders_session_and_window_entries -- --test-threads=1`
Expected: FAIL — `OzmuxTmuxWindowBarPlugin`/`WindowEntry`/`WindowEntryActive` not defined.

- [ ] **Step 3: Implement the bar marker, spawn, rebuild, and plugin**

In `tmux_window_bar.rs` add:
- Markers: `#[derive(Component)] struct WindowBarRoot;`, `#[derive(Component)] struct WindowEntry { index: u32, window: WindowId }`, `#[derive(Component)] struct WindowEntryActive(bool)`, and a session-name text marker.
- A `spawn_window_bar` startup system that spawns the `WindowBarRoot` node as a fixed-height (`TerminalCellMetricsResource` line height) full-width `FlexDirection::Row` child of `UiRoot` (query `UiRoot`; `ChildOf` it), positioned after `WorkspaceUiRoot`. Background from `src/theme`. Mirror `status_bar.rs`'s spawn.
- A `rebuild_window_bar` system gated `.run_if(resource_exists_and_changed::<ProjectionModel>)`: despawn the bar's children (descendant-aware), then spawn a `[<session_name>]` text node (empty string until `session_name` is `Some`), then one `WindowEntry` button per window in `index` order (read from `ProjectionModel.windows`, or from `TmuxWindow` entities — pick `ProjectionModel` for a single source), labelled `window_label(index, name)`, with `WindowEntryActive(model.active)` and an active-vs-normal color from `src/theme`. Use `Button`/`Interaction` on each entry (needed by T6).
- `OzmuxTmuxWindowBarPlugin` registering `spawn_window_bar` (Startup) and `rebuild_window_bar` (Update, with the `run_if`).

Make `WindowEntry`/`WindowEntryActive`/`OzmuxTmuxWindowBarPlugin` reachable by the test (same module → fine) and by T6 (`pub(crate)` if `tmux_window_bar_input.rs` needs `WindowEntry`).

- [ ] **Step 4: Gate the old-mux bar off in tmux mode**

In `src/ui/status_bar_sync.rs`: the old-mux `StatusBarRoot` rebuild keys on `WorkspaceMarker`/`AttachedWorkspace`, which are never populated in tmux mode, so it renders empty — but its root node still occupies a `UiRoot` row. Make the old bar not occupy space: either (a) add a `run_if` so its spawn is skipped when no old-mux workspace exists, or (b) set its root node `Display::None` when `ProjectionModel` is present/attached. Choose the smallest change that leaves exactly **one** visible bar (the new tmux one). Keep the old code otherwise intact (Phase 5 deletes it). Document the choice in the system's doc comment.

- [ ] **Step 5: Register the plugin**

In `src/main.rs`, add `OzmuxTmuxWindowBarPlugin` to the plugin list near the other tmux plugins (`OzmuxTmuxRenderPlugin`, `OzmuxTmuxInputPlugin`).

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p ozmux-gui tmux_window_bar -- --test-threads=1`
Expected: PASS.

- [ ] **Step 7: Build + clippy + commit**

```bash
cargo build 2>&1 | tail -3
cargo clippy -p ozmux-gui --all-targets 2>&1 | tail -3
cargo fmt
git add src/ui/tmux_window_bar.rs src/ui/status_bar_sync.rs src/main.rs
git commit -m "$(printf 'feat(ui): tmux window status bar under UiRoot\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 6: Window-entry click → select-window, hover cursor

**Files:**
- Create: `src/ui/tmux_window_bar_input.rs`
- Modify: `src/ui.rs` (`mod tmux_window_bar_input;`)
- Modify: `src/main.rs` (register, or fold into `OzmuxTmuxWindowBarPlugin`)

**Read first:** `src/ui/tab_input.rs` — `drive_tab_clicks` (`.in_set(InputPhase::Dispatch)`) and `tab_hover_cursor` (`.after(InputPhase::Hover)`). Mirror exactly. The connection send pattern: `connection.client().handle().send(&cmd)` with `tracing::warn!` on error (see `sync_client_size` in `src/tmux_render.rs`).

- [ ] **Step 1: Write the failing headless test** — in `tmux_window_bar_input.rs` tests, drive a press on a `WindowEntry` and assert a `select-window -t @<id>` command was sent. Use the same recording/fake `TmuxConnection` seam the existing `tmux_render.rs` tests use (read them); if the live `TmuxConnection` cannot be faked headlessly, assert instead the pure mapping `WindowEntry { window: WindowId(2), .. } → select_window_command(WindowId(2)) == "select-window -t @2"` and cover the click wiring in the gated test (T9). Pick the approach that the existing test infra supports; do not fabricate a brittle fake.

```rust
    #[test]
    fn entry_press_maps_to_select_window() {
        use ozmux_tmux::select_window_command;
        use tmux_control_parser::WindowId;
        // The click handler sends exactly this for a pressed entry.
        assert_eq!(select_window_command(WindowId(2)), "select-window -t @2");
    }
```

- [ ] **Step 2: Run it to verify it fails / compiles**

Run: `cargo test -p ozmux-gui tmux_window_bar_input -- --test-threads=1`
Expected: FAIL (module/file not present) until Step 3.

- [ ] **Step 3: Implement the click + hover systems**

`src/ui/tmux_window_bar_input.rs`:

```rust
//! tmux window-bar interaction: click a window entry to `select-window`, and a
//! pointer cursor while hovering an entry.

use crate::input::InputPhase;
use crate::ui::tmux_window_bar::WindowEntry;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozmux_tmux::{TmuxConnection, select_window_command};
```

- A `switch_window_on_click` system `.in_set(InputPhase::Dispatch)`: for each `(&Interaction, &WindowEntry)` with `Changed<Interaction>` that is `Interaction::Pressed`, if `connection.client()` is `Some`, send `select_window_command(entry.window)` via `client.handle().send(&cmd)`, `tracing::warn!` on error. No projection mutation (command-echo). Params: `connection: NonSend<TmuxConnection>` (immutable) plus the query.
- A `window_entry_hover_cursor` system `.after(InputPhase::Hover)` mirroring `tab_hover_cursor` (pointer when any entry is `Hovered`/`Pressed`).
- Register both (either a new plugin added in `main.rs`, or add them to `OzmuxTmuxWindowBarPlugin` from T5 — prefer folding into the existing plugin so the bar is one unit). If folded, make `WindowEntry` `pub(crate)` and import it.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux-gui tmux_window_bar_input -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Build + clippy + commit**

```bash
cargo build 2>&1 | tail -3
cargo clippy -p ozmux-gui --all-targets 2>&1 | tail -3
cargo fmt
git add src/ui/tmux_window_bar_input.rs src/ui.rs src/main.rs
git commit -m "$(printf 'feat(ui): click a window entry to select-window\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 7: Layout reservation (method A)

**Files:**
- Modify: `src/tmux_render.rs` (`sync_client_size`, and the `cells_for`/row computation)

**Context:** `tmux -CC` does not reserve a status row (verified). ozmux reserves it: the bar occupies one cell row of `UiRoot`, so the pane area (`WorkspaceUiRoot`) is one row shorter, and `sync_client_size` must tell tmux `rows-1` so tmux lays panes into that area. Read `sync_client_size` + `cells_for` (`src/tmux_render.rs` ~200-238).

- [ ] **Step 1: Write the failing test** — add a unit test for the reserved-row math. If row computation lives in the pure `cells_for(w_px, h_px, cell_w, cell_h)`, add a tiny pure helper `rows_for_panes(total_rows: u16) -> u16 { total_rows.saturating_sub(1).max(1) }` and test it:

```rust
    #[test]
    fn rows_for_panes_reserves_one_row_for_the_bar() {
        assert_eq!(rows_for_panes(24), 23);
        assert_eq!(rows_for_panes(1), 1); // never zero
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux-gui rows_for_panes_reserves_one_row -- --test-threads=1`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement and apply**

Add `fn rows_for_panes(total_rows: u16) -> u16 { total_rows.saturating_sub(1).max(1) }` (private, below the `pub`/system items). In `sync_client_size`, after computing `(cols, rows)` from the window's physical height via `cells_for`, reserve the bar row before the send/dedupe:

```rust
    let rows = rows_for_panes(rows);
```

so `refresh_client_command(cols, rows)` (and the `LastClientSize` dedupe) use the reserved value. Do not change `cols`. Update `sync_client_size`'s doc comment to note the reserved bar row.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux-gui rows_for_panes_reserves_one_row -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/tmux_render.rs
git commit -m "$(printf 'feat(tmux): reserve one row for the window status bar\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 8: Delete the dead pane/window actions + bindings

**Files:**
- Delete: `src/action/{split_pane,focus_pane,swap_pane,close_pane}.rs`
- Modify: `src/action.rs`, `src/action/workspace.rs`
- Modify: `crates/configs/src/shortcuts.rs`
- Modify: `crates/configs/src/raw.rs`, `crates/configs/tests/load.rs`
- Modify: `src/ui.rs` (the `ui.rs:815` test trigger), `src/input/mouse_buttons.rs` (the test trigger)

- [ ] **Step 1: Delete the action modules + registrations**

Delete `src/action/split_pane.rs`, `focus_pane.rs`, `swap_pane.rs`, `close_pane.rs`. In `src/action/workspace.rs` delete the `NewWorkspace`/`FocusWorkspace` action handlers + their `*ActionEvent`s + plugin(s). In `src/action.rs` remove the `mod` declarations and the sub-plugin registrations from `OzmuxActionPlugin` (if it becomes empty, delete `OzmuxActionPlugin` and its registration in `src/main.rs`). Remove the **test-only** triggers that reference these events: in `src/action/workspace.rs` tests, `src/ui.rs:815`, and the `SplitPaneActionEvent` trigger in the `src/input/mouse_buttons.rs` test (grep `git grep -n 'WorkspaceActionEvent\|SplitPaneActionEvent\|FocusPaneActionEvent\|SwapPaneActionEvent\|ClosePaneActionEvent'` and remove each non-deleted referent).

- [ ] **Step 2: Build to find dangling references**

Run: `cargo build 2>&1 | grep -nE "cannot find|unresolved|unused import" | head -30`
Fix each: remove now-unused imports, delete `ShortcutAction` match arms that referenced the deleted variants.

- [ ] **Step 3: Prune `ShortcutAction` + the `shortcuts.rs` enums**

In `crates/configs/src/shortcuts.rs` delete the `ShortcutAction` variants `SplitPane`/`FocusPane`/`SwapPane`/`ClosePane`/`NewWorkspace`/`FocusWorkspace` and the **`shortcuts.rs` copies** of `Direction`/`SplitDirection`/`SwapOffset`/`WorkspaceOffset` (only if no referent remains). Do NOT touch the identically-named `ozmux_multiplexer` enums.

- [ ] **Step 4: Move the pane/window binding keys to accept-and-ignore**

In `crates/configs/src/shortcuts.rs`, move `close_pane`, `focus_pane_left/down/up/right`, `split_pane_vertical/horizontal`, `swap_pane_prev/next`, `new_workspace`, `focus_workspace_prev/next` into the deprecated set exactly like the 3a surface keys: each `#[serde(default, skip_serializing, deserialize_with = "deser_chord_or_unbind")]`, set to `None` in `Default`, and removed from `iter()`. After this `iter()` yields the remaining active bindings only (`paste`, `release_inline_focus` if still active — keep whatever 3a left active). Add a back-compat test:

```rust
    #[test]
    fn deprecated_pane_window_bindings_are_accepted_and_ignored() {
        let toml = "\
[bindings]
split-pane-vertical = \"Cmd+I\"
focus-pane-left = \"Cmd+H\"
new-workspace = \"Cmd+R\"
";
        let parsed: Shortcuts = toml::from_str(toml).expect("deprecated keys must still parse");
        assert!(parsed.bindings.iter().all(|(label, _, _)| {
            !matches!(label, "split-pane-vertical" | "focus-pane-left" | "new-workspace")
        }), "ignored keys must not enter the active set");
    }
```

- [ ] **Step 5: Fix the live consumers of `iter()`/`validate_no_conflicts`**

`crates/configs/src/raw.rs:63` calls `validate_no_conflicts` — keep it callable (a no-op over the smaller/empty set is fine); only delete the call if `validate_no_conflicts` itself is removed. If `lookup()` now has **no** caller (3a moved keyboard dispatch to `tmux_input.rs`'s `GuiChord`; grep `git grep -n '\.lookup('`), delete `lookup()` rather than leave dead code. Update the `iter().count()` assertions in `crates/configs/tests/load.rs` (3 sites — grep `count()`) to the new active-binding count.

- [ ] **Step 6: Run the config + workspace + build checks**

```bash
cargo test -p ozmux_configs 2>&1 | tail -5
cargo build 2>&1 | tail -3
cargo clippy --workspace --all-targets 2>&1 | tail -4
```
Expected: green, no warnings. Fix any remaining dangling refs.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add -A
git commit -m "$(printf 'refactor(tmux): drop dead pane/window actions + bindings\n\nForward-only key routing means tmux owns these bindings; the ozmux\nhandlers had no trigger. Bindings kept accept-and-ignore for config\nback-compat.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 9: Integration test + final verification

**Files:**
- Create: `crates/tmux_session/tests/real_tmux_window.rs`

- [ ] **Step 1: Gated real-tmux integration test**

Mirror `crates/tmux_session/tests/real_tmux_input.rs` (READ it for the exact harness, `#[ignore = "requires a real tmux binary and a controlling PTY"]`, socket naming, kill-server teardown). The test: spawn `tmux -CC`, add `TmuxSessionPlugin`, drive `app.update()` until `ConnectionState::Attached` and `ProjectionModel.session_name.is_some()` and `windows` is non-empty; assert the windows carry indices; send `new-window` via the client handle, pump until a second window appears; send `select_window_command(<first window id>)`, pump, and assert `ProjectionModel.windows`'s active flag moved to the targeted window. Module `//!` doc with the run command `cargo test -p ozmux_tmux --test real_tmux_window -- --ignored`.

- [ ] **Step 2: Full check**

```bash
cargo build 2>&1 | tail -3
cargo clippy --workspace --all-targets 2>&1 | tail -4
cargo fmt --check 2>&1 | tail -3
cargo test -p ozma_tty_engine -p ozmux_tmux -p ozmux_configs 2>&1 | tail -8
cargo test -p ozmux-gui -- --test-threads=1 2>&1 | grep "test result" | tail -3
cargo test -p ozmux_tmux --test real_tmux_window 2>&1 | tail -4   # collected as 1 ignored
```
Expected: all green; the new gated test shows `1 ignored`.

- [ ] **Step 3: Manual GUI verification** (desktop; run from OUTSIDE the attached tmux session)

`cargo run`, pick a session. Confirm: the bottom status bar shows `[session]` + the window list; the panes occupy the area **above** the bar with no overlap (method A row reservation); creating a window via tmux's prefix (`C-b c`) adds an entry; clicking a window entry switches to it (and the highlight follows). Note any layout off-by-one.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A && git commit -m "$(printf 'test(tmux): gated real-tmux window-switch integration test\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')" || echo "nothing to commit"
```

---

## Out of scope (later phases)

- Per-window flags (`-`/`Z`/`#`), status-left/right customization, clock/host.
- Click-to-focus on **panes** + focus/dim (Phase 3c).
- Removal of the old `ozmux_multiplexer` crate and its dormant chrome (Phase 5).
- `list-keys` keybind mirror (display/awareness only).
