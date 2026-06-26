# IME Preedit Overlay — Cell-Grid Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate IME preedit caret drift by rendering the preedit as one cell-aligned `Text` node per grapheme cluster, so text body, caret, clause highlight, and occluding background all share the same monospace cell arithmetic.

**Architecture:** Replace the single proportionally-shaped overlay `Text` with a pooled set of per-grapheme leaf `Text` nodes, each absolutely positioned at `pos.x + cum_cells * cell_w_logical`. `ImeOverlayNode` becomes a background-only occluding rect; a new `ImeUnderline` bar draws the continuous underline; caret/clause math is unchanged (already cell-correct).

**Tech Stack:** Rust 2024 (toolchain 1.95), Bevy 0.18 ECS/UI, `unicode-segmentation` (grapheme clustering), `unicode-width` (cell width). Single binary; the only files touched are `src/ui/ime_overlay.rs` and two `Cargo.toml`s.

## Global Constraints

- Rust edition 2024, toolchain pinned `1.95`. All in-code comments in English.
- Comment taxonomy: only `// TODO:` / `// NOTE:` (critical caveat) / `// SAFETY:`. No narrative/block/commented-out code.
- No `mod.rs`. All `use` at top of file in one contiguous block; no inline fully-qualified paths.
- Visibility: items used only within `ime_overlay.rs` MUST be private (no modifier). Doc-comment every externally-`pub` item; `//!` on the file module.
- Bevy: gate whole-system change guards with `run_if` (N/A here — this system runs every frame to track pane movement). Mutate conditionally (equality-guard writes) so change detection fires only on real changes — no `set_changed()`/`bypass_change_detection()`.
- `Plugin::build` is a single method chain. Systems/observers are registered by the plugin in their defining file.
- Parameter ordering: mutable params before immutable in every `fn` signature.
- Prefer `#[expect(..., reason = "...")]` over `#[allow(...)]`.
- Test commands: `cargo test -p ozmux ime_overlay` (the binary crate is `ozmux`). Lint: `cargo clippy --workspace` then `cargo fmt`.

---

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `Cargo.toml` (root) | workspace dep catalog + binary deps | Promote `unicode-segmentation` to `[workspace.dependencies]`; add `{ workspace = true }` to binary deps |
| `crates/ozma_tty_renderer/Cargo.toml` | renderer crate deps | Point `unicode-segmentation` at `{ workspace = true }` |
| `src/ui/ime_overlay.rs` | the IME overlay: pure layout helper, markers, pool resource, spawn, position system, tests | All implementation lives here |

---

## Task 1: Promote `unicode-segmentation` to a workspace dependency

**Files:**
- Modify: `Cargo.toml` (root — `[workspace.dependencies]` and binary `[dependencies]`)
- Modify: `crates/ozma_tty_renderer/Cargo.toml:14`

**Interfaces:**
- Produces: `unicode-segmentation` resolvable from the `ozmux` binary crate via `use unicode_segmentation::UnicodeSegmentation;`.

- [ ] **Step 1: Add to `[workspace.dependencies]`**

In root `Cargo.toml`, find the `[workspace.dependencies]` block (it already contains `unicode-width = "0.2"`) and add directly below it:

```toml
unicode-segmentation = "1"
```

- [ ] **Step 2: Reference it from the binary crate**

In root `Cargo.toml`, find the binary `[dependencies]` block (it already contains `unicode-width = { workspace = true }`) and add directly below that line:

```toml
unicode-segmentation = { workspace = true }
```

- [ ] **Step 3: Point the renderer crate at the workspace dep**

In `crates/ozma_tty_renderer/Cargo.toml`, replace line 14:

```toml
unicode-segmentation = "1"
```

with:

```toml
unicode-segmentation = { workspace = true }
```

- [ ] **Step 4: Verify the workspace builds**

Run: `cargo build -p ozmux -p ozma_tty_renderer`
Expected: builds successfully (no version-resolution errors; `unicode-segmentation` unifies to one version).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/ozma_tty_renderer/Cargo.toml
git commit -m "build: promote unicode-segmentation to a workspace dependency"
```

---

## Task 2: Pure cell-layout helper `layout_preedit_cells`

**Files:**
- Modify: `src/ui/ime_overlay.rs` (add `use`, `CellPlacement`, `layout_preedit_cells`, tests)

**Interfaces:**
- Produces:
  - `struct CellPlacement { text: String, left: f32 }` (private; fields private)
  - `fn layout_preedit_cells(text: &str, cell_w_logical: f32, origin_x: f32) -> (Vec<CellPlacement>, u32)` (private) — returns the per-grapheme placements and the total cell count. Width follows the renderer's `runs_to_cells` rule: `width >= 2` → 2 cells, `width == 0` → 0 cells (merged into the previous placement's `text`).

- [ ] **Step 1: Add the import**

In the contiguous `use` block at the top of `src/ui/ime_overlay.rs`, alongside the existing `use unicode_width::UnicodeWidthStr;`, add:

```rust
use unicode_segmentation::UnicodeSegmentation;
```

- [ ] **Step 2: Write the failing tests**

Inside the existing `#[cfg(test)] mod tests { ... }` block in `src/ui/ime_overlay.rs`, add:

```rust
#[test]
fn layout_preedit_cells_ascii() {
    let (cells, total) = layout_preedit_cells("abc", 10.0, 100.0);
    assert_eq!(total, 3);
    let lefts: Vec<f32> = cells.iter().map(|c| c.left).collect();
    assert_eq!(lefts, vec![100.0, 110.0, 120.0]);
    let texts: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["a", "b", "c"]);
}

#[test]
fn layout_preedit_cells_fullwidth_cjk_consumes_two_cells_each() {
    // Each hiragana is 2 cells; cells start at 0 and 2 columns.
    let (cells, total) = layout_preedit_cells("あい", 10.0, 0.0);
    assert_eq!(total, 4);
    let lefts: Vec<f32> = cells.iter().map(|c| c.left).collect();
    assert_eq!(lefts, vec![0.0, 20.0]);
}

#[test]
fn layout_preedit_cells_mixed_ascii_and_cjk() {
    // "a"(1) + "あ"(2) + "b"(1): lefts at 0, 1, 3 columns.
    let (cells, total) = layout_preedit_cells("aあb", 10.0, 0.0);
    assert_eq!(total, 4);
    let lefts: Vec<f32> = cells.iter().map(|c| c.left).collect();
    assert_eq!(lefts, vec![0.0, 10.0, 30.0]);
}

#[test]
fn layout_preedit_cells_combining_mark_merges_into_previous() {
    // "e" + U+0301 (combining acute, width 0): one placement, total 1 cell.
    let (cells, total) = layout_preedit_cells("e\u{0301}", 10.0, 0.0);
    assert_eq!(total, 1);
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].text, "e\u{0301}");
    assert_eq!(cells[0].left, 0.0);
}

#[test]
fn layout_preedit_cells_empty() {
    let (cells, total) = layout_preedit_cells("", 10.0, 0.0);
    assert_eq!(total, 0);
    assert!(cells.is_empty());
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p ozmux ime_overlay::tests::layout_preedit_cells -- --nocapture`
Expected: FAIL with `cannot find function 'layout_preedit_cells'`.

- [ ] **Step 4: Implement the helper**

Add to `src/ui/ime_overlay.rs` (place it directly after `caret_cell_offsets`, keeping pure helpers together; it is private). Mark it `#[expect(dead_code)]` until Task 4 wires it into the system:

```rust
/// A single placed preedit cell-unit: the grapheme cluster's text and its
/// left edge in logical px (the cell origin it is anchored to).
struct CellPlacement {
    text: String,
    left: f32,
}

/// Splits `text` into grapheme clusters and assigns each a cell-aligned
/// `left` edge, returning `(placements, total_cells)`.
///
/// Cluster width follows the renderer's `runs_to_cells` rule
/// (`crates/ozma_tty_renderer/src/grid.rs`): a `width >= 2` cluster consumes
/// 2 cells, a `width == 0` cluster (lone combining mark) consumes 0 cells and
/// merges into the previous placement's text. `origin_x` is the composition's
/// left edge; `cell_w_logical` is the floored cell pitch — both in logical px.
#[expect(dead_code, reason = "wired into position_ime_overlay in a later task")]
fn layout_preedit_cells(text: &str, cell_w_logical: f32, origin_x: f32) -> (Vec<CellPlacement>, u32) {
    let mut placements: Vec<CellPlacement> = Vec::new();
    let mut cum_cells: u32 = 0;
    for cluster in text.graphemes(true) {
        let cells = match UnicodeWidthStr::width(cluster) {
            0 => {
                if let Some(last) = placements.last_mut() {
                    last.text.push_str(cluster);
                }
                continue;
            }
            1 => 1,
            _ => 2,
        };
        placements.push(CellPlacement {
            text: cluster.to_string(),
            left: origin_x + cum_cells as f32 * cell_w_logical,
        });
        cum_cells += cells;
    }
    (placements, cum_cells)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p ozmux ime_overlay::tests::layout_preedit_cells`
Expected: PASS (5 tests).

- [ ] **Step 6: Commit**

```bash
git add src/ui/ime_overlay.rs
git commit -m "feat(ime): add pure layout_preedit_cells cell-layout helper"
```

---

## Task 3: Add grapheme-cell pool, underline bar, and markers (dormant)

This task adds the new entities/resource and spawns them, **without** changing
`position_ime_overlay` or the `ImeOverlayNode` text rendering yet, so existing
behavior and tests stay green. The new entities are spawned hidden and idle.

**Files:**
- Modify: `src/ui/ime_overlay.rs` (markers, resource, plugin `init_resource`, `spawn_ime_overlay_once`, `spawn_grapheme_cell` helper, spawn test)

**Interfaces:**
- Consumes: `TerminalUiFont`, `TerminalFontSize` (already imported), `IME_OVERLAY_Z`.
- Produces:
  - `struct ImeGraphemeCell;` (private marker)
  - `struct ImeUnderline;` (private marker)
  - `struct ImeGraphemePool(Vec<Entity>)` (private `Resource`, `Default`)
  - `fn spawn_grapheme_cell(commands: &mut Commands, ui_font: &TerminalUiFont, font_size: &TerminalFontSize, text: &str, left: f32, top: f32, display: Display) -> Entity` (private)
  - `const INITIAL_POOL_CAP: usize = 16;`

- [ ] **Step 1: Write the failing spawn test**

In the `#[cfg(test)] mod tests` block, add:

```rust
#[test]
fn spawn_creates_grapheme_pool_and_underline() {
    use bevy::asset::Handle;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(crate::font::TerminalUiFont(Handle::default()));
    app.insert_resource(TerminalFontSize(12.0));
    app.init_resource::<ImeGraphemePool>();
    app.add_systems(Startup, spawn_ime_overlay_once);
    app.update();

    assert_eq!(
        app.world().resource::<ImeGraphemePool>().0.len(),
        INITIAL_POOL_CAP,
        "the grapheme pool must be pre-spawned at the initial capacity"
    );
    let mut underlines = app.world_mut().query_filtered::<Entity, With<ImeUnderline>>();
    assert_eq!(
        underlines.iter(app.world()).count(),
        1,
        "exactly one underline bar must be spawned"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux ime_overlay::tests::spawn_creates_grapheme_pool_and_underline`
Expected: FAIL with `cannot find type 'ImeGraphemePool'` / `ImeUnderline` / `INITIAL_POOL_CAP`.

- [ ] **Step 3: Add markers, resource, and the spawn helper**

In `src/ui/ime_overlay.rs`, add near the other markers (after `ImeClauseHighlight`):

```rust
/// Marker for a pooled per-grapheme preedit `Text` node. Each is an
/// independent top-level UI entity (never a child of another node) so it stays
/// a Taffy leaf and the text measure func drives its `ComputedNode.size`.
#[derive(Component)]
struct ImeGraphemeCell;

/// Marker for the single continuous underline bar drawn under the whole
/// preedit. A solid `Node` bar (not Bevy's per-glyph `Underline`) so it has no
/// gaps under fullwidth glyphs narrower than their cell span.
#[derive(Component)]
struct ImeUnderline;

/// Pool of `ImeGraphemeCell` entities, reused across compositions and grown on
/// demand. Index `i` holds the i-th visible grapheme; entries past the active
/// composition length are hidden (`Display::None`).
#[derive(Resource, Default)]
struct ImeGraphemePool(Vec<Entity>);

/// Initial number of pooled grapheme nodes pre-spawned at Startup. Covers
/// typical short compositions without runtime growth; longer compositions grow
/// the pool on demand.
const INITIAL_POOL_CAP: usize = 16;
```

Then add the spawn helper near `spawn_ime_overlay_once` (private, bottom of the file with the other helpers):

```rust
/// Spawns one `ImeGraphemeCell` leaf `Text` node, configured with `text`, an
/// absolute `left`/`top`, and `display`. Used both to pre-spawn hidden pool
/// nodes and to grow the pool with already-positioned nodes.
fn spawn_grapheme_cell(
    commands: &mut Commands,
    ui_font: &TerminalUiFont,
    font_size: &TerminalFontSize,
    text: &str,
    left: f32,
    top: f32,
    display: Display,
) -> Entity {
    commands
        .spawn((
            Text::new(text),
            TextFont {
                font: ui_font.0.clone(),
                font_size: font_size.0,
                ..default()
            },
            TextColor(Color::WHITE),
            TextLayout {
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            Node {
                position_type: PositionType::Absolute,
                display,
                left: Val::Px(left),
                top: Val::Px(top),
                ..default()
            },
            GlobalZIndex(IME_OVERLAY_Z),
            ImeGraphemeCell,
        ))
        .id()
}
```

- [ ] **Step 4: Register the resource in the plugin**

In `impl Plugin for ImeOverlayPlugin`, extend the `build` method chain to init the pool resource. Change:

```rust
app.add_systems(
    Startup,
    spawn_ime_overlay_once.after(TerminalFontInitSet::InitCellMetrics),
)
```

to:

```rust
app.init_resource::<ImeGraphemePool>()
    .add_systems(
        Startup,
        spawn_ime_overlay_once.after(TerminalFontInitSet::InitCellMetrics),
    )
```

(The `.add_systems(PostUpdate, ...)` call that follows stays chained as-is.)

- [ ] **Step 5: Spawn the underline bar and the initial pool**

Add `mut pool: ResMut<ImeGraphemePool>` as the **first** parameter of `spawn_ime_overlay_once` (mutable-params-first):

```rust
fn spawn_ime_overlay_once(
    mut commands: Commands,
    mut pool: ResMut<ImeGraphemePool>,
    ui_font: Res<TerminalUiFont>,
    font_size: Res<TerminalFontSize>,
) {
```

At the end of `spawn_ime_overlay_once` (after the existing `ImeClauseHighlight` spawn block), add:

```rust
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            width: Val::Px(0.0),
            height: Val::Px(1.0),
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            ..default()
        },
        BackgroundColor(Color::WHITE),
        GlobalZIndex(IME_OVERLAY_Z),
        ImeUnderline,
    ));

    pool.0 = (0..INITIAL_POOL_CAP)
        .map(|_| {
            spawn_grapheme_cell(
                &mut commands,
                &ui_font,
                &font_size,
                "",
                0.0,
                0.0,
                Display::None,
            )
        })
        .collect();
```

- [ ] **Step 6: Fix the two existing tests that call `spawn_ime_overlay_once` directly**

`spawn_ime_overlay_once` now requires `ResMut<ImeGraphemePool>`, so every test that
runs it as a Startup system must insert that resource or Bevy panics. Two existing
tests do: `ime_overlay_uses_terminal_font_size` and
`overlay_background_matches_pane_default_bg_while_composing`. In **each** of them,
add this line right after the other `insert_resource` calls and before
`app.add_systems(Startup, spawn_ime_overlay_once);`:

```rust
        app.init_resource::<ImeGraphemePool>();
```

- [ ] **Step 7: Run the spawn test (and the existing suite) to verify green**

Run: `cargo test -p ozmux ime_overlay`
Expected: PASS — the new `spawn_creates_grapheme_pool_and_underline` passes, and all existing `ime_overlay` tests still pass (behavior unchanged; new entities are idle).

- [ ] **Step 8: Commit**

```bash
git add src/ui/ime_overlay.rs
git commit -m "feat(ime): pre-spawn grapheme-cell pool and underline bar (dormant)"
```

---

## Task 4: Rewrite `position_ime_overlay` for cell-grid alignment

Switch the overlay to grid-aligned rendering: repurpose `ImeOverlayNode` to a
background-only rect, drive the grapheme pool, position the underline bar, and
keep caret/clause math. This is the behavior change that fixes the drift.

**Files:**
- Modify: `src/ui/ime_overlay.rs` (`spawn_ime_overlay_once` background node, `position_ime_overlay`, new `Node`-mutation helpers, remove the `#[expect(dead_code)]`, update/add tests)

**Interfaces:**
- Consumes: `CellPlacement`, `layout_preedit_cells` (Task 2); `ImeGraphemeCell`, `ImeUnderline`, `ImeGraphemePool`, `spawn_grapheme_cell` (Task 3); `compute_overlay_pos`, `caret_cell_offsets`, `resolve_focused_surface` (existing).
- Produces:
  - `fn set_node_display(nodes: &mut Query<&mut Node>, entity: Entity, display: Display)` (private)
  - `fn set_node_rect(node: &mut Node, left: f32, top: f32, width: f32, height: f32)` (private)

- [ ] **Step 1: Make `ImeOverlayNode` a background-only node in spawn**

In `spawn_ime_overlay_once`, replace the existing `ImeOverlayNode` spawn block (the one that spawns `Text::new("")`, `TextFont`, `TextColor`, `TextLayout`, `Underline`, `UnderlineColor`, `Node{...}`, `GlobalZIndex`, `ImeOverlayNode`) with a background-only node:

```rust
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            width: Val::Px(0.0),
            height: Val::Px(0.0),
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            ..default()
        },
        GlobalZIndex(IME_OVERLAY_Z),
        ImeOverlayNode,
    ));
```

(`BackgroundColor` is auto-inserted by `Node`'s required components, so it need
not be listed. The `Underline`/`UnderlineColor`/`Text`/`TextFont`/`TextLayout`
components are intentionally gone — text now lives on the grapheme pool, the
underline on `ImeUnderline`.)

After this edit, the now-unused imports `Underline`, `UnderlineColor` must be
removed from the top `use` block to avoid `unused_import` warnings.

- [ ] **Step 2: Add the `Node`-mutation helpers**

Add near the other private helpers in `src/ui/ime_overlay.rs`:

```rust
/// Sets `node.display` on `entity` only when it differs, so change detection
/// fires only on a real change.
fn set_node_display(nodes: &mut Query<&mut Node>, entity: Entity, display: Display) {
    if let Ok(mut node) = nodes.get_mut(entity)
        && node.display != display
    {
        node.display = display;
    }
}

/// Writes `left`/`top`/`width`/`height` (logical px) into `node`, each guarded
/// by an equality check so change detection fires only on a real change.
fn set_node_rect(node: &mut Node, left: f32, top: f32, width: f32, height: f32) {
    let left = Val::Px(left);
    if node.left != left {
        node.left = left;
    }
    let top = Val::Px(top);
    if node.top != top {
        node.top = top;
    }
    let width = Val::Px(width);
    if node.width != width {
        node.width = width;
    }
    let height = Val::Px(height);
    if node.height != height {
        node.height = height;
    }
}
```

- [ ] **Step 3: Remove the `#[expect(dead_code)]` from `layout_preedit_cells`**

Delete the line `#[expect(dead_code, reason = "wired into position_ime_overlay in a later task")]` above `layout_preedit_cells` (it is now used; the `expect` would otherwise fail the build).

- [ ] **Step 4: Replace `position_ime_overlay` wholesale**

Replace the entire `position_ime_overlay` function with:

```rust
/// PostUpdate system that grid-aligns the IME preedit overlay at the attached
/// terminal's cursor cell. Lays out the composition as one cell-anchored
/// `Text` node per grapheme cluster (pooled in [`ImeGraphemePool`], grown on
/// demand), draws an occluding background rect and a continuous underline bar,
/// and positions the caret beam (`begin == end`) or clause highlight
/// (`begin != end`). Every visible element uses the same cell arithmetic, so
/// the caret cannot drift from the text.
///
/// When `ImeState` has no composition — or the focused surface / window is
/// missing — hides every overlay part and returns.
fn position_ime_overlay(
    mut commands: Commands,
    mut pool: ResMut<ImeGraphemePool>,
    mut nodes: Query<&mut Node>,
    mut cell_texts: Query<&mut Text, With<ImeGraphemeCell>>,
    mut overlay_bg: Query<&mut BackgroundColor, With<ImeOverlayNode>>,
    state: Res<ImeState>,
    metrics: Res<TerminalCellMetricsResource>,
    ui_font: Res<TerminalUiFont>,
    font_size: Res<TerminalFontSize>,
    focused: Query<Entity, With<KeyboardFocused>>,
    anchors: Query<(&ComputedNode, &UiGlobalTransform, &TerminalGrid)>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    background: Query<Entity, With<ImeOverlayNode>>,
    underline: Query<Entity, With<ImeUnderline>>,
    caret: Query<Entity, With<ImeCaretBar>>,
    clause: Query<Entity, With<ImeClauseHighlight>>,
) {
    let Ok(bg_entity) = background.single() else {
        return;
    };
    let underline_entity = underline.single().ok();
    let caret_entity = caret.single().ok();
    let clause_entity = clause.single().ok();

    // Default: hide every overlay part each frame. The success path re-shows
    // the active ones. Guarantees no leak past a commit / cancel / focus loss.
    set_node_display(&mut nodes, bg_entity, Display::None);
    for entity in [underline_entity, caret_entity, clause_entity]
        .into_iter()
        .flatten()
    {
        set_node_display(&mut nodes, entity, Display::None);
    }
    for index in 0..pool.0.len() {
        set_node_display(&mut nodes, pool.0[index], Display::None);
    }

    let Some(comp) = state.composition() else {
        return;
    };
    let Some(entity) = resolve_focused_surface(&focused) else {
        return;
    };
    let Ok((node, ui_xform, grid)) = anchors.get(entity) else {
        return;
    };
    let Ok(window) = primary_window.single() else {
        return;
    };

    let scale = window.resolution.scale_factor();
    let cursor_cell = grid.cursor.as_ref().map(|c| (c.x, c.y)).unwrap_or((0, 0));
    let cell_w_logical = metrics.metrics.advance_phys.floor().max(1.0) / scale;
    let line_h_logical = metrics.metrics.line_height_phys.floor().max(1.0) / scale;

    // Lay out once at origin 0 to get the true total width, then anchor with
    // the real width so `compute_overlay_pos` can clamp at the pane edge, then
    // lay out again at the clamped origin.
    let (_, total_cells) = layout_preedit_cells(comp.text(), cell_w_logical, 0.0);
    let total_width_logical = total_cells as f32 * cell_w_logical;
    let pos = compute_overlay_pos(
        ui_xform.translation,
        node.size,
        cursor_cell,
        &metrics.metrics,
        total_width_logical,
        scale,
    );
    let (placements, _) = layout_preedit_cells(comp.text(), cell_w_logical, pos.x);

    // Occluding background rect.
    if let Ok(mut bg_node) = nodes.get_mut(bg_entity) {
        set_node_rect(&mut bg_node, pos.x, pos.y, total_width_logical, line_h_logical);
        if bg_node.display != Display::Flex {
            bg_node.display = Display::Flex;
        }
    }
    if let Ok(mut bg) = overlay_bg.single_mut() {
        let occluding =
            Color::srgb_u8(grid.default_bg[0], grid.default_bg[1], grid.default_bg[2]);
        if bg.0 != occluding {
            bg.0 = occluding;
        }
    }

    // Grapheme cells: reuse pool entries, growing the pool when short.
    for (index, placement) in placements.iter().enumerate() {
        if let Some(&cell) = pool.0.get(index) {
            if let Ok(mut node) = nodes.get_mut(cell) {
                let left = Val::Px(placement.left);
                if node.left != left {
                    node.left = left;
                }
                let top = Val::Px(pos.y);
                if node.top != top {
                    node.top = top;
                }
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
            }
            if let Ok(mut text) = cell_texts.get_mut(cell)
                && text.0 != placement.text
            {
                text.0 = placement.text.clone();
            }
        } else {
            // NOTE: grown entities are not in `nodes`/`cell_texts` this frame,
            // so they are spawned already configured; their tail appears one
            // frame late only on the growth frame (same latency class as the
            // overlay anchor NOTE above).
            let cell = spawn_grapheme_cell(
                &mut commands,
                &ui_font,
                &font_size,
                &placement.text,
                placement.left,
                pos.y,
                Display::Flex,
            );
            pool.0.push(cell);
        }
    }

    // Continuous underline bar. `underline_position_phys` is baseline-relative
    // and negative, so fold in ascent to land it below the baseline.
    if let Some(underline_entity) = underline_entity
        && let Ok(mut node) = nodes.get_mut(underline_entity)
    {
        let underline_top = pos.y
            + (metrics.metrics.ascent_phys - metrics.metrics.underline_position_phys) / scale;
        let underline_h = (metrics.metrics.underline_thickness_phys / scale).max(1.0);
        set_node_rect(
            &mut node,
            pos.x,
            underline_top,
            total_width_logical,
            underline_h,
        );
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
    }

    let (begin_cells, end_cells) = match comp.caret() {
        Some(range) => caret_cell_offsets(comp.text(), range),
        None => (0.0, 0.0),
    };
    let has_clause = comp.caret().is_some_and(|(b, e)| b != e);
    let has_beam = comp.caret().is_some() && !has_clause;

    if has_beam
        && let Some(caret_entity) = caret_entity
        && let Ok(mut node) = nodes.get_mut(caret_entity)
    {
        let left = Val::Px(pos.x + end_cells * cell_w_logical);
        if node.left != left {
            node.left = left;
        }
        let top = Val::Px(pos.y);
        if node.top != top {
            node.top = top;
        }
        let height = Val::Px(line_h_logical);
        if node.height != height {
            node.height = height;
        }
        node.display = Display::Flex;
    }

    if has_clause
        && let Some(clause_entity) = clause_entity
        && let Ok(mut node) = nodes.get_mut(clause_entity)
    {
        set_node_rect(
            &mut node,
            pos.x + begin_cells * cell_w_logical,
            pos.y,
            (end_cells - begin_cells) * cell_w_logical,
            line_h_logical,
        );
        node.display = Display::Flex;
    }
}
```

- [ ] **Step 5: Confirm the existing background test still holds**

`overlay_background_matches_pane_default_bg_while_composing` already inserts
`ImeGraphemePool` (added in Task 3 Step 6). Its assertions (overlay
`BackgroundColor == default_bg`, `Display::Flex`) remain valid because
`ImeOverlayNode` is now the occluding background rect. Leave the test body
unchanged — no edit needed in this step; it is called out so you verify it passes
rather than assuming.

- [ ] **Step 6: Add grid-alignment system tests**

Add to the `#[cfg(test)] mod tests` block. These assert the pool node lefts and the caret beam land on exact cell boundaries:

```rust
fn run_overlay_with_composition(value: &str, caret: Option<(usize, usize)>) -> App {
    use bevy::asset::Handle;
    use bevy::window::WindowResolution;
    use ozma_terminal::OzmaTerminal;
    use ozma_tty_renderer::prelude::Cursor;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(crate::font::TerminalUiFont(Handle::default()));
    app.insert_resource(TerminalFontSize(12.0));
    // advance 10, line height 16 → cell pitch 10×16 logical px at scale 1.
    app.insert_resource(TerminalCellMetricsResource {
        metrics: metrics(10.0, 16.0),
        phys_font_size: 12,
    });
    app.init_resource::<ImeGraphemePool>();

    let mut state = ImeState::default();
    apply_event(
        &mut state,
        &Ime::Preedit {
            window: Entity::PLACEHOLDER,
            value: value.into(),
            cursor: caret,
        },
    );
    app.insert_resource(state);

    app.add_systems(Startup, spawn_ime_overlay_once);
    app.world_mut().spawn((
        Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        },
        PrimaryWindow,
    ));
    app.world_mut().spawn((
        OzmaTerminal,
        KeyboardFocused,
        ComputedNode {
            size: Vec2::new(800.0, 600.0),
            ..ComputedNode::DEFAULT
        },
        UiGlobalTransform::from_xy(400.0, 300.0),
        TerminalGrid {
            cursor: Some(Cursor::default()),
            default_bg: [0, 0, 0],
            ..TerminalGrid::default()
        },
    ));

    app.update();
    app.world_mut()
        .run_system_once(position_ime_overlay)
        .unwrap();
    app
}

#[test]
fn ascii_grapheme_cells_land_on_cell_boundaries() {
    // Cursor at (0,0), cell pitch 10 → cells at x = 0, 10, 20.
    let mut app = run_overlay_with_composition("abc", Some((3, 3)));
    let pool = app.world().resource::<ImeGraphemePool>().0.clone();
    let lefts: Vec<Val> = pool
        .iter()
        .take(3)
        .map(|&e| app.world().get::<Node>(e).unwrap().left)
        .collect();
    assert_eq!(lefts, vec![Val::Px(0.0), Val::Px(10.0), Val::Px(20.0)]);

    let mut caret = app.world_mut().query_filtered::<&Node, With<ImeCaretBar>>();
    // Caret beam at end of "abc" → 3 cells × 10 = x 30, exactly the suffix.
    assert_eq!(caret.single(app.world()).unwrap().left, Val::Px(30.0));
}

#[test]
fn cjk_caret_lands_at_fullwidth_suffix_without_drift() {
    // "あい" = 4 cells; caret at end → x = 40, the exact suffix boundary.
    let mut app = run_overlay_with_composition("あい", Some((6, 6)));
    let mut caret = app.world_mut().query_filtered::<&Node, With<ImeCaretBar>>();
    assert_eq!(caret.single(app.world()).unwrap().left, Val::Px(40.0));

    let pool = app.world().resource::<ImeGraphemePool>().0.clone();
    let lefts: Vec<Val> = pool
        .iter()
        .take(2)
        .map(|&e| app.world().get::<Node>(e).unwrap().left)
        .collect();
    assert_eq!(lefts, vec![Val::Px(0.0), Val::Px(20.0)]);
}
```

- [ ] **Step 7: Run the full overlay suite**

Run: `cargo test -p ozmux ime_overlay`
Expected: PASS — `layout_preedit_cells_*`, `spawn_creates_grapheme_pool_and_underline`, `overlay_background_matches_pane_default_bg_while_composing`, `ascii_grapheme_cells_land_on_cell_boundaries`, `cjk_caret_lands_at_fullwidth_suffix_without_drift`, and all pre-existing tests.

- [ ] **Step 8: Lint and format**

Run: `cargo clippy --workspace --all-targets && cargo fmt`
Expected: no warnings (in particular no `unused_import` for `Underline`/`UnderlineColor`, no `dead_code`). Fix any clippy findings, then re-run.

- [ ] **Step 9: Commit**

```bash
git add src/ui/ime_overlay.rs
git commit -m "fix(ime): grid-align preedit overlay so the caret tracks the suffix"
```

---

## Task 5: Full verification

**Files:** none (verification only).

- [ ] **Step 1: Run the binary crate's whole test suite**

Run: `cargo test -p ozmux`
Expected: PASS (no regressions outside `ime_overlay`).

- [ ] **Step 2: Workspace lint + format check**

Run: `cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: clean clippy, formatting already applied.

- [ ] **Step 3: Manual smoke (interactive — request the user run it)**

Ask the user to run `cargo run`, focus a pane, and type a long Japanese
composition (e.g. keep typing kana before converting). Confirm the caret beam
stays glued to the end of the preedit text as it grows — no accumulating gap —
and the underline spans the full composition.

- [ ] **Step 4: Final commit (if any fmt/clippy fixes landed)**

```bash
git add -A
git commit -m "chore(ime): finalize grid-align overlay (lint/fmt)"
```

---

## Self-Review

**Spec coverage:**
- Root-cause fix (cell-grid alignment) → Tasks 2 + 4. ✓
- Entity structure (`ImeOverlayNode` background-only, `ImeGraphemeCell` pool, `ImeUnderline`, caret/clause unchanged) → Tasks 3 + 4. ✓
- Pure `layout_preedit_cells` with `runs_to_cells` width clamping → Task 2. ✓
- `unicode-segmentation` promoted to `[workspace.dependencies]` → Task 1. ✓
- Entity pool with grow-via-`Commands` + one-frame tail NOTE → Tasks 3 + 4. ✓
- Underline y folds in ascent (`pos.y + (ascent − underline_position)/scale`) → Task 4 Step 4. ✓
- Background explicit `left/top/width/height` → Task 4 Step 4. ✓
- Edge clamp via real total width (drops `measured_width = 0.0` shortcut) → Task 4 Step 4. ✓
- Equality-guarded writes incl. per-node `Display` → Task 4 helpers + inline guards. ✓
- Tests: `layout_preedit_cells` units, system-level cell-boundary + CJK no-drift, updated background test → Tasks 2 + 4. ✓
- `FontHinting::Disabled` / `BackgroundColor` auto-required review notes are documentation-only (no code), correctly reflected by not inserting `BackgroundColor` and by leaving root-cause (a) intact. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases" — every code step shows complete code. ✓

**Type consistency:** `layout_preedit_cells(text, cell_w_logical, origin_x) -> (Vec<CellPlacement>, u32)` used identically in Task 2 (def) and Task 4 (call). `spawn_grapheme_cell(commands, ui_font, font_size, text, left, top, display) -> Entity` defined in Task 3, called in Task 3 (initial pool) and Task 4 (growth) with matching arity. `ImeGraphemePool(Vec<Entity>)`, `ImeGraphemeCell`, `ImeUnderline`, `INITIAL_POOL_CAP`, `set_node_display`, `set_node_rect` referenced consistently. ✓
