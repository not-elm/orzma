# iTerm-Style Per-Pane Title Bar — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fixed-height (`cell_h` px) title bar to every tmux pane that displays `TerminalTitle`, absorbing the blank-strip artefact that appears when pane height is not an exact multiple of `cell_h`.

**Architecture:** Each `TmuxPane` becomes a Column-flex container; its two children are `PaneTitleBar` (fixed height T = cell_h) and `TerminalRenderChild` (flex_grow=1, owns `TerminalGrid` + `MaterialNode`). `sync_client_size` subtracts `vertical_depth(layout)` rows so tmux's row budget leaves room for title bars. `collapse()` adds T to each Leaf rect so the container nodes are sized correctly.

**Tech Stack:** Rust / Bevy 0.18 ECS, `bevy::ui::{Node, FlexDirection, Val}`, `ozma_tty_renderer::material::PaneDim`, `ozma_tty_engine::{TerminalHandle, TerminalTitle}`, `ozmux_tmux::{TmuxPane, ActivePane, TmuxWindowLayout}`, `tmux_control_parser::{Cell, SplitDir}`.

## Global Constraints

- Edition 2024, Rust toolchain 1.95 (`rust-toolchain.toml`). Run `cargo check` to verify compilation.
- Test command: `cargo test -p ozmux-gui <test_name>` (binary crate is `ozmux-gui`).
- `run_if` not in-body early-returns for whole-system change guards.
- Mutable parameters before immutable in every function signature.
- No `mod.rs` files. No plain narrative comments — only `// TODO:`, `// NOTE:`, `// SAFETY:`.
- Every new `pub` item needs a `///` doc comment. File-level `//!` on new files.
- Keep private items after public ones within each `impl`/module.
- After every task: run the specified test command and `cargo check`. Fix any compile errors before committing.

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `src/tmux_render.rs` | Modify | `vertical_depth`, `collapse()` pane_title_h, `TerminalRenderRef`, `attach_tmux_pane_terminal` refactor, `route_tmux_output` fix, `layout_tmux_panes` update, `sync_client_size` depth subtraction |
| `src/ui/tmux_pane_title.rs` | Create | `PaneTitleBar` component, `OzmuxTmuxPaneTitlePlugin`, title-sync systems |
| `src/ui.rs` | Modify | Add `pub(crate) mod tmux_pane_title;` |
| `src/main.rs` | Modify | Register `OzmuxTmuxPaneTitlePlugin` |
| `src/ui/tmux_pane_focus.rs` | Modify | `sync_pane_dim` targets `TerminalRenderChild` via `TerminalRenderRef`; `augment_tmux_pane` adds `FocusPolicy::Block` to `PaneTitleBar` |

---

### Task 1: `vertical_depth` function + unit tests

**Files:**
- Modify: `src/tmux_render.rs`

**Interfaces:**
- Produces: `fn vertical_depth(cell: &Cell) -> u16` — Leaf→1, LeftRight→max(children), TopBottom→sum(children), Floating→1

- [ ] **Step 1: Add `vertical_depth` after `last_pane_id` in `src/tmux_render.rs` (around line 349)**

```rust
fn vertical_depth(cell: &Cell) -> u16 {
    match cell {
        Cell::Leaf { .. } => 1,
        Cell::Split {
            dir: SplitDir::LeftRight,
            children,
            ..
        } => children.iter().map(vertical_depth).max().unwrap_or(1),
        Cell::Split {
            dir: SplitDir::TopBottom,
            children,
            ..
        } => children.iter().map(vertical_depth).sum(),
        Cell::Split {
            dir: SplitDir::Floating,
            ..
        } => 1,
    }
}
```

- [ ] **Step 2: Add unit tests at the end of the `#[cfg(test)] mod tests` block (before the closing `}`)**

```rust
#[test]
fn vertical_depth_leaf_is_one() {
    let leaf = Cell::Leaf {
        dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
        pane_id: Some(1),
    };
    assert_eq!(vertical_depth(&leaf), 1);
}

#[test]
fn vertical_depth_left_right_is_max_of_children() {
    let root = Cell::Split {
        dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
        dir: SplitDir::LeftRight,
        children: vec![
            Cell::Leaf { dims: CellDims { width: 40, height: 24, xoff: 0, yoff: 0 }, pane_id: Some(1) },
            Cell::Split {
                dims: CellDims { width: 39, height: 24, xoff: 41, yoff: 0 },
                dir: SplitDir::TopBottom,
                children: vec![
                    Cell::Leaf { dims: CellDims { width: 39, height: 12, xoff: 41, yoff: 0 }, pane_id: Some(2) },
                    Cell::Leaf { dims: CellDims { width: 39, height: 11, xoff: 41, yoff: 13 }, pane_id: Some(3) },
                ],
            },
        ],
    };
    assert_eq!(vertical_depth(&root), 2, "LeftRight takes max: left=1, right=2");
}

#[test]
fn vertical_depth_top_bottom_is_sum_of_children() {
    let root = Cell::Split {
        dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
        dir: SplitDir::TopBottom,
        children: vec![
            Cell::Leaf { dims: CellDims { width: 80, height: 12, xoff: 0, yoff: 0 }, pane_id: Some(1) },
            Cell::Leaf { dims: CellDims { width: 80, height: 11, xoff: 0, yoff: 13 }, pane_id: Some(2) },
        ],
    };
    assert_eq!(vertical_depth(&root), 2, "TopBottom: 1+1=2");
}

#[test]
fn vertical_depth_nested_top_bottom_sums_recursively() {
    let inner = Cell::Split {
        dims: CellDims { width: 80, height: 12, xoff: 0, yoff: 0 },
        dir: SplitDir::TopBottom,
        children: vec![
            Cell::Leaf { dims: CellDims { width: 80, height: 6, xoff: 0, yoff: 0 }, pane_id: Some(2) },
            Cell::Leaf { dims: CellDims { width: 80, height: 5, xoff: 0, yoff: 7 }, pane_id: Some(3) },
        ],
    };
    let root = Cell::Split {
        dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
        dir: SplitDir::TopBottom,
        children: vec![
            Cell::Leaf { dims: CellDims { width: 80, height: 11, xoff: 0, yoff: 0 }, pane_id: Some(1) },
            inner,
        ],
    };
    assert_eq!(vertical_depth(&root), 3, "TopBottom(Leaf, TopBottom(Leaf, Leaf)) = 1+2 = 3");
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p ozmux-gui vertical_depth
```
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux_render): add vertical_depth layout tree helper"
```

---

### Task 2: `collapse()` / `place()` — add `pane_title_h` parameter

**Files:**
- Modify: `src/tmux_render.rs`

**Interfaces:**
- Consumes: `vertical_depth` (Task 1)
- Produces: `collapse(root, cell_w, cell_h, gap, pane_title_h) -> (HashMap<PaneId,Rect>, Vec<DividerPixelRect>, Vec2)` where bbox.y = max pane rect max.y (accounts for title bars). Each Leaf rect now has `.height() = dims.height × cell_h + pane_title_h`.

- [ ] **Step 1: Thread `pane_title_h: f32` through `collapse` and `place`**

Replace the `collapse` function signature and body (lines ~204–225):

```rust
fn collapse(
    root: &Cell,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
    pane_title_h: f32,
) -> (HashMap<PaneId, Rect>, Vec<DividerPixelRect>, Vec2) {
    let dims = root.dims();
    let available = Vec2::new(dims.width as f32 * cell_w, dims.height as f32 * cell_h);
    let mut panes = HashMap::new();
    let mut dividers = Vec::new();
    place(
        &mut panes,
        &mut dividers,
        root,
        Vec2::ZERO,
        available,
        cell_w,
        cell_h,
        gap,
        pane_title_h,
    );
    let actual_h = panes.values().map(|r| r.max.y).fold(0.0f32, f32::max);
    let bbox = Vec2::new(available.x, actual_h.max(available.y));
    (panes, dividers, bbox)
}
```

Replace `place` signature (line ~227) — add `pane_title_h: f32` as the last parameter and thread it through every recursive call:

```rust
fn place(
    mut panes: &mut HashMap<PaneId, Rect>,
    mut dividers: &mut Vec<DividerPixelRect>,
    cell: &Cell,
    origin: Vec2,
    available: Vec2,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
    pane_title_h: f32,
) -> Vec2 {
```

In the `Cell::Leaf` arm, change:
```rust
// Old:
let size = Vec2::new(dims.width as f32 * cell_w, dims.height as f32 * cell_h);
let node_size = Vec2::new(available.x.max(size.x), available.y.max(size.y));
// New:
let node_size = Vec2::new(
    available.x.max(dims.width as f32 * cell_w),
    available.y.max(dims.height as f32 * cell_h + pane_title_h),
);
```

Add `pane_title_h` to every recursive `place(...)` call (LeftRight arm, TopBottom arm, Floating arm).

The parameter order must be: mutable first (`panes`, `dividers`), then immutable. The signature above is already correct (`mut panes: &mut ...` is mutable via reborrow, same for `dividers`). Keep the ordering: `(mut panes, mut dividers, cell, origin, available, cell_w, cell_h, gap, pane_title_h)`.

- [ ] **Step 2: Fix the one existing `collapse` call-site (in `layout_tmux_panes`, around line 386)**

Change:
```rust
let (rects, dividers, bbox) = collapse(&layout.0.root, cell_w, cell_h, theme::PANE_GAP_PX);
```
To (temporarily pass 0.0 to preserve existing behavior; Task 5 will supply the real value):
```rust
let (rects, dividers, bbox) = collapse(&layout.0.root, cell_w, cell_h, theme::PANE_GAP_PX, 0.0);
```

- [ ] **Step 3: Update all 5 existing `collapse(...)` calls in tests to pass `pane_title_h = 0.0`**

Search for `collapse(&root,` in the `mod tests` block and add `, 0.0` before the closing `)` of each call:

```rust
// collapse_single_pane_fills_available
let (rects, _, bbox) = collapse(&root, 8.0, 16.0, 1.0, 0.0);

// collapse_left_right_produces_one_px_gap
let (rects, dividers, bbox) = collapse(&root, 8.0, 16.0, 1.0, 0.0);

// collapse_nested_split_fills_without_blank_strips
let (rects, _, bbox) = collapse(&root, 8.0, 16.0, 1.0, 0.0);

// collapse_compound_non_last_child_no_overlap
let (rects, dividers, bbox) = collapse(&root, 8.0, 16.0, 1.0, 0.0);

// collapse_skips_leaf_without_pane_id
let (rects, _, _) = collapse(&root, 8.0, 16.0, 1.0, 0.0);
```

- [ ] **Step 4: Add new test verifying title-bar height is added to each Leaf**

```rust
#[test]
fn collapse_with_title_h_adds_t_to_every_leaf() {
    let root = Cell::Split {
        dims: CellDims { width: 80, height: 22, xoff: 0, yoff: 0 },
        dir: SplitDir::TopBottom,
        children: vec![
            Cell::Leaf {
                dims: CellDims { width: 80, height: 11, xoff: 0, yoff: 0 },
                pane_id: Some(1),
            },
            Cell::Leaf {
                dims: CellDims { width: 80, height: 10, xoff: 0, yoff: 12 },
                pane_id: Some(2),
            },
        ],
    };
    // cell_h = 16.0, pane_title_h = 16.0, gap = 1.0
    // pane1: dims.height*cell_h + pane_title_h = 11*16 + 16 = 192 px tall
    // pane2: available.y - consumed = (22*16) - 192 - 1 = 352-192-1 = 159 available
    //        max(159, 10*16+16) = max(159, 176) = 176 px tall
    // pane2 starts at y = 192+1 = 193
    let (rects, _, _) = collapse(&root, 8.0, 16.0, 1.0, 16.0);
    let r1 = rects[&PaneId(1)];
    let r2 = rects[&PaneId(2)];
    assert_eq!(r1, Rect::from_corners(Vec2::ZERO, Vec2::new(640.0, 192.0)));
    assert_eq!(r2, Rect::from_corners(Vec2::new(0.0, 193.0), Vec2::new(640.0, 369.0)));
    assert_eq!(r1.height(), 192.0, "11 rows + 1 title bar row = 192px");
    assert_eq!(r2.height(), 176.0, "10 rows + 1 title bar row = 176px");
}
```

- [ ] **Step 5: Run tests**

```
cargo test -p ozmux-gui collapse
```
Expected: 6 tests pass (5 existing + 1 new).

- [ ] **Step 6: Commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux_render): add pane_title_h param to collapse() and place()"
```

---

### Task 3: `PaneTitleBar` + `TerminalRenderRef` components; stub plugin

**Files:**
- Create: `src/ui/tmux_pane_title.rs`
- Modify: `src/ui.rs`
- Modify: `src/tmux_render.rs` (add `TerminalRenderRef`)

**Interfaces:**
- Produces:
  - `PaneTitleBar` (marker component) in `src/ui/tmux_pane_title.rs`
  - `TerminalRenderRef(pub Entity)` in `src/tmux_render.rs`
  - Empty `OzmuxTmuxPaneTitlePlugin` stub in `src/ui/tmux_pane_title.rs`

- [ ] **Step 1: Create `src/ui/tmux_pane_title.rs` with the component and a stub plugin**

```rust
//! Per-pane title bar: `PaneTitleBar` marker and the plugin that keeps it in sync.

use bevy::prelude::*;

/// Marker on the title-bar child entity that sits at the top of each `TmuxPane`
/// container.
#[derive(Component)]
pub(crate) struct PaneTitleBar;

/// Stub plugin. Systems are added in Task 8.
pub(crate) struct OzmuxTmuxPaneTitlePlugin;

impl Plugin for OzmuxTmuxPaneTitlePlugin {
    fn build(&self, _app: &mut App) {}
}
```

- [ ] **Step 2: Add module declaration to `src/ui.rs`** (after line 18, with the other `pub(crate) mod` lines)

```rust
pub(crate) mod tmux_pane_title;
```

- [ ] **Step 3: Add `TerminalRenderRef` component to `src/tmux_render.rs`** (after the `PackedTmuxLayout` struct, around line 79)

```rust
/// Links a `TmuxPane` container entity to its `TerminalRenderChild` (the entity
/// that owns `TerminalGrid` and `MaterialNode<TerminalUiMaterial>`). Inserted by
/// `attach_tmux_pane_terminal` alongside `TerminalHandle`.
///
/// Required because `flush_emit` / `emit_pending` must target the entity that
/// carries `TerminalGrid` (where `apply_snapshot` / `apply_delta` observers look),
/// and that entity is the child, not the `TmuxPane` container.
#[derive(Component)]
pub(crate) struct TerminalRenderRef(pub Entity);
```

- [ ] **Step 4: Compile check**

```
cargo check
```
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add src/ui/tmux_pane_title.rs src/ui.rs src/tmux_render.rs
git commit -m "feat(tmux_render): add PaneTitleBar and TerminalRenderRef components"
```

---

### Task 4: Refactor `attach_tmux_pane_terminal` and fix `route_tmux_output`

**Files:**
- Modify: `src/tmux_render.rs`

**Interfaces:**
- Consumes: `PaneTitleBar` from `crate::ui::tmux_pane_title`, `TerminalRenderRef` (Task 3)
- Produces: Every `TmuxPane` entity with `TerminalHandle` also has:
  - `Node { position_type: Absolute, flex_direction: Column, .. }` (container)
  - `TerminalRenderRef(render_child_entity)`
  - Children: `PaneTitleBar` (with `Text` grandchild) + `TerminalRenderChild` (with `TerminalRenderBundle`)

- [ ] **Step 1: Add import for `PaneTitleBar` to `src/tmux_render.rs`** (in the `use crate::...` block at the top)

```rust
use crate::ui::tmux_pane_title::PaneTitleBar;
```

- [ ] **Step 2: Replace `attach_tmux_pane_terminal` body (lines ~108–136)**

```rust
fn attach_tmux_pane_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    gate: Option<Res<OscWebviewGate>>,
    panes: Query<(Entity, &TmuxPane), Without<TerminalHandle>>,
) {
    // NOTE: clone the SHARED OscWebviewGate so a tmux pane captures OSC 5379 when
    // the feature is enabled; a fresh `false` atomic would leave inline-webview
    // capture permanently off for tmux panes. The fallback is only reached in
    // tests that do not install the gate resource.
    let gate = gate
        .map(|g| g.0.clone())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    for (entity, pane) in panes.iter() {
        let (cols, rows) = grid_dims(pane.dims.width, pane.dims.height);
        let handle = TerminalHandle::detached(cols, rows, gate.clone());
        let material = materials.add(TerminalUiMaterial::default());

        commands.entity(entity).insert((
            handle,
            TerminalTitle::default(),
            Node {
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                ..default()
            },
            Outline::new(Val::Px(theme::PANE_BORDER_PX), Val::Px(0.0), theme::BORDER),
        ));

        let title_bar = commands
            .spawn((
                PaneTitleBar,
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::axes(
                        Val::Px(theme::TAB_PADDING_X_PX),
                        Val::Px(0.0),
                    ),
                    align_items: AlignItems::Center,
                    overflow: Overflow::clip_x(),
                    ..default()
                },
                BackgroundColor(theme::PANEL),
                ChildOf(entity),
            ))
            .id();
        commands.spawn((
            Text::new(""),
            TextColor(theme::FOREGROUND),
            TextFont {
                font_size: theme::UI_FONT_SIZE,
                ..default()
            },
            ChildOf(title_bar),
        ));

        let render_child = commands
            .spawn((
                TerminalRenderBundle::new(material),
                Node {
                    flex_grow: 1.0,
                    width: Val::Percent(100.0),
                    ..default()
                },
                ChildOf(entity),
            ))
            .id();

        commands
            .entity(entity)
            .insert(TerminalRenderRef(render_child));
    }
}
```

- [ ] **Step 3: Fix `route_tmux_output` to target `TerminalRenderChild` for `flush_emit`**

Change the query signature of `route_tmux_output`:
```rust
// Old:
mut handles: Query<(&mut TerminalHandle, &mut TerminalTitle)>,

// New:
mut handles: Query<(&mut TerminalHandle, &mut TerminalTitle, &TerminalRenderRef)>,
```

In the inner loop, change the destructuring and `flush_emit` call:
```rust
// Old:
let Ok((mut handle, mut title)) = handles.get_mut(entity) else {
    continue;
};
// ...
if copy_modes.get(entity).is_err() {
    handle.flush_emit(&mut commands, entity);
}

// New:
let Ok((mut handle, mut title, render_ref)) = handles.get_mut(entity) else {
    continue;
};
// ...
if copy_modes.get(entity).is_err() {
    handle.flush_emit(&mut commands, render_ref.0);
}
```

`drain_control_events` still uses `entity` (the TmuxPane entity) — no change there; it triggers `TerminalBell`/`TerminalTitleChanged`/`OscWebviewRequest` which are not `TerminalGrid` observers.

- [ ] **Step 4: Update `output_routed_into_pane_grid_renders_text` test**

The test checks `TerminalGrid` on `pane_entity` — it now lives on `render_ref.0`. Update the grid read:

```rust
// Old:
let grid = app
    .world()
    .get::<TerminalGrid>(pane_entity)
    .expect("pane has a TerminalGrid");

// New:
let render_ref = app
    .world()
    .get::<TerminalRenderRef>(pane_entity)
    .expect("pane has TerminalRenderRef");
let grid = app
    .world()
    .get::<TerminalGrid>(render_ref.0)
    .expect("render child has TerminalGrid");
```

Apply the same replacement at every `get::<TerminalGrid>(pane_entity)` occurrence in this test.

- [ ] **Step 5: Update `copy_mode_pane_advances_but_gates_the_emit` test**

Same pattern as Step 4. Replace every:
```rust
app.world().get::<TerminalGrid>(pane_entity).expect("pane has a TerminalGrid")
// or:
app.world().get::<TerminalGrid>(pane_entity).unwrap()
```
with:
```rust
{
    let rr = app.world().get::<TerminalRenderRef>(pane_entity).unwrap();
    app.world().get::<TerminalGrid>(rr.0).unwrap()
}
```

The inline block is needed each time to reborrow `app.world()` freshly.

- [ ] **Step 6: Run tests**

```
cargo test -p ozmux-gui output_routed_into_pane_grid_renders_text
cargo test -p ozmux-gui copy_mode_pane_advances_but_gates_the_emit
cargo test -p ozmux-gui mount_inline_osc_from_pane_triggers_webview_request
```
Expected: all 3 pass.

- [ ] **Step 7: Commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux_render): split pane entity — PaneTitleBar + TerminalRenderChild children"
```

---

### Task 5: Update `layout_tmux_panes` — use `TerminalRenderRef`, pass real `pane_title_h`

**Files:**
- Modify: `src/tmux_render.rs`

**Interfaces:**
- Consumes: `TerminalRenderRef` (Task 3), `collapse()` with `pane_title_h` (Task 2)
- Produces: TmuxPane container Node sized to full rect (terminal rows × cell_h + title_h); TerminalRenderChild grid resized to terminal rows.

- [ ] **Step 1: Update `layout_tmux_panes` queries and body**

Replace the function (lines ~359–434):

```rust
fn layout_tmux_panes(
    mut commands: Commands,
    mut windows: Query<
        (
            Entity,
            &TmuxWindowLayout,
            &mut Node,
            &Children,
            Option<&PackedTmuxLayout>,
        ),
        With<TmuxWindow>,
    >,
    mut panes: Query<
        (&TmuxPane, &mut Node, &mut TerminalHandle, &TerminalRenderRef),
        Without<TmuxWindow>,
    >,
    mut grids: Query<&mut TerminalGrid>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let dpr = window.scale_factor().max(0.5);
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0) / dpr;
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0) / dpr;
    let pane_title_h = cell_h;

    for (window_entity, layout, mut container, children, maybe_packed) in windows.iter_mut() {
        let (rects, dividers, bbox) =
            collapse(&layout.0.root, cell_w, cell_h, theme::PANE_GAP_PX, pane_title_h);

        // NOTE: only write the Node fields when they actually change — writing
        // through `Mut<Node>` every frame marks the component changed and forces
        // a full UI relayout pass each tick even when nothing moved.
        if container.width != Val::Px(bbox.x) || container.height != Val::Px(bbox.y) {
            container.width = Val::Px(bbox.x);
            container.height = Val::Px(bbox.y);
        }

        let packed_changed = maybe_packed
            .is_none_or(|p| p.panes != rects || p.dividers != dividers || p.bbox != bbox);
        if packed_changed {
            commands.entity(window_entity).insert(PackedTmuxLayout {
                panes: rects.clone(),
                dividers: dividers.clone(),
                bbox,
            });
        }

        for child in children.iter() {
            let Ok((pane, mut node, mut handle, render_ref)) = panes.get_mut(child) else {
                continue;
            };
            let Some(rect) = rects.get(&pane.id) else {
                continue;
            };
            let (left, top, width, height) =
                (rect.min.x, rect.min.y, rect.width(), rect.height());
            if node.left != Val::Px(left)
                || node.top != Val::Px(top)
                || node.width != Val::Px(width)
                || node.height != Val::Px(height)
            {
                node.left = Val::Px(left);
                node.top = Val::Px(top);
                node.width = Val::Px(width);
                node.height = Val::Px(height);
            }
            let (cols, rows) = grid_dims(pane.dims.width, pane.dims.height);
            let (cur_cols, cur_rows, _) = handle.read_geometry();
            if (cur_cols, cur_rows) != (cols, rows) {
                handle.resize_grid_only(cols, rows);
                if let Ok(mut grid) = grids.get_mut(render_ref.0) {
                    grid.cols = cols;
                    grid.rows = rows;
                }
                handle.emit_pending(&mut commands, render_ref.0);
            }
        }
    }
}
```

- [ ] **Step 2: Update `resize_only_updates_grid_dims_and_emits` test**

The test manually spawns `TerminalGrid` on the pane entity. After this task, `layout_tmux_panes` queries `TerminalRenderRef` on the pane and looks up `TerminalGrid` via `grids.get_mut(render_ref.0)`. The test must spawn a `TerminalRenderChild` entity and use `TerminalRenderRef`.

Replace the pane spawn:
```rust
// Old:
let entity = app
    .world_mut()
    .spawn((
        TmuxPane { id: PaneId(1), dims: CellDims { width: 40, height: 10, xoff: 0, yoff: 0 } },
        Node::default(),
        TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
        TerminalGrid::default(),
        ChildOf(window_e),
    ))
    .id();

// New:
let render_child = app.world_mut().spawn(TerminalGrid::default()).id();
let entity = app
    .world_mut()
    .spawn((
        TmuxPane { id: PaneId(1), dims: CellDims { width: 40, height: 10, xoff: 0, yoff: 0 } },
        Node::default(),
        TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
        TerminalRenderRef(render_child),
        ChildOf(window_e),
    ))
    .id();
```

Replace the grid read:
```rust
// Old:
let grid = app.world().get::<TerminalGrid>(entity).expect("pane has a TerminalGrid");
assert_eq!((grid.cols, grid.rows), (40, 10), ...);

// New:
let grid = app.world().get::<TerminalGrid>(render_child).expect("render child has TerminalGrid");
assert_eq!((grid.cols, grid.rows), (40, 10), ...);
```

- [ ] **Step 3: Update `resize_fires_fresh_snapshot_after_first_emit` test**

The test manually spawns `TerminalGrid` on the pane entity AND has an inline system that calls `flush_emit` with the pane entity. Both must be updated.

Replace the pane spawn (same pattern as Step 2):
```rust
// Old:
let entity = app
    .world_mut()
    .spawn((
        TmuxPane { id: PaneId(2), dims: CellDims { width: 20, height: 5, xoff: 0, yoff: 0 } },
        Node::default(),
        TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
        TerminalGrid::default(),
        ChildOf(window_e),
    ))
    .id();

// New:
let render_child = app.world_mut().spawn(TerminalGrid::default()).id();
let entity = app
    .world_mut()
    .spawn((
        TmuxPane { id: PaneId(2), dims: CellDims { width: 20, height: 5, xoff: 0, yoff: 0 } },
        Node::default(),
        TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
        TerminalRenderRef(render_child),
        ChildOf(window_e),
    ))
    .id();
```

Replace the inline flush system:
```rust
// Old:
|mut commands: Commands, mut q: Query<(Entity, &mut TerminalHandle)>| {
    for (e, mut h) in q.iter_mut() {
        h.advance(b"x");
        h.flush_emit(&mut commands, e);
    }
},

// New:
|mut commands: Commands, mut q: Query<(Entity, &mut TerminalHandle, &TerminalRenderRef)>| {
    for (_, mut h, render_ref) in q.iter_mut() {
        h.advance(b"x");
        h.flush_emit(&mut commands, render_ref.0);
    }
},
```

Replace the grid assertion at the end:
```rust
// Old:
let grid = app.world().get::<TerminalGrid>(entity).expect("pane has a TerminalGrid");

// New:
let grid = app.world().get::<TerminalGrid>(render_child).expect("render child has TerminalGrid");
```

- [ ] **Step 4: Run tests**

```
cargo test -p ozmux-gui resize_only_updates_grid_dims_and_emits
cargo test -p ozmux-gui resize_fires_fresh_snapshot_after_first_emit
cargo test -p ozmux-gui layout
```
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux_render): layout_tmux_panes uses TerminalRenderRef and real pane_title_h"
```

---

### Task 6: `sync_client_size` — subtract `vertical_depth` rows

**Files:**
- Modify: `src/tmux_render.rs`

**Interfaces:**
- Consumes: `vertical_depth` (Task 1), `TmuxWindowLayout`, `ActiveWindow` (already in scope)
- Produces: rows sent to tmux = `rows_for_panes(total) - vertical_depth(active_layout.root)`, reserving one row per stacked title bar level.

- [ ] **Step 1: Update `sync_client_size` signature and depth subtraction**

```rust
fn sync_client_size(
    mut last: ResMut<LastClientSize>,
    connection: NonSend<TmuxConnection>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
    active_layout: Query<&TmuxWindowLayout, With<ActiveWindow>>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    let Ok(window) = window.single() else {
        return;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let (cols, rows) = cells_for(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        cell_w,
        cell_h,
    );
    let rows = rows_for_panes(rows);
    let depth = active_layout
        .single()
        .map(|l| vertical_depth(&l.0.root))
        .unwrap_or(1) as u16;
    let rows = rows.saturating_sub(depth).max(1);
    if (cols, rows) == (last.cols, last.rows) {
        return;
    }
    // NOTE: only record the size as sent AFTER a successful send — otherwise a
    // transient send failure would poison the dedupe and permanently suppress
    // re-sending this size, leaving tmux stuck at the stale client dimensions.
    match client.handle().send(&refresh_client_command(cols, rows)) {
        Ok(_) => {
            last.cols = cols;
            last.rows = rows;
        }
        Err(e) => tracing::warn!(?e, cols, rows, "refresh-client send failed"),
    }
}
```

Note: `active_layout: Query<&TmuxWindowLayout, With<ActiveWindow>>` is an immutable param, so it goes after all the mutable params (`mut last`, `connection` is NonSend which moves, `metrics`, `window` are Res). The param ordering rule applies: `mut last` (mutable) first, then the immutable reads. `connection` is `NonSend` (moves) — treat as mutable for ordering purposes: keep it early. The order above satisfies the mutable-first rule.

- [ ] **Step 2: Update the `rows_for_panes` test to document the new behavior**

The existing test is still correct (it tests `rows_for_panes` in isolation, which is unchanged). Add a comment to the test documenting that `sync_client_size` now additionally subtracts `vertical_depth`:

```rust
#[test]
fn rows_for_panes_reserves_one_row_for_the_bar() {
    // rows_for_panes reserves 1 row for the window bar.
    // sync_client_size additionally subtracts vertical_depth for title bars.
    assert_eq!(rows_for_panes(24), 23);
    assert_eq!(rows_for_panes(1), 1); // never zero
    assert_eq!(rows_for_panes(2), 1);
}
```

- [ ] **Step 3: Run tests**

```
cargo test -p ozmux-gui rows_for_panes
cargo check
```
Expected: test passes, no compile errors.

- [ ] **Step 4: Commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux_render): subtract vertical_depth rows in sync_client_size for title bars"
```

---

### Task 7: Migrate `PaneDim` to `TerminalRenderChild` in `tmux_pane_focus.rs`

**Files:**
- Modify: `src/ui/tmux_pane_focus.rs`

**Interfaces:**
- Consumes: `TerminalRenderRef` from `crate::tmux_render` (Task 3)
- Produces: `PaneDim` inserted on `render_ref.0` (the `TerminalRenderChild` entity), not on `TmuxPane`. `update_terminal_material` in `ozma_tty_renderer` queries `PaneDim` alongside `MaterialNode` — they must be on the same entity.

- [ ] **Step 1: Update imports in `src/ui/tmux_pane_focus.rs`**

Add import:
```rust
use crate::tmux_render::TerminalRenderRef;
```

- [ ] **Step 2: Update `sync_pane_dim` to target `TerminalRenderChild`**

```rust
fn sync_pane_dim(
    mut commands: Commands,
    panes: Query<(Has<ActivePane>, Option<&TerminalRenderRef>), With<TmuxPane>>,
    render_dims: Query<Option<&PaneDim>>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let dim_factor = inactive_dim_factor(configs.as_deref());
    let any_active = panes.iter().any(|(active, _)| active);
    for (active, maybe_ref) in panes.iter() {
        let Some(render_ref) = maybe_ref else {
            continue;
        };
        let want = if active || !any_active {
            1.0
        } else {
            dim_factor
        };
        let current = render_dims.get(render_ref.0).ok().flatten();
        if current.map(|d| d.0) != Some(want) {
            commands.entity(render_ref.0).insert(PaneDim(want));
        }
    }
}
```

- [ ] **Step 3: Update `sync_sets_pane_dim_from_active_marker` test**

The test spawns `TmuxPane + TerminalHandle + (maybe ActivePane)` entities and then calls `dim(&app, entity)`. After the change, `PaneDim` is on `render_ref.0`. Update the test to spawn render child entities and check dim on them:

```rust
#[test]
fn sync_sets_pane_dim_from_active_marker() {
    use ozmux_tmux::ActivePane;

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
    app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
    app.insert_resource(OzmuxConfigsResource::default());
    let h = || TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false)));

    let rc1 = app.world_mut().spawn(()).id();
    let p1 = app
        .world_mut()
        .spawn((
            TmuxPane { id: PaneId(1), dims: dims() },
            h(),
            ActivePane,
            crate::tmux_render::TerminalRenderRef(rc1),
        ))
        .id();

    let rc2 = app.world_mut().spawn(()).id();
    let p2 = app
        .world_mut()
        .spawn((
            TmuxPane { id: PaneId(2), dims: dims() },
            h(),
            crate::tmux_render::TerminalRenderRef(rc2),
        ))
        .id();

    let dim = |app: &App, e| app.world().get::<PaneDim>(e).map(|d| d.0);

    app.update();
    // PaneDim is now on the render children, not on the pane entities.
    assert_eq!(dim(&app, rc1), Some(1.0), "active pane render child full-bright");
    assert_eq!(dim(&app, rc2), Some(0.5), "inactive pane render child dimmed");
    // p1/p2 themselves no longer carry PaneDim.
    assert_eq!(dim(&app, p1), None);
    assert_eq!(dim(&app, p2), None);

    // Move ActivePane to p2.
    app.world_mut().entity_mut(p1).remove::<ActivePane>();
    app.world_mut().entity_mut(p2).insert(ActivePane);
    app.update();
    assert_eq!(dim(&app, rc1), Some(0.5));
    assert_eq!(dim(&app, rc2), Some(1.0));

    // No active pane: both full-bright.
    app.world_mut().entity_mut(p2).remove::<ActivePane>();
    app.update();
    assert_eq!(dim(&app, rc1), Some(1.0));
    assert_eq!(dim(&app, rc2), Some(1.0));
}
```

Note: the test file now needs `use crate::tmux_render::TerminalRenderRef;` inside the test module or as a full path. Since tests are in `#[cfg(test)] mod tests { use super::*; }`, `TerminalRenderRef` is accessible via the `use crate::tmux_render::TerminalRenderRef` import added in Step 1.

- [ ] **Step 4: Run tests**

```
cargo test -p ozmux-gui sync_sets_pane_dim
cargo test -p ozmux-gui augment_adds_button
```
Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git add src/ui/tmux_pane_focus.rs
git commit -m "fix(tmux_pane_focus): insert PaneDim on TerminalRenderChild entity via TerminalRenderRef"
```

---

### Task 8: Full `OzmuxTmuxPaneTitlePlugin` — title sync, active styling, focus, registration

**Files:**
- Modify: `src/ui/tmux_pane_title.rs`
- Modify: `src/ui/tmux_pane_focus.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `PaneTitleBar` component (Task 3), `TerminalTitle` from `ozma_tty_engine`, `ActivePane` from `ozmux_tmux`
- Produces: Title bar text tracks `TerminalTitle`; active pane title bar has `TAB_BAR_BG` background + `ACCENT` outline; inactive has `PANEL` background; `PaneTitleBar` entity has `FocusPolicy::Block`.

- [ ] **Step 1: Replace stub plugin with full implementation in `src/ui/tmux_pane_title.rs`**

```rust
//! Per-pane title bar: `PaneTitleBar` marker and the plugin that keeps it in sync.

use crate::theme;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use ozma_tty_engine::TerminalTitle;
use ozmux_tmux::{ActivePane, TmuxPane};

/// Marker on the title-bar child entity that sits at the top of each `TmuxPane`
/// container.
#[derive(Component)]
pub(crate) struct PaneTitleBar;

/// Keeps each pane's title bar text and color in sync with `TerminalTitle` and
/// `ActivePane` state.
pub(crate) struct OzmuxTmuxPaneTitlePlugin;

impl Plugin for OzmuxTmuxPaneTitlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (sync_pane_title_text, sync_pane_title_active),
        );
    }
}

/// Updates the `Text` grandchild of each `PaneTitleBar` when `TerminalTitle` changes.
fn sync_pane_title_text(
    changed: Query<(&TerminalTitle, &Children), (With<TmuxPane>, Changed<TerminalTitle>)>,
    bars: Query<&Children, With<PaneTitleBar>>,
    mut texts: Query<&mut Text>,
) {
    for (title, pane_children) in changed.iter() {
        let Some(&bar) = pane_children.iter().find(|&&c| bars.contains(c)) else {
            continue;
        };
        let Ok(bar_children) = bars.get(bar) else {
            continue;
        };
        for &text_entity in bar_children.iter() {
            if let Ok(mut text) = texts.get_mut(text_entity) {
                let s = title.0.as_deref().unwrap_or("");
                *text = Text::new(s);
            }
        }
    }
}

/// Recolors each pane's title bar: `TAB_BAR_BG` + accent outline for the active
/// pane, `PANEL` + transparent outline otherwise. Write-guarded to avoid
/// triggering a UI relayout on every frame.
fn sync_pane_title_active(
    panes: Query<(Has<ActivePane>, &Children), With<TmuxPane>>,
    mut bars: Query<(&mut BackgroundColor, &mut Outline), With<PaneTitleBar>>,
) {
    for (active, children) in panes.iter() {
        for &child in children.iter() {
            let Ok((mut bg, mut outline)) = bars.get_mut(child) else {
                continue;
            };
            let want_bg = if active {
                BackgroundColor(theme::TAB_BAR_BG)
            } else {
                BackgroundColor(theme::PANEL)
            };
            let want_outline_color = if active { theme::ACCENT } else { Color::NONE };
            if *bg != want_bg {
                *bg = want_bg;
            }
            if outline.color != want_outline_color {
                outline.color = want_outline_color;
            }
        }
    }
}
```

- [ ] **Step 2: Add `Outline` to the `PaneTitleBar` spawn in `attach_tmux_pane_terminal` (`src/tmux_render.rs`)**

In the `PaneTitleBar` spawn inside `attach_tmux_pane_terminal`, add `Outline` so `sync_pane_title_active` can mutate it:

```rust
let title_bar = commands
    .spawn((
        PaneTitleBar,
        Node {
            width: Val::Percent(100.0),
            padding: UiRect::axes(Val::Px(theme::TAB_PADDING_X_PX), Val::Px(0.0)),
            align_items: AlignItems::Center,
            overflow: Overflow::clip_x(),
            ..default()
        },
        BackgroundColor(theme::PANEL),
        Outline::new(Val::Px(2.0), Val::Px(0.0), Color::NONE),
        ChildOf(entity),
    ))
    .id();
```

- [ ] **Step 3: Update `augment_tmux_pane` in `src/ui/tmux_pane_focus.rs` to also block focus on `PaneTitleBar`**

Add import to the existing `use` block:
```rust
use crate::ui::tmux_pane_title::PaneTitleBar;
```

Add a second query to `augment_tmux_pane`:
```rust
fn augment_tmux_pane(
    mut commands: Commands,
    panes: Query<Entity, (With<TmuxPane>, With<TerminalHandle>, Without<Button>)>,
    title_bars: Query<Entity, (With<PaneTitleBar>, Without<FocusPolicy>)>,
) {
    for pane in panes.iter() {
        commands.entity(pane).insert((Button, FocusPolicy::Block));
    }
    for bar in title_bars.iter() {
        commands.entity(bar).insert(FocusPolicy::Block);
    }
}
```

- [ ] **Step 4: Register `OzmuxTmuxPaneTitlePlugin` in `src/main.rs`**

Add import near the other `ui::` imports:
```rust
use ui::tmux_pane_title::OzmuxTmuxPaneTitlePlugin;
```

Add plugin registration after `OzmuxTmuxPaneFocusPlugin`:
```rust
.add_plugins(OzmuxTmuxPaneFocusPlugin)
.add_plugins(OzmuxTmuxPaneTitlePlugin)
```

- [ ] **Step 5: Run all tests**

```
cargo test -p ozmux-gui
```
Expected: all tests pass.

- [ ] **Step 6: Compile check**

```
cargo check
```
Expected: no errors or warnings (other than pre-existing ones).

- [ ] **Step 7: Commit**

```bash
git add src/ui/tmux_pane_title.rs src/ui/tmux_pane_focus.rs src/tmux_render.rs src/main.rs
git commit -m "feat(ui): OzmuxTmuxPaneTitlePlugin — title text sync and active pane accent"
```

---

## Self-Review Checklist

- **Spec coverage:**
  - `vertical_depth` function → Task 1 ✓
  - `collapse()` pane_title_h → Task 2 ✓
  - `sync_client_size` depth subtraction → Task 6 ✓
  - `attach_tmux_pane_terminal` Column-flex + children → Task 4 ✓
  - `layout_tmux_panes` TerminalRenderRef query + sizing → Task 5 ✓
  - `route_tmux_output` TerminalRenderRef → Task 4 ✓
  - `PaneDim` migration → Task 7 ✓
  - `OzmuxTmuxPaneTitlePlugin` → Task 8 ✓
  - `augment_tmux_pane` FocusPolicy → Task 8 ✓
  - Plugin registration → Task 8 ✓

- **Type consistency:** `TerminalRenderRef(pub Entity)` named consistently across Tasks 3, 4, 5, 7. `PaneTitleBar` named consistently across Tasks 3, 4, 8. `vertical_depth(&Cell) -> u16` used as `as u16` in Task 6.

- **Placeholder scan:** None — all steps contain actual code.

- **Sequencing:** Each task compiles independently after its steps. Task 8 requires Tasks 3 + 4 (PaneTitleBar entity must exist to be queried).
