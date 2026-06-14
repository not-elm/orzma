# Tmux Migration Phase 1b — Projection Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure, indexed projection that mirrors tmux session/window/pane state as Bevy entities: a plain-data model + reducer (driven by control-mode notifications and a parsed `list-windows` reply), and an ECS reconcile that spawns/updates/despawns `TmuxSession`/`TmuxWindow`/`TmuxPane` entities to match — with NO rendering. Entities are asserted in tests.

**Architecture:** All new code lives in the `ozmux_tmux` crate (`crates/tmux_session`). A plain-data `ProjectionModel` (a Bevy `Resource`) is the desired projection; pure functions mutate it: `seed_from_rows` (from a parsed `list-windows` reply) and `apply_event` (from `ControlEvent` notifications). A `reconcile_projection` system diffs the model against a `TmuxProjection` index (`HashMap<WindowId, Entity>` / `HashMap<PaneId, Entity>`) and spawns/updates/despawns entities (`TmuxSession` → `TmuxWindow` children → `TmuxPane` children). The Phase 0 event pump is refactored to a two-phase shape: drain transport events into a `Vec`, advance `ConnectionState`, then route notifications into `ProjectionModel`. Initial `list-windows` enumeration + command/reply correlation is deferred to Phase 1c; this phase populates the model from the live notification stream (`%window-add` fires per window on attach; `%layout-change` carries geometry) and is fully exercised by synthetic-event tests plus a gated real-tmux test.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18, in-repo `tmux_control` + `tmux_control_parser`.

---

## Background — verified facts the implementer must rely on

Trust these.

- **`tmux_control_parser` exports** (add it as a normal dependency of `ozmux_tmux`): `ControlEvent`, `PaneId`, `WindowId`, `SessionId`, `Cell`, `CellDims`, `SplitDir`, `WindowLayout`, `TmuxResult`, `TmuxError`.
  - `CellDims { width: u32, height: u32, xoff: i32, yoff: i32 }` derives `Debug, Clone, Copy, PartialEq, Eq`.
  - `Cell::Leaf { dims: CellDims, pane_id: Option<u32> }` | `Cell::Split { dims: CellDims, dir: SplitDir, children: Vec<Cell> }`; `Cell::dims() -> CellDims`.
  - `WindowLayout { checksum: u16, root: Cell }`; `WindowLayout::parse(input: &[u8]) -> TmuxResult<WindowLayout>`.
  - `PaneId(pub u32)`, `WindowId(pub u32)`, `SessionId(pub u32)` — all derive `Debug, Clone, Copy, PartialEq, Eq, Hash`.
  - `ControlEvent` variants used here: `SessionChanged { session: SessionId, name: String }`, `WindowAdd { window: WindowId }`, `WindowClose { window: WindowId }`, `WindowRenamed { window: WindowId, name: String }`, `LayoutChange { window: WindowId, layout: WindowLayout, visible_layout: WindowLayout, flags: String }`, `WindowPaneChanged { window: WindowId, pane: PaneId }`.
- **`tmux_control` exports** (already a dep): `ClientEvent { CommandComplete { id, number, ok, output }, Notification(ControlEvent) }`, `TransportEvent { Protocol(ClientEvent), Closed { reason } }`, `ControlEvent` (re-export). The Phase 0 `ozmux_tmux` crate has `ConnectionState`, `TmuxConnection`, `event_pump::{drain_events, ...}`, `state::next_state`, `plugin::TmuxSessionPlugin`.
- **Phase 0 `event_pump.rs`** currently: `pub(crate) fn drain_events(state: &mut ConnectionState, events: &Receiver<TransportEvent>)` (drains + logs + advances state via `next_state`) and private `log_transport_event`. The Phase 0 plugin system `drain_tmux_events(mut state: ResMut<ConnectionState>, connection: NonSend<TmuxConnection>)` calls it. Task 5 refactors this.
- **Repo Rust rules:** no `mod.rs`; only `// TODO:`/`// NOTE:`/`// SAFETY:` comments; `//!` per module file; `///` on every `pub` item; all `use` at top in one block; mutable params before immutable; private items after public; visibility minimized; no `#[allow]`/`#[expect]` without a justified `// NOTE:`. Use `std::collections::HashMap` (the crate is not on Bevy's collection aliases).

## File Structure

- Modify `crates/tmux_session/Cargo.toml` — add `tmux_control_parser` as a normal dependency.
- Create `crates/tmux_session/src/model.rs` — `PaneModel`, `WindowModel`, `ProjectionModel` (Resource), `pane_leaves`, `seed_from_rows`, `apply_event`.
- Create `crates/tmux_session/src/enumerate.rs` — `WindowRow`, `parse_window_rows`, `LIST_WINDOWS_FORMAT`.
- Create `crates/tmux_session/src/components.rs` — `TmuxSession`, `TmuxWindow`, `TmuxPane`.
- Create `crates/tmux_session/src/reconcile.rs` — `TmuxProjection` (index Resource), `reconcile_projection` system.
- Modify `crates/tmux_session/src/event_pump.rs` — two-phase drain (`drain_transport` + `advance_state` + `route_to_model`).
- Modify `crates/tmux_session/src/plugin.rs` — register the new resources + systems, rewire the drain system.
- Modify `crates/tmux_session/src/lib.rs` — declare modules + re-exports.

---

### Task 1: `tmux_control_parser` dep + layout→panes extraction

**Files:** Modify `crates/tmux_session/Cargo.toml`; create `crates/tmux_session/src/model.rs`; modify `crates/tmux_session/src/lib.rs`.

- [ ] **Step 1: Add the dependency**

In `crates/tmux_session/Cargo.toml`, under `[dependencies]`, add (it's already a `[dev-dependencies]` entry — now it's needed in non-test code too; keep the dev-dep line as well, or rely on the normal dep which also covers tests):

```toml
tmux_control_parser = { path = "../tmux_control_parser" }
```

If a `[dev-dependencies]` entry for `tmux_control_parser` already exists, you may remove it (the normal dependency makes it available to tests too).

- [ ] **Step 2: Create `model.rs` with `PaneModel` + `pane_leaves`**

Create `crates/tmux_session/src/model.rs`:

```rust
//! The plain-data projection model and the pure reducer that maintains it.

use tmux_control_parser::{Cell, CellDims, PaneId, WindowLayout};

/// A projected pane: its tmux id and cell geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneModel {
    /// tmux pane id (`%N`).
    pub id: PaneId,
    /// Cell geometry from the window layout.
    pub dims: CellDims,
}

/// Flattens a window layout tree into its panes, in layout order.
///
/// Each `Cell::Leaf` carrying a pane id becomes a [`PaneModel`]; leaves with
/// no id (a layout-grammar artifact) are skipped.
pub fn pane_leaves(layout: &WindowLayout) -> Vec<PaneModel> {
    let mut out = Vec::new();
    collect_leaves(&layout.root, &mut out);
    out
}

fn collect_leaves(cell: &Cell, out: &mut Vec<PaneModel>) {
    match cell {
        Cell::Leaf { dims, pane_id } => {
            if let Some(id) = pane_id {
                out.push(PaneModel {
                    id: PaneId(*id),
                    dims: *dims,
                });
            }
        }
        Cell::Split { children, .. } => {
            for child in children {
                collect_leaves(child, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims(width: u32, height: u32, xoff: i32, yoff: i32) -> CellDims {
        CellDims {
            width,
            height,
            xoff,
            yoff,
        }
    }

    #[test]
    fn single_pane_layout_yields_one_pane() {
        // "checksum,80x24,0,0,0" — one leaf, pane id 0.
        let layout = WindowLayout::parse(b"b25f,80x24,0,0,0").unwrap();
        assert_eq!(
            pane_leaves(&layout),
            vec![PaneModel {
                id: PaneId(0),
                dims: dims(80, 24, 0, 0),
            }]
        );
    }

    #[test]
    fn horizontal_split_yields_two_panes_in_order() {
        // Two panes side by side: left %1 (40x24@0,0), right %2 (39x24@41,0).
        let layout = WindowLayout::parse(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").unwrap();
        let panes = pane_leaves(&layout);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].id, PaneId(1));
        assert_eq!(panes[1].id, PaneId(2));
        assert_eq!(panes[0].dims, dims(40, 24, 0, 0));
        assert_eq!(panes[1].dims, dims(39, 24, 41, 0));
    }
}
```

NOTE: the exact checksum prefix (`b25f`/`abcd`) does not matter — `WindowLayout::parse` stores the checksum verbatim and never fails on a mismatch (confirmed in `crates/tmux_control_parser/src/layout.rs`). If a test's layout string fails to parse for a structural reason, fix the layout string to match tmux's grammar (`WxH,xoff,yoff[,paneid]`, `{}` = left-right, `[]` = top-bottom), not the parser.

- [ ] **Step 3: Declare the module**

In `crates/tmux_session/src/lib.rs`, add `mod model;` to the module block and `pub use model::{PaneModel, pane_leaves};` to the re-export block.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ozmux_tmux model::`
Expected: 2 tests pass. (If `WindowLayout::parse` rejects a test string, adjust the string to valid tmux layout grammar — see the NOTE.)

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/Cargo.toml crates/tmux_session/src/model.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux_session): pane_leaves layout-tree flattening + parser dep"
```

---

### Task 2: `list-windows` reply parsing

**Files:** Create `crates/tmux_session/src/enumerate.rs`; modify `crates/tmux_session/src/lib.rs`.

- [ ] **Step 1: Write the module with tests**

Create `crates/tmux_session/src/enumerate.rs`:

```rust
//! Parsing the `list-windows -F` reply used to enumerate windows on attach.

use tmux_control_parser::{WindowId, WindowLayout};

/// The `-F` format ozmux sends to enumerate windows. Tab-separated, with the
/// free-text `window_name` LAST so a `splitn(5, '\t')` keeps it intact.
pub const LIST_WINDOWS_FORMAT: &str =
    "#{window_active}\t#{window_id}\t#{window_layout}\t#{window_visible_layout}\t#{window_name}";

/// One parsed row of the `list-windows` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRow {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// Whether this is the session's active window.
    pub active: bool,
    /// Window name.
    pub name: String,
    /// Parsed structural layout (panes + geometry).
    pub layout: WindowLayout,
}

/// Parses the lines of a `list-windows -F LIST_WINDOWS_FORMAT` reply.
///
/// Each line is `active \t window_id \t layout \t visible_layout \t name`.
/// The `visible_layout` field is currently ignored. Blank lines are skipped.
/// Returns a descriptive `Err(String)` on a malformed row.
pub fn parse_window_rows(lines: &[String]) -> Result<Vec<WindowRow>, String> {
    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        rows.push(parse_row(line)?);
    }
    Ok(rows)
}

fn parse_row(line: &str) -> Result<WindowRow, String> {
    let mut fields = line.splitn(5, '\t');
    let active = fields.next().is_some_and(|f| f == "1");
    let id = fields
        .next()
        .and_then(parse_window_id)
        .ok_or_else(|| format!("bad window id in row: {line}"))?;
    let layout_field = fields
        .next()
        .ok_or_else(|| format!("missing layout in row: {line}"))?;
    let layout = WindowLayout::parse(layout_field.as_bytes())
        .map_err(|e| format!("bad layout in row {line}: {e}"))?;
    let _visible = fields
        .next()
        .ok_or_else(|| format!("missing visible layout in row: {line}"))?;
    let name = fields
        .next()
        .ok_or_else(|| format!("missing name in row: {line}"))?
        .to_string();
    Ok(WindowRow {
        id,
        active,
        name,
        layout,
    })
}

fn parse_window_id(field: &str) -> Option<WindowId> {
    Some(WindowId(field.strip_prefix('@')?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_is_tab_separated_with_name_last() {
        assert!(LIST_WINDOWS_FORMAT.contains('\t'));
        assert!(LIST_WINDOWS_FORMAT.ends_with("#{window_name}"));
    }

    #[test]
    fn parses_one_active_window() {
        let lines = vec!["1\t@1\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tmain".to_string()];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, WindowId(1));
        assert!(rows[0].active);
        assert_eq!(rows[0].name, "main");
        assert_eq!(rows[0].layout.root.dims().width, 80);
    }

    #[test]
    fn parses_multiple_windows_active_flag() {
        let lines = vec![
            "0\t@1\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tone".to_string(),
            "1\t@2\tb25f,80x24,0,0,1\tb25f,80x24,0,0,1\ttwo".to_string(),
        ];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!((rows[0].active, rows[1].active), (false, true));
        assert_eq!((rows[0].id, rows[1].id), (WindowId(1), WindowId(2)));
    }

    #[test]
    fn name_with_tabs_is_preserved_as_last_field() {
        // splitn(5) keeps everything after the 4th tab in `name`.
        let lines = vec!["1\t@1\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tmy\tnamed\twin".to_string()];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!(rows[0].name, "my\tnamed\twin");
    }

    #[test]
    fn bad_window_id_errors() {
        let lines = vec!["1\t1\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tx".to_string()];
        assert!(parse_window_rows(&lines).is_err());
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(parse_window_rows(&[]).unwrap(), vec![]);
    }
}
```

NOTE: `parse_window_rows` returns `Result<_, String>` deliberately — the `tmux_control_parser` and `tmux_control` crates have separate `TmuxError` types and neither has a clean "malformed window row" variant, so a descriptive `String` avoids coupling to either. `WindowLayout::parse` returns the parser's `TmuxResult`; `.map_err(|e| format!(..))` bridges it. The first parse test reads `rows[0].layout.root.dims().width` via the public `Cell::dims()` accessor.

- [ ] **Step 2: Declare the module**

In `crates/tmux_session/src/lib.rs`, add `mod enumerate;` and `pub use enumerate::{LIST_WINDOWS_FORMAT, WindowRow, parse_window_rows};`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p ozmux_tmux enumerate::`
Expected: 6 tests pass. If the `TmuxError` variant name is wrong, fix `malformed` per the NOTE and re-run.

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/src/enumerate.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux_session): parse_window_rows for the list-windows reply"
```

---

### Task 3: `ProjectionModel` + pure reducer

**Files:** Modify `crates/tmux_session/src/model.rs`; modify `crates/tmux_session/src/lib.rs`.

- [ ] **Step 1: Add the model types + reducer to `model.rs`**

Append to `crates/tmux_session/src/model.rs` (after `pane_leaves`, before the `#[cfg(test)] mod tests` block) — and update the top `use` line to include the extra imports:

Change the top `use` to:

```rust
use crate::enumerate::WindowRow;
use bevy::prelude::Resource;
use tmux_control_parser::{Cell, CellDims, ControlEvent, PaneId, SessionId, WindowId, WindowLayout};
```

Add:

```rust
/// A projected window: id, active flag, name, and its panes (layout order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowModel {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// Whether this is the session's active window.
    pub active: bool,
    /// Window name.
    pub name: String,
    /// Panes in layout order.
    pub panes: Vec<PaneModel>,
}

/// The desired projection: the session and its windows, plus the active pane.
///
/// Mutated by the pure reducer ([`ProjectionModel::seed_from_rows`] /
/// [`ProjectionModel::apply_event`]); the ECS reconcile syncs entities to it.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectionModel {
    /// The attached session id, once known.
    pub session: Option<SessionId>,
    /// Windows in insertion order.
    pub windows: Vec<WindowModel>,
    /// The currently active pane, once known.
    pub active_pane: Option<PaneId>,
}

impl ProjectionModel {
    /// Replaces the window set from a parsed `list-windows` reply.
    pub fn seed_from_rows(&mut self, rows: &[WindowRow]) {
        self.windows = rows
            .iter()
            .map(|row| WindowModel {
                id: row.id,
                active: row.active,
                name: row.name.clone(),
                panes: pane_leaves(&row.layout),
            })
            .collect();
    }

    /// Applies one control-mode notification to the model.
    pub fn apply_event(&mut self, event: &ControlEvent) {
        match event {
            ControlEvent::SessionChanged { session, .. } => {
                self.session = Some(*session);
            }
            ControlEvent::WindowAdd { window } => {
                self.ensure_window(*window);
            }
            ControlEvent::WindowClose { window } => {
                self.windows.retain(|w| w.id != *window);
            }
            ControlEvent::WindowRenamed { window, name } => {
                if let Some(w) = self.window_mut(*window) {
                    w.name = name.clone();
                }
            }
            ControlEvent::LayoutChange {
                window, layout, ..
            } => {
                self.set_layout(*window, layout);
            }
            ControlEvent::WindowPaneChanged { window, pane } => {
                self.active_pane = Some(*pane);
                self.set_active_window(*window);
            }
            _ => {}
        }
    }

    fn ensure_window(&mut self, id: WindowId) -> &mut WindowModel {
        if let Some(idx) = self.windows.iter().position(|w| w.id == id) {
            return &mut self.windows[idx];
        }
        self.windows.push(WindowModel {
            id,
            active: false,
            name: String::new(),
            panes: Vec::new(),
        });
        self.windows.last_mut().expect("just pushed")
    }

    fn window_mut(&mut self, id: WindowId) -> Option<&mut WindowModel> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    fn set_layout(&mut self, id: WindowId, layout: &WindowLayout) {
        let panes = pane_leaves(layout);
        self.ensure_window(id).panes = panes;
    }

    fn set_active_window(&mut self, id: WindowId) {
        for w in &mut self.windows {
            w.active = w.id == id;
        }
    }
}
```

NOTE: `set_layout` computes `pane_leaves(layout)` BEFORE calling `ensure_window` to avoid an overlapping mutable/immutable borrow of `self`. Keep that ordering.

- [ ] **Step 2: Add reducer tests**

In `model.rs`'s `#[cfg(test)] mod tests`, add (the existing `use super::*;` covers the new items; add `use tmux_control_parser::{ControlEvent, SessionId, WindowId};` inside the test module if not already imported, plus a small helper):

```rust
    fn layout(spec: &[u8]) -> WindowLayout {
        WindowLayout::parse(spec).unwrap()
    }

    #[test]
    fn session_changed_sets_session() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::SessionChanged {
            session: SessionId(3),
            name: "main".to_string(),
        });
        assert_eq!(m.session, Some(SessionId(3)));
    }

    #[test]
    fn window_add_then_close() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::WindowAdd { window: WindowId(1) });
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].id, WindowId(1));
        m.apply_event(&ControlEvent::WindowClose { window: WindowId(1) });
        assert!(m.windows.is_empty());
    }

    #[test]
    fn layout_change_sets_panes_and_creates_window() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::LayoutChange {
            window: WindowId(7),
            layout: layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
            visible_layout: layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
            flags: String::new(),
        });
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].panes.len(), 2);
        assert_eq!(m.windows[0].panes[0].id, PaneId(1));
    }

    #[test]
    fn window_pane_changed_sets_active_pane_and_window() {
        let mut m = ProjectionModel::default();
        m.apply_event(&ControlEvent::WindowAdd { window: WindowId(1) });
        m.apply_event(&ControlEvent::WindowAdd { window: WindowId(2) });
        m.apply_event(&ControlEvent::WindowPaneChanged {
            window: WindowId(2),
            pane: PaneId(5),
        });
        assert_eq!(m.active_pane, Some(PaneId(5)));
        assert!(!m.windows[0].active);
        assert!(m.windows[1].active);
    }

    #[test]
    fn seed_from_rows_builds_windows_with_panes() {
        use crate::enumerate::parse_window_rows;
        let rows = parse_window_rows(&[
            "1\t@1\tabcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}\tx\tmain".to_string(),
        ])
        .unwrap();
        let mut m = ProjectionModel::default();
        m.seed_from_rows(&rows);
        assert_eq!(m.windows.len(), 1);
        assert_eq!(m.windows[0].panes.len(), 2);
        assert!(m.windows[0].active);
    }
```

- [ ] **Step 3: Re-export**

In `crates/tmux_session/src/lib.rs`, extend the model re-export to `pub use model::{PaneModel, ProjectionModel, WindowModel, pane_leaves};`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ozmux_tmux model::`
Expected: 2 (Task 1) + 5 (new) = 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/model.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux_session): ProjectionModel + seed_from_rows/apply_event reducer"
```

---

### Task 4: ECS components, index, and reconcile

**Files:** Create `crates/tmux_session/src/components.rs`; create `crates/tmux_session/src/reconcile.rs`; modify `crates/tmux_session/src/lib.rs`.

- [ ] **Step 1: Create `components.rs`**

Create `crates/tmux_session/src/components.rs`:

```rust
//! ECS components mirroring tmux session/window/pane identity + geometry.

use bevy::prelude::Component;
use tmux_control_parser::{CellDims, PaneId, SessionId, WindowId};

/// The projected tmux session entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmuxSession {
    /// tmux session id (`$N`).
    pub id: SessionId,
}

/// A projected tmux window entity (child of [`TmuxSession`]).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct TmuxWindow {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// Whether this is the session's active window.
    pub active: bool,
    /// Window name.
    pub name: String,
}

/// A projected tmux pane entity (child of [`TmuxWindow`]).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmuxPane {
    /// tmux pane id (`%N`).
    pub id: PaneId,
    /// Cell geometry from the window layout.
    pub dims: CellDims,
}
```

- [ ] **Step 2: Create `reconcile.rs`**

Create `crates/tmux_session/src/reconcile.rs`:

```rust
//! Reconciles the [`ProjectionModel`] into ECS entities, maintaining the
//! tmux-id → entity index.

use crate::components::{TmuxPane, TmuxWindow};
use crate::model::ProjectionModel;
use bevy::prelude::*;
use std::collections::HashMap;
use tmux_control_parser::{PaneId, WindowId};

/// Maps tmux ids to their projected entities.
#[derive(Resource, Default)]
pub struct TmuxProjection {
    /// Window id → entity.
    pub windows: HashMap<WindowId, Entity>,
    /// Pane id → entity.
    pub panes: HashMap<PaneId, Entity>,
}

/// Spawns/updates/despawns `TmuxWindow`/`TmuxPane` entities so they match the
/// current [`ProjectionModel`]. Runs only when the model changed.
pub fn reconcile_projection(
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
    model: Res<ProjectionModel>,
) {
    if !model.is_changed() {
        return;
    }
    reconcile_windows(&mut commands, &mut index, &model);
}

fn reconcile_windows(
    commands: &mut Commands,
    index: &mut TmuxProjection,
    model: &ProjectionModel,
) {
    let live_windows: std::collections::HashSet<WindowId> =
        model.windows.iter().map(|w| w.id).collect();
    let live_panes: std::collections::HashSet<PaneId> = model
        .windows
        .iter()
        .flat_map(|w| w.panes.iter().map(|p| p.id))
        .collect();

    index.windows.retain(|id, entity| {
        let keep = live_windows.contains(id);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });
    index.panes.retain(|id, entity| {
        let keep = live_panes.contains(id);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });

    for window in &model.windows {
        match index.windows.get(&window.id) {
            Some(entity) => {
                commands.entity(*entity).insert(TmuxWindow {
                    id: window.id,
                    active: window.active,
                    name: window.name.clone(),
                });
            }
            None => {
                let entity = commands
                    .spawn(TmuxWindow {
                        id: window.id,
                        active: window.active,
                        name: window.name.clone(),
                    })
                    .id();
                index.windows.insert(window.id, entity);
            }
        }
    }
    for window in &model.windows {
        for pane in &window.panes {
            match index.panes.get(&pane.id) {
                Some(entity) => {
                    commands.entity(*entity).insert(TmuxPane {
                        id: pane.id,
                        dims: pane.dims,
                    });
                }
                None => {
                    let entity = commands
                        .spawn(TmuxPane {
                            id: pane.id,
                            dims: pane.dims,
                        })
                        .id();
                    index.panes.insert(pane.id, entity);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PaneModel, WindowModel};
    use tmux_control_parser::CellDims;

    fn dims() -> CellDims {
        CellDims {
            width: 80,
            height: 24,
            xoff: 0,
            yoff: 0,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<ProjectionModel>();
        app.init_resource::<TmuxProjection>();
        app.add_systems(Update, reconcile_projection);
        app
    }

    #[test]
    fn spawns_window_and_pane_entities() {
        let mut app = app();
        app.world_mut().resource_mut::<ProjectionModel>().windows = vec![WindowModel {
            id: WindowId(1),
            active: true,
            name: "main".to_string(),
            panes: vec![PaneModel {
                id: PaneId(9),
                dims: dims(),
            }],
        }];
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert_eq!(index.windows.len(), 1);
        assert_eq!(index.panes.len(), 1);
        let pane_entity = index.panes[&PaneId(9)];
        let pane = app.world().get::<TmuxPane>(pane_entity).unwrap();
        assert_eq!(pane.id, PaneId(9));
    }

    #[test]
    fn despawns_removed_window_and_its_panes() {
        let mut app = app();
        app.world_mut().resource_mut::<ProjectionModel>().windows = vec![WindowModel {
            id: WindowId(1),
            active: true,
            name: "main".to_string(),
            panes: vec![PaneModel {
                id: PaneId(9),
                dims: dims(),
            }],
        }];
        app.update();
        // Remove the window.
        app.world_mut()
            .resource_mut::<ProjectionModel>()
            .windows
            .clear();
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
    }
}
```

NOTE: Phase 1b intentionally spawns windows AND panes as FLAT, independent entities (no `ChildOf` hierarchy) — the index despawns each by id directly, so there is no recursive double-despawn hazard. The `TmuxSession` parent entity and the `ChildOf(session)` / `ChildOf(window)` hierarchy are deferred to Phase 1c, when the model reliably carries `session`. Confirm `Commands::entity(e).despawn()` is the correct Bevy 0.18 spelling against existing repo usage (e.g. `crates/multiplexer`); since entities are flat, plain `despawn` is sufficient.

- [ ] **Step 3: Declare modules + re-exports**

In `crates/tmux_session/src/lib.rs`: add `mod components;` and `mod reconcile;`; re-export `pub use components::{TmuxPane, TmuxSession, TmuxWindow};` and `pub use reconcile::TmuxProjection;`.

- [ ] **Step 4: Run the tests + clippy**

Run: `cargo test -p ozmux_tmux reconcile:: && cargo clippy -p ozmux_tmux -- -D warnings`
Expected: 2 reconcile tests pass; clippy clean. (`TmuxSession` is currently unused in non-test code — if clippy flags it as dead code, that is expected; resolve it by ensuring it is `pub` + re-exported, which makes it public API. Do NOT add `#[allow]`.)

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/components.rs crates/tmux_session/src/reconcile.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux_session): TmuxSession/Window/Pane components + reconcile system"
```

---

### Task 5: Two-phase event pump + plugin wiring + gated test

**Files:** Modify `crates/tmux_session/src/event_pump.rs`, `crates/tmux_session/src/plugin.rs`, `crates/tmux_session/src/lib.rs`; create `crates/tmux_session/tests/real_tmux_projection.rs`.

- [ ] **Step 1: Refactor `event_pump.rs` to two phases**

Replace the contents of `crates/tmux_session/src/event_pump.rs` with:

```rust
//! Draining, logging, and routing of tmux transport events: into
//! `ConnectionState` and the `ProjectionModel`.

use crate::model::ProjectionModel;
use crate::state::{ConnectionState, next_state};
use crossbeam_channel::Receiver;
use tmux_control::{ClientEvent, TransportEvent};

/// Drains every currently-available transport event from `events`, logging
/// each. Non-blocking: returns once the channel is empty for now.
pub(crate) fn drain_transport(events: &Receiver<TransportEvent>) -> Vec<TransportEvent> {
    let mut drained = Vec::new();
    while let Ok(event) = events.try_recv() {
        log_transport_event(&event);
        drained.push(event);
    }
    drained
}

/// Advances `state` through [`next_state`] for each drained event.
pub(crate) fn advance_state(state: &mut ConnectionState, events: &[TransportEvent]) {
    for event in events {
        let next = next_state(state, event);
        if *state != next {
            *state = next;
        }
    }
}

/// Routes notification events into the projection model.
pub(crate) fn route_to_model(model: &mut ProjectionModel, events: &[TransportEvent]) {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::Notification(notification)) = event {
            model.apply_event(notification);
        }
    }
}

/// Emits a `tracing` line describing a single transport event.
fn log_transport_event(event: &TransportEvent) {
    match event {
        TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, .. }) => {
            tracing::debug!(?id, ok, "tmux command complete");
        }
        TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
            tracing::debug!(?notification, "tmux notification");
        }
        TransportEvent::Closed { reason } => {
            tracing::info!(reason, "tmux transport closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;
    use tmux_control::ControlEvent;
    use tmux_control_parser::{PaneId, WindowId, WindowLayout};

    fn window_add(id: u32) -> TransportEvent {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
            window: WindowId(id),
        }))
    }

    #[test]
    fn drain_then_advance_state_attaches() {
        let (tx, rx) = unbounded();
        tx.send(window_add(1)).unwrap();
        let drained = drain_transport(&rx);
        let mut state = ConnectionState::Connecting;
        advance_state(&mut state, &drained);
        assert_eq!(state, ConnectionState::Attached);
    }

    #[test]
    fn route_to_model_applies_notifications() {
        let (tx, rx) = unbounded();
        tx.send(window_add(1)).unwrap();
        tx.send(TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::LayoutChange {
                window: WindowId(1),
                layout: WindowLayout::parse(b"abcd,80x24,0,0,4").unwrap(),
                visible_layout: WindowLayout::parse(b"abcd,80x24,0,0,4").unwrap(),
                flags: String::new(),
            },
        )))
        .unwrap();
        let drained = drain_transport(&rx);
        let mut model = ProjectionModel::default();
        route_to_model(&mut model, &drained);
        assert_eq!(model.windows.len(), 1);
        assert_eq!(model.windows[0].panes.len(), 1);
        assert_eq!(model.windows[0].panes[0].id, PaneId(4));
    }
}
```

- [ ] **Step 2: Rewire the plugin**

Replace the body of `TmuxSessionPlugin::build` and the drain system in `crates/tmux_session/src/plugin.rs`. The new `plugin.rs`:

```rust
//! The `TmuxSessionPlugin`: connection state, projection, and the per-frame
//! event-drain + reconcile systems.

use crate::connection::TmuxConnection;
use crate::event_pump::{advance_state, drain_transport, route_to_model};
use crate::model::ProjectionModel;
use crate::reconcile::{TmuxProjection, reconcile_projection};
use crate::state::ConnectionState;
use bevy::prelude::*;

/// Wires the tmux integration into the Bevy app: connection state, the
/// projection model + index, the per-frame drain system, and the reconcile
/// system. Phase 1b does not auto-connect.
pub struct TmuxSessionPlugin;

impl Plugin for TmuxSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectionState>();
        app.init_resource::<ProjectionModel>();
        app.init_resource::<TmuxProjection>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_systems(Update, (drain_tmux_events, reconcile_projection).chain());
    }
}

/// Drains the live connection's transport events each frame, advancing
/// `ConnectionState` and routing notifications into the `ProjectionModel`.
fn drain_tmux_events(
    mut state: ResMut<ConnectionState>,
    mut model: ResMut<ProjectionModel>,
    connection: NonSend<TmuxConnection>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    let events = drain_transport(client.events());
    if events.is_empty() {
        return;
    }
    advance_state(&mut state, &events);
    route_to_model(&mut model, &events);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_resources_and_stays_idle_without_connection() {
        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.update();
        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Idle
        );
        assert!(
            app.world()
                .resource::<ProjectionModel>()
                .windows
                .is_empty()
        );
    }
}
```

NOTE: the `if events.is_empty() { return; }` guard is load-bearing for the `reconcile_projection` change-detection optimization. On idle frames (no events) the system returns BEFORE taking `&mut model`, so `ProjectionModel` is not deref-mutated and not marked changed, and `reconcile`'s `model.is_changed()` guard correctly skips. On frames WITH events, `route_to_model` derefs the model mutably (marking it changed) and reconcile runs. `reconcile` is idempotent regardless, so `is_changed()` is an optimization, not a correctness dependency.

- [ ] **Step 3: Update the Phase 0 re-export if needed**

`event_pump`'s old `drain_events` is gone. Confirm nothing else references `drain_events` (it was `pub(crate)`, only used by the plugin). `grep -rn "drain_events" crates/tmux_session/src` should show no remaining references. Update `lib.rs` only if it re-exported `drain_events` (it did not).

- [ ] **Step 4: Run all crate tests + clippy**

Run: `cargo test -p ozmux_tmux && cargo clippy -p ozmux_tmux -- -D warnings`
Expected: all unit tests pass (model, enumerate, reconcile, event_pump, plugin, select, state) and clippy is clean.

- [ ] **Step 5: Gated real-tmux projection test**

Create `crates/tmux_session/tests/real_tmux_projection.rs`:

```rust
//! Gated end-to-end test: connect to a real tmux and verify the projection
//! model populates from the live notification stream.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_projection -- --ignored`.

use bevy::prelude::*;
use ozmux_tmux::{ConnectionState, ProjectionModel, TmuxConnection, TmuxSessionPlugin};
use std::time::{Duration, Instant};
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn projection_populates_from_real_tmux() {
    let socket = format!("ozmux-phase1b-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);
    let client = server.new_session().expect("spawn tmux -CC new-session");

    let mut app = App::new();
    app.add_plugins(TmuxSessionPlugin);
    app.world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection inserted by the plugin")
        .set(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut has_window = false;
    while Instant::now() < deadline {
        app.update();
        if !app.world().resource::<ProjectionModel>().windows.is_empty() {
            has_window = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert_eq!(
        *app.world().resource::<ConnectionState>(),
        ConnectionState::Attached
    );
    assert!(
        has_window,
        "the projection should gain at least one window from the attach notifications"
    );

    if let Some(client) = app
        .world_mut()
        .get_non_send_resource_mut::<TmuxConnection>()
        .expect("TmuxConnection present")
        .take()
    {
        client.handle().send("kill-server").ok();
    }
}
```

Run: `cargo test -p ozmux_tmux --test real_tmux_projection --no-run` (must compile).
If tmux is installed: `cargo test -p ozmux_tmux --test real_tmux_projection -- --ignored`.
Expected: passes — a `%window-add` notification on attach gives the model a window. If tmux is absent, note it; not a failure.

NOTE: a fresh `new-session` emits `%window-add` for its initial window on attach (per the control-mode attach burst), so `windows` becomes non-empty without any `list-windows` query. If this proves flaky (timing/version), the test may need Phase 1c's explicit enumeration — if so, mark the test `#[ignore]` (already is) and record the observation in your report rather than weakening the assertion.

- [ ] **Step 6: Commit**

```bash
git add crates/tmux_session/src/event_pump.rs crates/tmux_session/src/plugin.rs crates/tmux_session/tests/real_tmux_projection.rs
git commit -m "feat(tmux_session): two-phase pump routes notifications into the projection"
```

---

## Done criteria for Phase 1b

- `pane_leaves`, `parse_window_rows`, `seed_from_rows`, and `apply_event` are pure and unit-tested (layout flattening, list-windows parsing, the full reducer).
- `reconcile_projection` spawns/updates/despawns `TmuxWindow`/`TmuxPane` entities to match `ProjectionModel`, maintaining the `TmuxProjection` index; headless-tested.
- The event pump is two-phase: drain → advance `ConnectionState` → route notifications into `ProjectionModel`; reconcile runs chained after it.
- Gated real-tmux test confirms the projection gains a window on attach.
- `cargo test -p ozmux_tmux` passes; `cargo clippy -p ozmux_tmux -- -D warnings` clean; binary still builds; no rendering added.

## Next: Phase 1c — initial enumeration + command correlation

Send `list-windows -F LIST_WINDOWS_FORMAT` once on entering `Attached`, remember the `CommandId`, and on the matching `CommandComplete { id, ok, output }` call `ProjectionModel::seed_from_rows(&parse_window_rows(&output)?)` — so pre-existing windows' full layouts populate immediately (not only on the next `%layout-change`). Also spawn the `TmuxSession` parent entity and parent windows under it (`ChildOf(session)`). Then Phase 2 renders the projected panes.
```
