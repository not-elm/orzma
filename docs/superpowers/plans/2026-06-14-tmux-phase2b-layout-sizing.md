# tmux Phase 2b — Multi-pane layout + window sizing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Lay out the active tmux window's panes by their tmux cell geometry (absolute positioned), resize each pane's grid to match, and tell tmux the GUI window's cell size via `refresh-client` so tmux re-lays-out to fit.

**Architecture:** The ECS projection hierarchy becomes a real `ChildOf` tree (`reconcile` parents panes under their window entity). The binary mirrors that tree into the UI: `WorkspaceUiRoot → window-container Node → pane Node`; only the active window's container is `Display::Flex`. A layout system positions each pane Node from `TmuxPane.dims × cell-pixel-size` and calls a new PTY-less `TerminalHandle::resize_grid_only`. A sizing system converts the GUI window pixel size → `(cols, rows)` and sends `refresh-client -C W,H` (typed builder in `ozmux_tmux`); tmux re-emits `%layout-change`, closing the loop.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS + UI, alacritty_terminal, tmux control mode.

**Spec:** `docs/superpowers/specs/2026-06-14-tmux-phase2-pane-rendering-design.md` (Phase 2b sections).

**Builds on Phase 2a** (`tmux-phase2a` branch): `TerminalHandle::detached`/`flush_emit`, `PaneOutput`, `TmuxProjectionSet`, `src/tmux_render.rs` (`attach_tmux_pane_terminal` + `route_tmux_output`), startup session picker.

**Decisions settled:**
- Active-window membership is expressed via **`ChildOf`** (reconcile parents pane→window; UI tree mirrors tmux). Only the active window renders (its container Node toggled).
- `refresh-client` first cut: **send + react to `%layout-change` only**. If a live test shows tmux does not re-emit layout after a control-client resize, add a `list-windows` re-query fallback later (tracked by a `// TODO:`).
- Bare client size uses the **`W,H`** form (accepted on all tmux versions; no version floor).

**Conventions:** `.claude/rules/rust.md` — `//!` on module files; comments only `// TODO:`/`// NOTE:`/`// SAFETY:`; `///` on every `pub`; imports contiguous; mutable params before immutable; private items after public; minimize visibility.

---

## Coordinate facts (verified)

- `TerminalCellMetricsResource { metrics: CellMetrics, phys_font_size }` (`ozma_tty_renderer::TerminalCellMetricsResource`). `metrics.advance_phys` (cell width), `metrics.line_height_phys` (cell height), `metrics.max_overflow_phys` — all **physical px**, DPR baked in.
- px → cells: `ozma_tty_renderer`'s `compute_grid_dims(w_px, h_px, cell_w_phys, cell_h_phys, max_overflow_phys) -> (u16, u16)` — used by the existing `resize_terminals_to_node` (`src/ui/terminal.rs:194`). It is currently private to the renderer crate; if not public, replicate its flooring math locally (cols = floor(w_px / cell_w_phys), rows = floor(h_px / cell_h_phys), clamped ≥1). Verify visibility and either use it or inline the simple floor.
- cells → logical px (for `Node` `Val::Px`, which is logical): `px_logical = cell_count * cell_phys / scale_factor`, where `scale_factor = window.scale_factor()`.
- `TmuxPane.dims: CellDims { width, height, xoff, yoff }` (all `u32`; `xoff`/`yoff` are cell offsets within the window).
- `Window` (primary) gives pixel size via `window.resolution.physical_width()/physical_height()` and `window.scale_factor()`.

---

## Task 1: `TerminalHandle::resize_grid_only` (engine)

**Files:** Modify `crates/ozma_tty_engine/src/handle.rs`

- [ ] **Step 1: Write the failing test** — append to `handle.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn resize_grid_only_changes_geometry_without_pty() {
    let mut h = TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false)));
    h.resize_grid_only(40, 10);
    let (cols, rows, _) = h.read_geometry();
    assert_eq!((cols, rows), (40, 10));
}
```

- [ ] **Step 2: Run, expect fail** — `cargo test -p ozma_tty_engine resize_grid_only_changes_geometry_without_pty` → FAIL (no method).

- [ ] **Step 3: Implement** — in `impl TerminalHandle`, among the public methods (near `flush_emit`), add:

```rust
/// Resizes the alacritty grid only — no PTY, no echo to any backend.
///
/// For tmux panes, tmux owns the pane size; this applies the size tmux
/// reported (`%layout-change`) to the local grid. Stages full damage so the
/// reflowed grid reaches the renderer even when no output is pending.
/// Unlike [`TerminalHandle::resize`], it must NOT touch a PTY or echo the
/// size anywhere (echoing back to tmux would loop).
pub fn resize_grid_only(&mut self, cols: u16, rows: u16) {
    self.resize_grid(cols, rows);
    let mut scratch = std::mem::take(&mut self.scratch_dirty);
    self.pending_damage = Some(DirtyRows::collect(&mut self.term, &mut scratch));
    self.scratch_dirty = scratch;
}
```

(`resize_grid` is the existing private method at `handle.rs:960` doing the alacritty-only resize + hash clear. `DirtyRows` is already imported. NOTE: bare `resize_grid` does NOT stage damage — this method adds the staging, mirroring `force_bootstrap_damage`. The staged damage is emitted on the next `flush_emit`.)

- [ ] **Step 4: Run, expect pass** — same command → PASS.
- [ ] **Step 5: clippy + fmt + commit**

```bash
cargo clippy -p ozma_tty_engine --all-targets && cargo fmt
git add crates/ozma_tty_engine/src/handle.rs
git commit -m "feat(ozma_tty_engine): resize_grid_only for PTY-less pane reflow"
```

---

## Task 2: `refresh_client_command` builder (ozmux_tmux)

**Files:** Modify `crates/tmux_session/src/enumerate.rs` (it already hosts `list_windows_command`), export from `lib.rs`.

- [ ] **Step 1: Write the failing test** — in `enumerate.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn refresh_client_command_uses_comma_size_form() {
    assert_eq!(refresh_client_command(80, 24), "refresh-client -C 80,24");
}
```

- [ ] **Step 2: Run, expect fail** — `cargo test -p ozmux_tmux refresh_client_command_uses_comma_size_form`.

- [ ] **Step 3: Implement** — near `list_windows_command` in `enumerate.rs`:

```rust
/// Builds a `refresh-client -C <cols>,<rows>` control-mode command that tells
/// tmux this client's cell size. The bare `W,H` form is accepted by all tmux
/// versions (the `WxH` form is only required for the `@id:WxH` per-window
/// variant, which Phase 2b does not use).
pub(crate) fn refresh_client_command(cols: u16, rows: u16) -> String {
    format!("refresh-client -C {cols},{rows}")
}
```

Then re-export it for the binary: in `crates/tmux_session/src/lib.rs` add `refresh_client_command` to the `enumerate` re-export line (it currently re-exports `LIST_WINDOWS_FORMAT, WindowRow, parse_window_rows`). Change `refresh_client_command` to `pub` (not `pub(crate)`) since the binary calls it; update the doc accordingly. So: `pub fn refresh_client_command(...)` and `pub use enumerate::{..., refresh_client_command};`.

- [ ] **Step 4: Run, expect pass.**
- [ ] **Step 5: clippy + fmt + commit**

```bash
git add crates/tmux_session/src/enumerate.rs crates/tmux_session/src/lib.rs
git commit -m "feat(ozmux_tmux): refresh_client_command builder (W,H form)"
```

---

## Task 3: reconcile parents panes under windows (ozmux_tmux)

**Files:** Modify `crates/tmux_session/src/reconcile.rs`

Goal: each pane entity becomes `ChildOf(window_entity)`; despawn ordering avoids double-despawn under Bevy 0.18's descendant-aware `despawn`.

- [ ] **Step 1: Write the failing test** — in `reconcile.rs` `#[cfg(test)] mod tests`, add a test asserting a spawned pane is a child of its window entity:

```rust
#[test]
fn pane_is_child_of_its_window() {
    let mut app = app();
    app.world_mut().resource_mut::<ProjectionModel>().windows = vec![WindowModel {
        id: WindowId(1),
        active: true,
        name: "main".to_string(),
        panes: vec![PaneModel { id: PaneId(9), dims: dims() }],
    }];
    app.update();
    let index = app.world().resource::<TmuxProjection>();
    let window_entity = index.windows[&WindowId(1)];
    let pane_entity = index.panes[&PaneId(9)];
    let child_of = app.world().get::<ChildOf>(pane_entity).expect("pane has ChildOf");
    assert_eq!(child_of.parent(), window_entity);
}
```

(`ChildOf` comes from `bevy::prelude`; the test helpers `app()`, `dims()` already exist in this module.)

- [ ] **Step 2: Run, expect fail.**

- [ ] **Step 3: Implement** — two changes in `reconcile_windows`:

(a) **Despawn panes before windows** so a window despawn never cascades onto an already-handled pane. Reorder so `index.panes.retain(...)` runs BEFORE `index.windows.retain(...)`. (Currently windows-retain is first.)

(b) When spawning/updating a pane, set its parent to the window entity. The window entity is `index.windows[&window.id]` (it was inserted in the windows loop, which now must run BEFORE the panes loop — it already does). For each pane:

```rust
for window in &model.windows {
    let window_entity = index.windows[&window.id];
    for pane in &window.panes {
        match index.panes.get(&pane.id) {
            Some(entity) => {
                commands.entity(*entity).insert((
                    TmuxPane { id: pane.id, dims: pane.dims },
                    ChildOf(window_entity),
                ));
            }
            None => {
                let entity = commands
                    .spawn((TmuxPane { id: pane.id, dims: pane.dims }, ChildOf(window_entity)))
                    .id();
                index.panes.insert(pane.id, entity);
            }
        }
    }
}
```

(Re-inserting `ChildOf` every reconcile is idempotent and also handles a pane moving between windows.) Add `use bevy::prelude::*;` already present? reconcile.rs uses `use bevy::prelude::*;` — confirm `ChildOf` resolves; if not, add the explicit import.

NOTE on despawn: with panes-first retain, dead panes are despawned and removed from `index.panes`; then dead windows are despawned. A dead window may still have live-in-model? No — if a window is gone from the model, its panes are gone too, so they were despawned in the panes pass. The window then despawns with no tracked children. If Bevy logs a warning about despawning a parent with children, it is benign, but the panes-first order prevents the double-despawn of tracked pane entities.

- [ ] **Step 4: Run, expect pass** — also run the whole `cargo test -p ozmux_tmux` to ensure existing reconcile tests (spawn/despawn) still pass.
- [ ] **Step 5: clippy + fmt + commit**

```bash
git add crates/tmux_session/src/reconcile.rs
git commit -m "feat(ozmux_tmux): parent panes under their window entity via ChildOf"
```

---

## Task 4: window container Nodes + active toggle (binary)

**Files:** Modify `src/tmux_render.rs`

Each `TmuxWindow` entity gets a full-window container `Node` (child of `WorkspaceUiRoot`); only the active window's container is shown.

- [ ] **Step 1: Add an attach system for windows.** In `src/tmux_render.rs`, add a system that gives each `TmuxWindow` lacking a `Node` a container Node under `WorkspaceUiRoot`:

```rust
fn attach_tmux_window_container(
    mut commands: Commands,
    windows: Query<Entity, (With<TmuxWindow>, Without<Node>)>,
    ui_root: Query<Entity, With<WorkspaceUiRoot>>,
) {
    let Ok(root) = ui_root.single() else { return; };
    for window in windows.iter() {
        commands.entity(window).insert((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ChildOf(root),
        ));
    }
}
```

- [ ] **Step 2: Add an active-toggle system** driven by `TmuxWindow.active`:

```rust
fn sync_active_window(mut windows: Query<(&TmuxWindow, &mut Node)>) {
    for (w, mut node) in windows.iter_mut() {
        let want = if w.active { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
}
```

- [ ] **Step 3: Register both** in `OzmuxTmuxRenderPlugin::build`, ordered after `TmuxProjectionSet` and before/with the pane systems. `attach_tmux_window_container` must run before `attach_tmux_pane_terminal` (panes' `ChildOf(window)` needs the window present; the window Node parents the pane Node). Use `.chain()`:

```rust
app.add_systems(
    Update,
    (
        attach_tmux_window_container,
        attach_tmux_pane_terminal,
        route_tmux_output,
        sync_active_window,
    )
        .chain()
        .after(TmuxProjectionSet),
);
```

Import `TmuxWindow` from `ozmux_tmux` and `Display` from `bevy::prelude`. (Task 5 inserts `layout_tmux_panes` into this chain after `sync_active_window`; do not reference it here yet — it does not exist until Task 5.)

- [ ] **Step 4: build + clippy + fmt + commit** (no unit test here — exercised in Task 6 integration / manual). `cargo build` must pass.

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux): per-window container nodes; show only the active window"
```

---

## Task 5: pane layout + grid resize (binary)

**Files:** Modify `src/tmux_render.rs`

Change `attach_tmux_pane_terminal` so panes are NOT full-window and NOT re-parented to `WorkspaceUiRoot` (reconcile now parents them to their window). Add `layout_tmux_panes` that positions each pane Node from its `dims` and resizes its grid.

- [ ] **Step 1: Update `attach_tmux_pane_terminal`** — remove the `ChildOf(root)` insert and the `WorkspaceUiRoot` query (panes inherit `ChildOf(window)` from reconcile). Replace the full-window `Node` with an absolute zero-size placeholder Node (the layout system sets the real rect each frame):

```rust
fn attach_tmux_pane_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    panes: Query<(Entity, &TmuxPane), Without<TerminalHandle>>,
) {
    for (entity, pane) in panes.iter() {
        let cols = pane.dims.width.max(1) as u16;
        let rows = pane.dims.height.max(1) as u16;
        let handle = TerminalHandle::detached(cols, rows, Arc::new(AtomicBool::new(false)));
        let material = materials.add(TerminalUiMaterial::default());
        commands.entity(entity).insert((
            handle,
            TerminalRenderBundle::new(material),
            Node { position_type: PositionType::Absolute, ..default() },
        ));
    }
}
```

- [ ] **Step 2: Add `layout_tmux_panes`** — positions each pane Node from `dims × cell size` (logical px) and resizes the grid when dims change:

```rust
fn layout_tmux_panes(
    mut panes: Query<(&TmuxPane, &mut Node, &mut TerminalHandle)>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else { return; };
    let dpr = window.scale_factor().max(0.5);
    let cell_w = (metrics.metrics.advance_phys.floor().max(1.0)) / dpr;
    let cell_h = (metrics.metrics.line_height_phys.floor().max(1.0)) / dpr;
    for (pane, mut node, mut handle) in panes.iter_mut() {
        let d = pane.dims;
        node.left = Val::Px(d.xoff as f32 * cell_w);
        node.top = Val::Px(d.yoff as f32 * cell_h);
        node.width = Val::Px(d.width as f32 * cell_w);
        node.height = Val::Px(d.height as f32 * cell_h);
        let (cols, rows) = (d.width.max(1) as u16, d.height.max(1) as u16);
        let (cur_cols, cur_rows, _) = handle.read_geometry();
        if (cur_cols, cur_rows) != (cols, rows) {
            handle.resize_grid_only(cols, rows);
        }
    }
}
```

Imports: `TerminalCellMetricsResource` (`ozma_tty_renderer`), `Window`, `PrimaryWindow` (`bevy::prelude` / `bevy::window`). `layout_tmux_panes` runs unconditionally each frame (cheap; positions are idempotent). **Insert it into the Task-4 chain** right after `sync_active_window` (i.e. `(attach_tmux_window_container, attach_tmux_pane_terminal, route_tmux_output, sync_active_window, layout_tmux_panes).chain().after(TmuxProjectionSet)`).

NOTE: running every frame re-writes `Node` fields each tick, which Bevy change-detection treats as changed only if the value differs (Bevy `DerefMut` always marks changed though — acceptable here; if it causes layout churn, gate the writes behind an inequality check like the grid resize does). Keep it simple first; optimize only if profiling shows churn.

- [ ] **Step 3: build + clippy + fmt + commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux): absolute cell-dim pane layout + grid resize on dims change"
```

---

## Task 6: window-size → refresh-client (binary)

**Files:** Modify `src/tmux_render.rs`

Convert the GUI window pixel size to `(cols, rows)` and send `refresh-client -C W,H` to tmux when it changes.

- [ ] **Step 1: Add a `LastClientSize` resource** (binary, in `tmux_render.rs`) to debounce at cell granularity:

```rust
#[derive(Resource, Default)]
struct LastClientSize {
    cols: u16,
    rows: u16,
}
```

`app.init_resource::<LastClientSize>();` in the plugin.

- [ ] **Step 2: Add the sizing system:**

```rust
fn sync_client_size(
    mut last: ResMut<LastClientSize>,
    connection: NonSend<TmuxConnection>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(client) = connection.client() else { return; };
    let Ok(window) = window.single() else { return; };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let w_px = window.resolution.physical_width() as f32;
    let h_px = window.resolution.physical_height() as f32;
    let cols = ((w_px / cell_w).floor() as u16).max(1);
    let rows = ((h_px / cell_h).floor() as u16).max(1);
    if (cols, rows) == (last.cols, last.rows) {
        return;
    }
    last.cols = cols;
    last.rows = rows;
    if let Err(e) = client.handle().send(&refresh_client_command(cols, rows)) {
        tracing::warn!(?e, cols, rows, "refresh-client send failed");
    }
}
```

Imports: `TmuxConnection`, `refresh_client_command` (`ozmux_tmux`). `connection.client()` returns `Option<&TmuxClient>`; `client.handle().send(&str)` returns `TmuxResult<CommandId>`. Register `sync_client_size` in the plugin under `Update` (it does not need to be in the projection chain — it only reads window + sends; add it as a separate `.add_systems(Update, sync_client_size)` or include in the tuple, after `layout_tmux_panes` is fine). It runs every frame but only sends when the integer cell size changes.

- [ ] **Step 3: build + clippy + fmt + commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux): refresh-client window sizing on cell-size change"
```

---

## Task 7: integration test + verify

**Files:** Modify `src/tmux_render.rs` (test)

- [ ] **Step 1: Extend the headless test** (the Phase 2a `output_routed_into_pane_grid_renders_text` test exists). Add a test that a pane Node gets positioned from dims. Build an app with `MinimalPlugins`, insert a `TerminalCellMetricsResource` with known `advance_phys`/`line_height_phys` (e.g. 8.0 / 16.0) and a `Window` (PrimaryWindow) with `scale_factor 1.0`, spawn a `TmuxPane` with `dims { width: 10, height: 4, xoff: 2, yoff: 1 }` carrying a `Node` + `TerminalHandle::detached`, run `layout_tmux_panes` once, and assert `node.left == Val::Px(16.0)`, `node.top == Val::Px(16.0)`, `node.width == Val::Px(80.0)`, `node.height == Val::Px(64.0)`. Construct `TerminalCellMetricsResource` and `CellMetrics` via their public fields (check `ozma_tty_renderer::CellMetrics` field list and fill the rest with 0.0). If constructing `CellMetrics` is awkward, instead extract the cell→px math into a pure `fn cell_rect(dims, cell_w, cell_h) -> (f32,f32,f32,f32)` and unit-test THAT (preferred — cleaner and avoids the metrics struct).

- [ ] **Step 2: Run filtered** — `cargo test -p ozmux-gui tmux_render` (filtered; the full binary has a known CEF segfault). All pass.

- [ ] **Step 3: Full check** — `cargo build`, `cargo clippy --workspace --all-targets`, `cargo fmt --check`, and `cargo test -p ozma_tty_engine -p ozmux_tmux -p ozmux_configs`.

- [ ] **Step 4: Manual GUI verification** (requires a desktop; run from OUTSIDE the tmux session you attach to). Start a multi-pane session, e.g.:
  ```
  tmux kill-server 2>/dev/null
  tmux new-session -d -s test
  tmux split-window -h -t test
  tmux send-keys -t test:0.0 'echo LEFT' Enter
  tmux send-keys -t test:0.1 'echo RIGHT' Enter
  cargo run
  ```
  Pick `test` in the session picker. Expected: two panes side by side, each showing its echo, laid out by tmux's split geometry. Resize the GUI window → tmux should re-lay-out (`%layout-change`) and the panes should track the new size. If panes do NOT track window resize, tmux did not re-emit layout after `refresh-client` — add the `list-windows` re-query fallback (see spec) as a follow-up and note it.

- [ ] **Step 5: commit any fixes**

```bash
git add -A && git commit -m "test(tmux): pane layout math + phase 2b verification" || echo "nothing to commit"
```

---

## Out of scope (Phase 3 / later)

- Keyboard/mouse input to panes, reply routing to tmux, click-to-focus, focus/dim → Phase 3.
- `%layout-change`-after-`refresh-client` fallback (`list-windows` re-query) → only if the live test shows it is needed.
- Multi-window tab strip UI, pane borders/gutters → later.
