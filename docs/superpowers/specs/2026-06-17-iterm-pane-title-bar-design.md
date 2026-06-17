# Per-Pane Title Bar Design

Date: 2026-06-17  
Branch: `iterm-pane`

## Problem

ozmux clips terminal content to the top of each pane node. When a window height is not
an exact multiple of `cell_h`, or when a top-bottom split divides rows unevenly, the
*last* pane in the vertical chain is stretched to fill the remaining pixels. The extra
pixels at the bottom of that pane render as `bg_padding_color` (terminal background),
producing a visible blank strip. In practice the strip is `H_ws mod cell_h` pixels tall
and is especially noticeable on high-DPI displays.

## Goal

Add a per-pane title bar (≈ one `cell_h` tall) at the top of every pane, displaying the
pane's `TerminalTitle`. The title bar absorbs the height that would otherwise be blank
space, gives each pane a clear identity, and matches iTerm2's visual convention.

## Research Summary (iTerm2)

iTerm2 uses a fixed-22pt `SessionTitleView` placed above each pane's scroll view. The
height is **not** dynamically resized to absorb pixel remainders; the blank space still
exists at the bottom of each pane's scroll view. What makes it invisible is that it blends
with the terminal background color. The key user-visible benefit is the consistent per-pane
identity and the visual "frame" that a title bar provides.

For ozmux we go one step further: we reduce the rows we report to tmux so the terminal
content exactly fills the area below the title bar (no content clipping, no extra blank strip).

## Architecture

### Entity Tree (After)

```
TmuxWindow (Absolute, in WorkspaceUiRoot)
  └── TmuxPane (Absolute, Column flex, height = T + dims.height × cell_h)
        ├── PaneTitleBar (flex child, height = T = cell_h)
        │     └── Text  ← TerminalTitle value
        └── TerminalRenderChild (flex_grow = 1, height = dims.height × cell_h)
              TerminalGrid + MaterialNode<TerminalUiMaterial>
```

Before this change, `TerminalRenderBundle` (which bundles `TerminalGrid` and
`MaterialNode<TerminalUiMaterial>`) was inserted directly on `TmuxPane`. After the
change, `TmuxPane` is a Column-flex container; `TerminalGrid` and `MaterialNode` move to
the `TerminalRenderChild` entity.

**Why both must be on the same entity**: `update_terminal_material` (in
`ozma_tty_renderer`) queries `MaterialNode<TerminalUiMaterial>` and `TerminalGrid`
together on the same entity. `apply_snapshot`/`apply_delta` observers look up
`TerminalGrid` via the entity ID passed to `flush_emit`. Splitting them across entities
would require query restructuring in the renderer crate.

**Components that stay on `TmuxPane`**: `TmuxPane`, `TerminalHandle`, `TerminalTitle`,
`Node` (container), `Outline`, `Button`, `FocusPolicy`, `PaneDim`, and a new
`TerminalRenderRef(Entity)` pointing to the `TerminalRenderChild`.

**`TerminalRenderRef`**: A new marker component added to `TmuxPane` after child
entities are spawned:
```rust
#[derive(Component)]
pub(crate) struct TerminalRenderRef(pub Entity);
```

`route_tmux_output` uses `TerminalRenderRef` to call `flush_emit` with the
`TerminalRenderChild` entity (not `TmuxPane`), so `apply_snapshot`/`apply_delta`
correctly find `TerminalGrid` on the child.

### Height Budget

For a vertical chain of `N_vert` panes (= `vertical_depth` of the active window layout):

```
H_ws  =  N_vert × T  +  sum(dims_i.height × cell_h)  +  (N_vert - 1) × gap_px
```

Where:
- `H_ws` = workspace height (window height minus window bar height)
- `T` = `cell_h` (one terminal row, converted from physical to logical pixels)
- `gap_px` = `PANE_GAP_PX` = 1.0 logical px (the 1-px inter-pane divider)

`sync_client_size` sends tmux:

```
rows_sent = rows_for_panes(total_rows) - vertical_depth(active_layout)
```

This ensures `sum(dims_i.height)` from tmux exactly fills `H_ws - N_vert × T` (up to the
1-px gap rounding).

### `vertical_depth` Definition

```
vertical_depth(Leaf)             = 1
vertical_depth(LeftRight split)  = max(vertical_depth(child) for each child)
vertical_depth(TopBottom split)  = sum(vertical_depth(child) for each child)
vertical_depth(Floating split)   = 1
```

This measures the maximum number of stacked title bars along the tallest vertical path
in the layout tree.

### `collapse()` Changes

`pane_title_h` is computed from `TerminalCellMetricsResource.line_height_phys` (the physical
cell height) divided by the window DPR to get logical pixels — the same source as
`bar_height_px()` in `tmux_window_bar.rs`. `collapse()` gains a `pane_title_h: f32`
parameter. For a `Leaf` node, the pane rectangle height is extended by `pane_title_h`:

```
node_size.y = dims.height × cell_h + pane_title_h
```

`PackedTmuxLayout` does **not** grow a `title_bars` map — the title bar is a Column-flex
child with a fixed `Val::Px(T)` height, so Bevy's layout engine positions it automatically
without ozmux needing to track its rect separately.

### `layout_tmux_panes` Changes

For each `TmuxPane` child of a `TmuxWindow`:

1. Read `pane_rect` from `PackedTmuxLayout.panes` (now includes title bar height).
2. Set `TmuxPane` container Node: Absolute, `left/top/width` unchanged, `height = pane_rect.height()` (= dims.height × cell_h + T). Bevy flex layout automatically sizes:
   - `PaneTitleBar` child to `height = T`
   - `TerminalRenderChild` child to `height = pane_rect.height() - T = dims.height × cell_h`
3. Resize the terminal handle to `(dims.width, dims.height)` as before — tmux now reports
   the reduced row count that exactly fills the terminal child's height.

### `attach_tmux_pane_terminal` Changes

Instead of inserting `TerminalRenderBundle` on `TmuxPane`, this function spawns two
child entities and adds `TerminalRenderRef` to `TmuxPane`:

```rust
// 1. Change TmuxPane Node to Column flex (title bar stacks on top of terminal)
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

// 2. PaneTitleBar child
let title_bar = commands.spawn((
    PaneTitleBar,
    Node { width: Val::Percent(100.0), ..default() },
    BackgroundColor(theme::PANEL),
    ChildOf(entity),
)).id();
commands.spawn((
    Text::new(""),
    TextColor(theme::FOREGROUND),
    TextFont { font_size: theme::UI_FONT_SIZE, ..default() },
    ChildOf(title_bar),
));

// 3. TerminalRenderChild (TerminalGrid + MaterialNode here, not on TmuxPane)
let render_child = commands.spawn((
    TerminalRenderBundle::new(material),
    Node { flex_grow: 1.0, width: Val::Percent(100.0), ..default() },
    ChildOf(entity),
)).id();

commands.entity(entity).insert(TerminalRenderRef(render_child));
```

The `TmuxPane` entity keeps: `TmuxPane`, `TerminalHandle`, `TerminalTitle`, `TerminalRenderRef`,
`Node` (container, Column flex), `Outline`.

The `TerminalRenderChild` entity has: `TerminalGrid`, `MaterialNode<TerminalUiMaterial>`,
`TerminalMaterialState` (auto-inserted by hook), `PaneDim` (brightness), `Node` (flex_grow).

**`PaneDim` migration**: `update_terminal_material` in `ozma_tty_renderer` queries `PaneDim`
alongside `MaterialNode` on the same entity. Since `MaterialNode` moves to `TerminalRenderChild`,
`PaneDim` must also move there. The `sync_pane_dim` system in `tmux_pane_focus.rs` currently
inserts `PaneDim` on `TmuxPane` entities — it must be updated to insert `PaneDim` on the
`TerminalRenderChild` entity instead (looked up via `TerminalRenderRef`).

### New Plugin: `OzmuxTmuxPaneTitlePlugin`

Located in `src/ui/tmux_pane_title.rs`. Registered in `main.rs`.

Systems:

| System | Trigger | Action |
|---|---|---|
| `sync_pane_title_text` | `TerminalTitle` changed | Update `Text` on the `PaneTitleBar` child |
| `sync_pane_title_active` | `ActivePane` added/removed | Recolor `PaneTitleBar` background |

`sync_pane_title_active` uses the same change-detection pattern as `sync_active_pane_outline`:
recolor (don't insert/remove) to avoid ECS table moves.

Active pane title bar background: `theme::TAB_BAR_BG` with a thin top accent line
(drawn via `Outline` or a nested 2px `Node` with `BackgroundColor(theme::ACCENT)`).

Inactive pane title bar background: `theme::PANEL`.

Text padding: `theme::TAB_PADDING_X_PX` via `UiRect::axes(Val::Px(TAB_PADDING_X_PX), Val::Px(0.0))`.

### `sync_client_size` Changes

```rust
fn sync_client_size(
    mut last: ResMut<LastClientSize>,
    connection: NonSend<TmuxConnection>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
    active_layout: Query<&TmuxWindowLayout, With<ActiveWindow>>,  // NEW
) {
    // ... existing size computation ...
    let rows = rows_for_panes(rows);
    let depth = active_layout
        .single()
        .map(|l| vertical_depth(&l.0.root))
        .unwrap_or(1) as u16;
    let rows = rows.saturating_sub(depth);
    // ... send refresh_client_command(cols, rows) ...
}
```

On the first frame (no layout yet), `depth` defaults to 1 (single-pane assumption).
After tmux responds with a layout, `depth` is recomputed and a corrected size is sent
within one tmux round-trip.

## Visual Specification

```
┌─────────────────────────────────────┐
│ /Users/you/project  [PANEL bg]      │  ← PaneTitleBar, T = cell_h px
├─────────────────────────────────────┤
│                                     │
│  terminal content (dims.height rows)│
│                                     │
└─────────────────────────────────────┘
```

Active pane title bar: top 2px accent line (`theme::ACCENT`) + `theme::TAB_BAR_BG` fill.  
Inactive pane title bar: `theme::PANEL` fill, no accent line.  
Text: truncated with ellipsis when wider than the bar.  
Font: same `TerminalUiFont` as the window bar.

## Files Affected

| File | Change |
|---|---|
| `src/tmux_render.rs` | `collapse()` gains `pane_title_h` param, Leaf height += T; `layout_tmux_panes` sets container node size (children auto-sized by flex); `sync_client_size` reads `vertical_depth` from active layout; `attach_tmux_pane_terminal` spawns PaneTitleBar + TerminalRenderChild, inserts `TerminalRenderRef`; `route_tmux_output` calls `flush_emit` via `TerminalRenderRef` |
| `src/ui/tmux_pane_title.rs` | New file: `OzmuxTmuxPaneTitlePlugin`, `PaneTitleBar` component, `sync_pane_title_text`, `sync_pane_title_active` systems |
| `src/ui.rs` | Module declaration for `tmux_pane_title` |
| `src/main.rs` | Register `OzmuxTmuxPaneTitlePlugin` |
| `src/ui/tmux_pane_focus.rs` | `augment_tmux_pane`: add `FocusPolicy::Block` to `PaneTitleBar` child; `sync_pane_dim`: insert `PaneDim` on `TerminalRenderChild` (via `TerminalRenderRef`) instead of on `TmuxPane` |
| `src/tmux_render.rs` (tests) | Update `collapse` unit tests for `pane_title_h`; add `vertical_depth` unit tests |

## Edge Cases

| Scenario | Handling |
|---|---|
| Single pane, window height divisible by `cell_h` | `depth=1`, 1 row subtracted. Title bar T px, terminal fills rest exactly. |
| Single pane, window height not divisible | Same; last row of terminal area has `slack = H_ws mod cell_h` pixels below content — absorbed by `bg_padding_color` (terminal background), invisible. |
| Pure horizontal (LeftRight) split | `vertical_depth = 1`; only 1 row subtracted for all panes side by side. |
| Nested vertical split (TopBottom inside TopBottom) | `vertical_depth = sum` propagates correctly. |
| Floating pane | `vertical_depth = 1` for floating split; floating panes get title bar at their floating origin. |
| First frame (no layout yet) | `depth = 1` default; corrects after tmux round-trip. |
| Pane count change (split/kill) | `vertical_depth` recomputes next frame; 1-frame layout jitter acceptable. |

## Testing Plan

1. **Unit tests** in `src/tmux_render.rs`:
   - `vertical_depth` for Leaf, LeftRight, TopBottom, mixed trees.
   - `collapse` with `pane_title_h > 0`: verify rect heights include title bar height.
   - `layout_tmux_panes` integration test: verify title bar child node height = `T`,
     terminal child node height = `dims.height × cell_h`.

2. **Manual smoke tests**:
   - Single pane: title bar visible, terminal fills below.
   - 2-pane top-bottom split: both panes have equal-height title bars.
   - 2-pane left-right split: both panes have title bars.
   - 3-pane nested split: correct depth subtraction, all panes have title bars.
   - TerminalTitle updates live (change directory).
   - Active pane changes: accent line moves correctly.
