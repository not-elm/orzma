# tmux Pane Border & Gap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Draw a 1px divider between tmux panes and an accent border around the active pane, while collapsing tmux's reserved 1-cell inter-pane gap down to a 1px line.

**Architecture:** Retain the tmux `WindowLayout` tree on each `TmuxWindow` entity (threaded through the `TmuxLayoutChanged` event). A pure `collapse` function walks the tree and produces packed pixel rects per pane (each reserved separator → 1px), plus the packed bounding box. `layout_tmux_panes` sizes a grey window container to that bbox so the 1px gaps between opaque pane nodes bleed grey as divider lines; the active pane gets a Bevy `Outline` recolored to the accent.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS + UI (`Node`, `BackgroundColor`, `Outline`), `tmux_control_parser` layout tree, `ozmux_tmux` (crate `ozmux_tmux` at `crates/tmux_session`).

**Spec:** `docs/superpowers/specs/2026-06-16-tmux-pane-border-design.md`

---

## File Structure

- `crates/tmux_session/src/components.rs` — add `TmuxWindowLayout(pub WindowLayout)` component.
- `crates/tmux_session/src/events.rs` — `TmuxLayoutChanged` carries `layout: WindowLayout` (replaces `panes: Vec<PaneGeom>`).
- `crates/tmux_session/src/event_pump.rs` — two construction sites pass the tree.
- `crates/tmux_session/src/observers.rs` — `on_layout_changed` inserts `TmuxWindowLayout` and derives panes via `pane_geoms(&ev.layout)`.
- `crates/tmux_session/src/plugin.rs` — one test construction site updated.
- `crates/tmux_session/src/lib.rs` — export `TmuxWindowLayout`.
- `src/theme.rs` — add `PANE_GAP_PX`.
- `src/tmux_render.rs` — add pure `collapse`; rewrite `layout_tmux_panes` to be tree-driven (container bbox + pane rects); grey container background; add `Outline` to panes + `sync_active_pane_outline`; register it; remove the old flat `pane_rect`.

Note: every `TmuxLayoutChanged { ... }` construction site (production + tests) must switch from `panes: pane_geoms(...)` to `layout: <WindowLayout>`. Sites: `event_pump.rs:296,326`; `observers.rs:302,329,349,434`; `plugin.rs:253`.

---

## Task 1: Thread the layout tree into the session projection

**Files:**
- Modify: `crates/tmux_session/src/components.rs`
- Modify: `crates/tmux_session/src/events.rs:49-54`
- Modify: `crates/tmux_session/src/event_pump.rs:7,295-299,324-329`
- Modify: `crates/tmux_session/src/observers.rs:4-9,111-137,302,329,349,434`
- Modify: `crates/tmux_session/src/plugin.rs:235-236,251-254`
- Modify: `crates/tmux_session/src/lib.rs:22`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/tmux_session/src/observers.rs` (after `window_added_then_layout_spawns_window_and_panes`):

```rust
    #[test]
    fn layout_change_attaches_window_layout_component() {
        use crate::components::TmuxWindowLayout;
        let mut app = app();
        app.world_mut().trigger(TmuxWindowAdded {
            window: WindowId(1),
            index: 0,
            name: "w".into(),
        });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            layout: layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        let window_e = index.windows[&WindowId(1)];
        assert!(
            app.world().get::<TmuxWindowLayout>(window_e).is_some(),
            "window carries its layout tree after %layout-change",
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux_tmux layout_change_attaches_window_layout_component`
Expected: FAIL to compile — `TmuxLayoutChanged` has no `layout` field and `TmuxWindowLayout` does not exist.

- [ ] **Step 3: Add the `TmuxWindowLayout` component**

In `crates/tmux_session/src/components.rs`, change the import line:

```rust
use tmux_control_parser::{CellDims, PaneId, SessionId, WindowId, WindowLayout};
```

and append at the end of the file:

```rust
/// The window's full tmux layout tree, retained so the renderer can collapse
/// tmux's reserved inter-pane separator cells into 1px dividers.
#[derive(Component, Debug, Clone)]
pub struct TmuxWindowLayout(pub WindowLayout);
```

- [ ] **Step 4: Carry the tree on the event**

In `crates/tmux_session/src/events.rs:49-54`, replace the struct:

```rust
/// `%layout-change` or a seed row: the window's full layout tree.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxLayoutChanged {
    pub(crate) window: WindowId,
    pub(crate) layout: WindowLayout,
}
```

(Keep `PaneGeom`, `pane_geoms`, and `collect_leaves` unchanged — the observer still uses `pane_geoms`.)

- [ ] **Step 5: Update the two production construction sites**

In `crates/tmux_session/src/event_pump.rs`, line 7 remove the now-unused `pane_geoms` from the import:

```rust
    TmuxWindowAdded, TmuxWindowClosed, TmuxWindowRenamed, TmuxWindowsRetained,
```

Replace the `LayoutChange` arm (around line 295):

```rust
        ControlEvent::LayoutChange { window, layout, .. } => {
            commands.trigger(TmuxLayoutChanged {
                window: *window,
                layout: layout.clone(),
            });
        }
```

Replace the seed-row trigger (around line 326):

```rust
        commands.trigger(TmuxLayoutChanged {
            window: row.id,
            layout: row.layout.clone(),
        });
```

- [ ] **Step 6: Insert the component + derive panes in the observer**

In `crates/tmux_session/src/observers.rs`, add `TmuxWindowLayout` to the components import (line 4) and `pane_geoms` to the events import (line 5-9):

```rust
use crate::components::{ActivePane, ActiveWindow, TmuxPane, TmuxSession, TmuxWindow, TmuxWindowLayout};
use crate::events::{
    PaneGeom, TmuxActivePaneChanged, TmuxActiveWindowChanged, TmuxConnectionReset,
    TmuxLayoutChanged, TmuxSessionChanged, TmuxWindowAdded, TmuxWindowClosed, TmuxWindowRenamed,
    TmuxWindowsRetained, pane_geoms,
};
```

Replace the body of `on_layout_changed` (lines 111-137) so it inserts the component and derives panes from the tree:

```rust
fn on_layout_changed(
    ev: On<TmuxLayoutChanged>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
    active_panes: Query<Entity, With<ActivePane>>,
) {
    let window = ensure_window(&mut commands, &mut index, ev.window);
    commands
        .entity(window)
        .insert(TmuxWindowLayout(ev.layout.clone()));

    let panes = pane_geoms(&ev.layout);
    let live: HashSet<PaneId> = panes.iter().map(|p| p.id).collect();
    let stale: Vec<PaneId> = index
        .panes
        .iter()
        .filter(|(id, (_, w))| *w == ev.window && !live.contains(id))
        .map(|(id, _)| *id)
        .collect();
    for id in stale {
        if let Some((e, _)) = index.panes.remove(&id) {
            commands.entity(e).despawn();
        }
    }

    for geom in &panes {
        upsert_pane(&mut commands, &mut index, window, ev.window, geom);
    }

    apply_pending_active_pane(&mut commands, &mut index, &active_panes);
}
```

- [ ] **Step 7: Update the remaining test construction sites**

In `crates/tmux_session/src/observers.rs`, the test module imports `use crate::events::pane_geoms;` (around line 280) is now unused — remove that line. Then update each test trigger to pass `layout:` instead of `panes: pane_geoms(...)`:

- line ~302: `layout: layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),`
- line ~329: `layout: layout(b"abcd,80x24,0,0,5"),`
- line ~349: `layout: layout(b"abcd,80x24,0,0,9"),`
- line ~434: `layout: layout(b"abcd,80x24,0,0,1"),`

In `crates/tmux_session/src/plugin.rs`, the test import (line 235-236) drops `pane_geoms` (keep `WindowLayout`):

```rust
        use crate::events::{
            TmuxLayoutChanged, TmuxSessionChanged, TmuxWindowAdded, TmuxWindowsRetained,
        };
```

and the trigger (line 251-254) becomes:

```rust
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            layout: WindowLayout::parse(b"abcd,80x24,0,0,1").unwrap(),
        });
```

- [ ] **Step 8: Export the component**

In `crates/tmux_session/src/lib.rs:22`:

```rust
pub use components::{ActivePane, ActiveWindow, TmuxPane, TmuxSession, TmuxWindow, TmuxWindowLayout};
```

- [ ] **Step 9: Run the test to verify it passes**

Run: `cargo test -p ozmux_tmux`
Expected: PASS — all `ozmux_tmux` tests including `layout_change_attaches_window_layout_component`.

- [ ] **Step 10: Commit**

```bash
git add crates/tmux_session/src
git commit -m "feat(tmux): retain WindowLayout tree on TmuxWindow entity"
```

---

## Task 2: Pure `collapse` function (packed pane rects)

**Files:**
- Modify: `src/theme.rs`
- Modify: `src/tmux_render.rs` (add `collapse`/`place` + tests)

- [ ] **Step 1: Add the gap constant**

In `src/theme.rs`, after `PANE_BORDER_PX` (line 41):

```rust
/// Gap in logical px between packed panes. The grey window container bleeds
/// through this gap as the 1px divider line.
pub const PANE_GAP_PX: f32 = 1.0;
```

- [ ] **Step 2: Write the failing tests**

In `src/tmux_render.rs`, add these imports to the `#[cfg(test)] mod tests` block's `use` lines (alongside the existing `use tmux_control_parser::{CellDims, PaneId};`):

```rust
    use bevy::math::{Rect, Vec2};
    use tmux_control_parser::{Cell, SplitDir};
```

Then add the tests inside the same test module:

```rust
    #[test]
    fn collapse_single_pane_fills_with_no_gap() {
        let root = Cell::Leaf {
            dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
            pane_id: Some(0),
        };
        let (rects, size) = collapse(&root, 8.0, 16.0, 1.0);
        assert_eq!(
            rects[&PaneId(0)],
            Rect::from_corners(Vec2::ZERO, Vec2::new(640.0, 384.0)),
        );
        assert_eq!(size, Vec2::new(640.0, 384.0));
    }

    #[test]
    fn collapse_horizontal_split_is_one_px_gap() {
        let root = Cell::Split {
            dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
            dir: SplitDir::LeftRight,
            children: vec![
                Cell::Leaf {
                    dims: CellDims { width: 40, height: 24, xoff: 0, yoff: 0 },
                    pane_id: Some(1),
                },
                Cell::Leaf {
                    dims: CellDims { width: 39, height: 24, xoff: 41, yoff: 0 },
                    pane_id: Some(2),
                },
            ],
        };
        let (rects, size) = collapse(&root, 8.0, 16.0, 1.0);
        assert_eq!(
            rects[&PaneId(1)],
            Rect::from_corners(Vec2::ZERO, Vec2::new(320.0, 384.0)),
        );
        assert_eq!(
            rects[&PaneId(2)],
            Rect::from_corners(Vec2::new(321.0, 0.0), Vec2::new(633.0, 384.0)),
        );
        assert_eq!(size, Vec2::new(633.0, 384.0));
    }

    #[test]
    fn collapse_nested_split_advances_by_packed_extent() {
        // LeftRight[ pane1(40x24), TopBottom[ pane2(39x12), pane3(39x11) ] ]
        let root = Cell::Split {
            dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
            dir: SplitDir::LeftRight,
            children: vec![
                Cell::Leaf {
                    dims: CellDims { width: 40, height: 24, xoff: 0, yoff: 0 },
                    pane_id: Some(1),
                },
                Cell::Split {
                    dims: CellDims { width: 39, height: 24, xoff: 41, yoff: 0 },
                    dir: SplitDir::TopBottom,
                    children: vec![
                        Cell::Leaf {
                            dims: CellDims { width: 39, height: 12, xoff: 41, yoff: 0 },
                            pane_id: Some(2),
                        },
                        Cell::Leaf {
                            dims: CellDims { width: 39, height: 11, xoff: 41, yoff: 13 },
                            pane_id: Some(3),
                        },
                    ],
                },
            ],
        };
        let (rects, _size) = collapse(&root, 8.0, 16.0, 1.0);
        assert_eq!(
            rects[&PaneId(1)],
            Rect::from_corners(Vec2::ZERO, Vec2::new(320.0, 384.0)),
        );
        assert_eq!(
            rects[&PaneId(2)],
            Rect::from_corners(Vec2::new(321.0, 0.0), Vec2::new(633.0, 192.0)),
        );
        assert_eq!(
            rects[&PaneId(3)],
            Rect::from_corners(Vec2::new(321.0, 193.0), Vec2::new(633.0, 369.0)),
        );
    }

    #[test]
    fn collapse_skips_leaf_without_pane_id() {
        let root = Cell::Split {
            dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
            dir: SplitDir::LeftRight,
            children: vec![
                Cell::Leaf {
                    dims: CellDims { width: 40, height: 24, xoff: 0, yoff: 0 },
                    pane_id: None,
                },
                Cell::Leaf {
                    dims: CellDims { width: 39, height: 24, xoff: 41, yoff: 0 },
                    pane_id: Some(2),
                },
            ],
        };
        let (rects, _size) = collapse(&root, 8.0, 16.0, 1.0);
        assert_eq!(rects.len(), 1);
        assert!(rects.contains_key(&PaneId(2)));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ozmux-gui collapse_`
Expected: FAIL to compile — `collapse` is not defined.

- [ ] **Step 4: Add the implementation**

In `src/tmux_render.rs`, add these to the top-of-file `use` block:

```rust
use bevy::math::Rect;
use tmux_control_parser::{Cell, SplitDir};
```

(`PaneId` is already available via `ozmux_tmux`/`tmux_control_parser`; if not in scope at module level, add `use tmux_control_parser::PaneId;`.)

Add the functions to the module (place them just above `fn layout_tmux_panes`):

```rust
/// Computes packed pixel rects for every pane in a layout tree, collapsing
/// tmux's reserved 1-cell inter-pane separators to `gap` pixels. Returns the
/// per-pane rects keyed by tmux pane id, plus the root subtree's packed size.
fn collapse(root: &Cell, cell_w: f32, cell_h: f32, gap: f32) -> (HashMap<PaneId, Rect>, Vec2) {
    let mut out = HashMap::new();
    let size = place(&mut out, root, Vec2::ZERO, cell_w, cell_h, gap);
    (out, size)
}

/// Places `cell` at `origin`, recording leaf rects into `out`, and returns the
/// subtree's packed pixel size. Siblings advance by the returned packed size
/// (not tmux container dims) so nested separators are never double-counted.
fn place(
    out: &mut HashMap<PaneId, Rect>,
    cell: &Cell,
    origin: Vec2,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
) -> Vec2 {
    match cell {
        Cell::Leaf { dims, pane_id } => {
            let size = Vec2::new(dims.width as f32 * cell_w, dims.height as f32 * cell_h);
            if let Some(id) = pane_id {
                let min = origin.round();
                let max = (origin + size).round();
                out.insert(PaneId(*id), Rect::from_corners(min, max));
            }
            size
        }
        Cell::Split { dir: SplitDir::LeftRight, children, .. } => {
            let mut x = origin.x;
            let mut max_h = 0.0_f32;
            let last = children.len().saturating_sub(1);
            for (i, child) in children.iter().enumerate() {
                let csz = place(out, child, Vec2::new(x, origin.y), cell_w, cell_h, gap);
                x += csz.x;
                max_h = max_h.max(csz.y);
                if i < last {
                    x += gap;
                }
            }
            Vec2::new(x - origin.x, max_h)
        }
        Cell::Split { dir: SplitDir::TopBottom, children, .. } => {
            let mut y = origin.y;
            let mut max_w = 0.0_f32;
            let last = children.len().saturating_sub(1);
            for (i, child) in children.iter().enumerate() {
                let csz = place(out, child, Vec2::new(origin.x, y), cell_w, cell_h, gap);
                y += csz.y;
                max_w = max_w.max(csz.x);
                if i < last {
                    y += gap;
                }
            }
            Vec2::new(max_w, y - origin.y)
        }
        Cell::Split { dir: SplitDir::Floating, children, dims } => {
            for child in children {
                let d = child.dims();
                let lit = Vec2::new(d.xoff as f32 * cell_w, d.yoff as f32 * cell_h);
                place(out, child, lit, cell_w, cell_h, gap);
            }
            Vec2::new(dims.width as f32 * cell_w, dims.height as f32 * cell_h)
        }
    }
}
```

Note: `place` takes the `&mut HashMap` first (mutable-params-first rule).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ozmux-gui collapse_`
Expected: PASS — all four `collapse_*` tests.

- [ ] **Step 6: Commit**

```bash
git add src/theme.rs src/tmux_render.rs
git commit -m "feat(tmux): add pure collapse() for packed pane rects"
```

---

## Task 3: Tree-driven `layout_tmux_panes` + grey container

**Files:**
- Modify: `src/tmux_render.rs:50-70` (`attach_tmux_window_container`)
- Modify: `src/tmux_render.rs:147-207` (remove `pane_rect`, rewrite `layout_tmux_panes`)
- Modify: `src/tmux_render.rs` tests (`resize_only_updates_grid_dims_and_emits`, remove `pane_rect_scales_cell_dims_to_pixels`)

- [ ] **Step 1: Update the failing test first**

In `src/tmux_render.rs`, rewrite `resize_only_updates_grid_dims_and_emits` so the pane is a child of a window carrying a single-pane layout. Replace the entity-spawn block (lines ~401-417) with:

```rust
        let window_e = app
            .world_mut()
            .spawn((
                TmuxWindow {
                    id: WindowId(1),
                    index: 0,
                    name: String::new(),
                },
                TmuxWindowLayout(WindowLayout::parse(b"xxxx,40x10,0,0,1").unwrap()),
                Node::default(),
            ))
            .id();
        let entity = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 40,
                        height: 10,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                Node::default(),
                TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
                TerminalGrid::default(),
                ChildOf(window_e),
            ))
            .id();
```

Add to that test's local `use` lines:

```rust
        use ozmux_tmux::{TmuxWindow, TmuxWindowLayout};
        use tmux_control_parser::{WindowId, WindowLayout};
```

Also delete the entire `pane_rect_scales_cell_dims_to_pixels` test (lines ~290-302) — `pane_rect` is removed in Step 3.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux-gui resize_only_updates_grid_dims_and_emits`
Expected: FAIL to compile (`TmuxWindowLayout`/new query not yet wired) or FAIL at runtime (grid dims not updated) — confirming the test now drives the windowed structure.

- [ ] **Step 3: Give the container a grey background**

In `src/tmux_render.rs`, add `use crate::theme;` to the top-of-file `use` block. Replace the body of `attach_tmux_window_container` (lines 58-68) so the container is grey and sized by the layout pass (not 100%):

```rust
    for window in windows.iter() {
        commands.entity(window).insert((
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            BackgroundColor(theme::BORDER),
            ChildOf(root),
        ));
    }
```

(`BackgroundColor` is in the Bevy prelude — already imported via `use bevy::prelude::*;`.)

- [ ] **Step 4: Remove the flat `pane_rect`**

Delete the `pane_rect` function (lines 147-161). It is replaced by `collapse`.

- [ ] **Step 5: Rewrite `layout_tmux_panes`**

Replace `layout_tmux_panes` (lines 163-207) with the tree-driven version. Add `TmuxWindowLayout` to the `ozmux_tmux::{...}` import at the top of the file.

```rust
fn layout_tmux_panes(
    mut commands: Commands,
    mut windows: Query<(&TmuxWindowLayout, &mut Node, &Children), With<TmuxWindow>>,
    mut panes: Query<
        (&TmuxPane, &mut Node, &mut TerminalHandle, &mut TerminalGrid),
        Without<TmuxWindow>,
    >,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let dpr = window.scale_factor().max(0.5);
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0) / dpr;
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0) / dpr;

    for (layout, mut container, children) in windows.iter_mut() {
        let (rects, bbox) = collapse(&layout.0.root, cell_w, cell_h, theme::PANE_GAP_PX);

        // Size the grey container to the packed bbox so the 1px inter-pane gaps
        // bleed grey as dividers, with no grey band beyond the panes.
        if container.width != Val::Px(bbox.x) || container.height != Val::Px(bbox.y) {
            container.width = Val::Px(bbox.x);
            container.height = Val::Px(bbox.y);
        }

        for &child in children {
            let Ok((pane, mut node, mut handle, mut grid)) = panes.get_mut(child) else {
                continue;
            };
            let Some(rect) = rects.get(&pane.id) else {
                continue;
            };
            let left = rect.min.x;
            let top = rect.min.y;
            let width = rect.width();
            let height = rect.height();
            // NOTE: only write the Node fields when they actually change — writing
            // through `Mut<Node>` every frame would mark the component changed and
            // force a full UI relayout pass each tick even when nothing moved.
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
                grid.cols = cols;
                grid.rows = rows;
                handle.emit_pending(&mut commands, child);
            }
        }
    }
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p ozmux-gui resize_only_updates_grid_dims_and_emits`
Expected: PASS — grid dims become `(40, 10)`.

- [ ] **Step 7: Run the full crate test + build**

Run: `cargo test -p ozmux-gui && cargo build`
Expected: PASS / build OK. (If the `output_routed_into_pane_grid_renders_text` test fails because it ran `attach_tmux_pane_terminal` only, it is unaffected — it does not run `layout_tmux_panes`.)

- [ ] **Step 8: Commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux): tree-driven pane layout with 1px grey dividers"
```

---

## Task 4: Active-pane accent border via `Outline`

**Files:**
- Modify: `src/tmux_render.rs:79-97` (`attach_tmux_pane_terminal` — add `Outline`)
- Modify: `src/tmux_render.rs:31-47` (`OzmuxTmuxRenderPlugin::build` — register system)
- Modify: `src/tmux_render.rs` (add `sync_active_pane_outline` + test)

- [ ] **Step 1: Write the failing test**

In `src/tmux_render.rs` test module, add:

```rust
    #[test]
    fn active_pane_outline_is_accent_inactive_is_none() {
        use bevy::prelude::*;
        use ozmux_tmux::{ActivePane, TmuxPane};

        let mut app = App::new();
        app.add_systems(Update, sync_active_pane_outline);
        let active = app
            .world_mut()
            .spawn((
                TmuxPane { id: PaneId(1), dims: dims() },
                ActivePane,
                Outline::new(Val::Px(1.0), Val::Px(0.0), Color::NONE),
            ))
            .id();
        let inactive = app
            .world_mut()
            .spawn((
                TmuxPane { id: PaneId(2), dims: dims() },
                Outline::new(Val::Px(1.0), Val::Px(0.0), Color::NONE),
            ))
            .id();
        app.update();

        assert_eq!(app.world().get::<Outline>(active).unwrap().color, theme::ACCENT);
        assert_eq!(app.world().get::<Outline>(inactive).unwrap().color, Color::NONE);
    }
```

This relies on the existing `fn dims()` helper in the test module (returns a `CellDims`).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux-gui active_pane_outline_is_accent_inactive_is_none`
Expected: FAIL to compile — `sync_active_pane_outline` is not defined.

- [ ] **Step 3: Add `Outline` to panes at attach**

In `src/tmux_render.rs`, add `ActivePane` to the `ozmux_tmux::{...}` import. In `attach_tmux_pane_terminal`, add an `Outline` (hidden by default) to the inserted bundle:

```rust
        commands.entity(entity).insert((
            handle,
            TerminalRenderBundle::new(material),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Outline::new(Val::Px(theme::PANE_BORDER_PX), Val::Px(0.0), Color::NONE),
        ));
```

(`Outline`, `Val`, `Color` are in the Bevy prelude.)

- [ ] **Step 4: Add the sync system**

Add to `src/tmux_render.rs` (below `layout_tmux_panes`):

```rust
/// Recolors each pane's `Outline`: the accent color on the pane carrying
/// `ActivePane`, transparent otherwise. Recoloring (not insert/remove) avoids
/// ECS table moves on every active-pane change.
fn sync_active_pane_outline(mut panes: Query<(Has<ActivePane>, &mut Outline), With<TmuxPane>>) {
    for (active, mut outline) in panes.iter_mut() {
        let want = if active { theme::ACCENT } else { Color::NONE };
        if outline.color != want {
            outline.color = want;
        }
    }
}
```

- [ ] **Step 5: Register the system**

In `OzmuxTmuxRenderPlugin::build`, add `sync_active_pane_outline` to the chained `Update` tuple after `layout_tmux_panes`:

```rust
        app.add_systems(
            Update,
            (
                attach_tmux_window_container,
                attach_tmux_pane_terminal,
                route_tmux_output.run_if(on_message::<PaneOutput>),
                sync_active_window,
                layout_tmux_panes,
                sync_active_pane_outline,
            )
                .chain()
                .after(TmuxProjectionSet),
        );
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p ozmux-gui active_pane_outline_is_accent_inactive_is_none`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/tmux_render.rs
git commit -m "feat(tmux): accent Outline on the active pane"
```

---

## Task 5: Verify, lint, and confirm the Outline spike

**Files:** none (verification only), plus a possible fallback note.

- [ ] **Step 1: Full workspace test**

Run: `cargo test`
Expected: PASS across the workspace.

- [ ] **Step 2: Lint + format**

Run: `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`
Expected: no remaining warnings; tree formatted. Re-run `cargo build` to confirm it still compiles.

- [ ] **Step 3: Manual visual verification (the Outline-over-material spike)**

Run: `cargo run`
Then inside the app: split the active tmux pane horizontally and vertically (so at least a 2×2 grid exists).
Verify ALL of:
1. Adjacent panes are separated by a thin (~1px) grey line, with no wide empty gap.
2. There is no large grey band on the right/bottom edges beyond the panes.
3. The active pane shows a 1px accent (blue) border; switching panes (click another pane) moves the accent border and the previously-active pane's border disappears.
4. The accent border is not occluded by neighboring panes.

- [ ] **Step 4: If the spike fails — apply the documented fallback**

Only if Step 3 item 3 or 4 fails (Outline does not render over `MaterialNode<TerminalUiMaterial>` or is occluded): implement the spec's fallback (§5) — a separate inflated overlay `Node` with `Node.border = UiRect::all(Val::Px(1.0))`, `BorderColor::all(theme::ACCENT)`, transparent background, `FocusPolicy::Pass`, `GlobalZIndex`, positioned at the active pane's packed rect inflated by 1px on each side, registered after `layout_tmux_panes`. Revert the Task 4 `Outline` changes in that case. Re-run Step 1-3.

- [ ] **Step 5: Final commit (only if Step 2 or Step 4 changed files)**

```bash
git add -A
git commit -m "chore(tmux): clippy/fmt + verify pane border rendering"
```

---

## Self-Review Notes

- **Spec coverage:** Task 1 = §1 (tree retention, corrected event threading). Task 2 = §2 (collapse, packed-extent advance, pixel snap). Task 3 = §3 + §4 (tree-driven layout, container bbox/grey bleed, no outer band). Task 4 = §5 (active accent via `Outline`). Task 5 = §5 spike + fallback, plus the test section's collapse cases (single / 2-split / nested / `pane_id: None`).
- **Margin/asymmetry artifacts** (spec edge cases) are inherent and accepted; Step 3 manual checks bound them visually.
- **Type consistency:** `collapse(&Cell, f32, f32, f32) -> (HashMap<PaneId, Rect>, Vec2)` and `place(&mut HashMap, &Cell, Vec2, f32, f32, f32) -> Vec2` are used identically across Task 2 and Task 3. `TmuxWindowLayout(pub WindowLayout)` field access `.0.root` matches Task 1's definition. `TmuxLayoutChanged.layout` is used consistently in every construction site listed in the File Structure note.
