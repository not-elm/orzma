//! IME preedit overlay.
//!
//! Provides `compute_overlay_pos` (the pure pixel-math function for
//! anchoring the overlay), `ImeOverlayPlugin` (Bevy plugin that spawns
//! the overlay entity tree at Startup and schedules
//! `position_ime_overlay`), and the marker components identifying the
//! root, pre-caret span, post-caret span, and caret bar.

use crate::font::TerminalUiFont;
use crate::input::ime::ImeState;
use crate::input::ime::resolve_focused_surface;
use bevy::app::{App, Plugin, PostUpdate, Startup};
use bevy::color::Color;
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::system::{Commands, Query, Res, ResMut};
use bevy::math::Vec2;
use bevy::prelude::default;
use bevy::text::{LineBreak, TextColor, TextFont, TextLayout};
use bevy::ui::widget::Text;
use bevy::ui::{
    BackgroundColor, BorderColor, ComputedNode, Display, GlobalZIndex, Node, PositionType,
    UiGlobalTransform, UiRect, UiSystems, Val,
};
use bevy::window::{PrimaryWindow, Window};
use ozma_terminal::KeyboardFocused;
use ozma_tty_renderer::CellMetrics;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::TerminalFontInitSet;
use ozma_tty_renderer::TerminalFontSize;
use ozma_tty_renderer::material::TerminalMaterialSystems;
use ozma_tty_renderer::prelude::TerminalGrid;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Bevy plugin that spawns the IME overlay entity tree at Startup and
/// schedules `position_ime_overlay` in PostUpdate.
pub struct ImeOverlayPlugin;

impl Plugin for ImeOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ImeGraphemePool>()
            .add_systems(
                Startup,
                spawn_ime_overlay_once.after(TerminalFontInitSet::InitCellMetrics),
            )
            // NOTE: must run BEFORE `UiSystems::Content`. Bevy's
            // `detect_text_needs_rerender::<Text>` and `measure_text_system`
            // run in `UiSystems::Content` (`bevy_ui-0.18.1/src/lib.rs:226-243`)
            // and that's where `ComputedTextBlock` is refreshed from
            // ECS `TextSpan`s (`bevy_text-0.18.1/src/pipeline.rs:245-272`).
            // If we mutate `TextSpan.0` after `Content`, detection sees no
            // change this frame, `text_system` in `PostLayout` is gated off
            // (`bevy_ui-0.18.1/src/widget/text.rs:343-393`), the root's
            // `ComputedNode` stays at 0×0, and `bevy_ui_render` skips empty
            // nodes (`bevy_ui_render-0.18.1/src/lib.rs:1044-1046`) —
            // explaining why the inline preedit was invisible.
            //
            // Side effect of running before `Layout`: the anchor's
            // `UiGlobalTransform` and `ComputedNode.size` we read here are
            // from the PRIOR frame's `PostLayout`. For a stable terminal
            // pane (no per-frame resize) this is invisible; under window
            // drag it lags overlay placement by one frame, which is also
            // the same cosmetic latency the caret-bar `left` calc already
            // had.
            .add_systems(
                PostUpdate,
                (
                    position_ime_overlay.before(UiSystems::Content),
                    suppress_terminal_cursor_during_ime
                        .before(TerminalMaterialSystems::UpdateMaterial),
                ),
            );
    }
}

/// Marker for the singleton IME preedit overlay root entity.
#[derive(Component)]
pub struct ImeOverlayNode;

/// Marker for the 1-px `Node` that draws the caret bar. Spawned as a
/// top-level UI entity (NOT a child of [`ImeOverlayNode`]) so the
/// `Text` root stays a taffy leaf — required for `NodeMeasure::Text`
/// to drive its `ComputedNode.size`.
#[derive(Component)]
pub struct ImeCaretBar;

/// Marker for the hollow-block `Node` that highlights the macOS-IME
/// clause-selection range. Spawned as a top-level UI entity (NOT a
/// child of [`ImeOverlayNode`] — same constraint as [`ImeCaretBar`]
/// per the parent's leaf-only measure requirement).
///
/// Visible only when the composition's caret range has `begin != end`;
/// in that state, [`ImeCaretBar`] is hidden. The two markers are
/// mutually exclusive visually.
#[derive(Component)]
pub struct ImeClauseHighlight;

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

/// Computes the overlay's top-left logical-pixel position relative to
/// the window origin. Caller is responsible for writing this into
/// `Node.left` / `Node.top`.
///
/// All metric inputs are physical px; the function does the
/// physical→logical conversion via `scale`.
///
/// Layout: the overlay sits **at the cursor row**, matching Alacritty.
/// The composed glyph overlays the terminal-rendered cursor cell for
/// the duration of composition; this is the conventional placement
/// users expect from a terminal IME. Clamps:
///   - right: if `cell_origin_x + measured_width > host_right`,
///     shifts left so the right edge stays inside the host rect.
///   - left: after the right-edge clamp, ensures `left >= host_left`
///     so a very wide composition can't escape the left side of the
///     pane.
pub(crate) fn compute_overlay_pos(
    ui_global_translation_phys: Vec2,
    host_size_phys: Vec2,
    cursor_cell: (u16, u16),
    metrics: &CellMetrics,
    measured_width_logical: f32,
    scale: f32,
) -> Vec2 {
    // NOTE: `UiGlobalTransform.translation` is the CENTER of the
    // node in PHYSICAL pixels (verified via Bevy 0.18 source:
    // `bevy_ui-0.18.1/src/layout/mod.rs:239-275`). To get the
    // top-left we subtract `0.5 * host_size_phys`. We do NOT
    // multiply by `scale` — translation is already physical. The
    // earlier draft treated `translation` as logical-px top-left,
    // producing an offset of ~(host_w/2, host_h/2) at scale=1
    // (visible in bug1.png) and a compounding unit error at DPR>1.
    let cell_w_phys = metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.line_height_phys.floor().max(1.0);
    let host_top_left_phys = ui_global_translation_phys - 0.5 * host_size_phys;
    let cell_origin_phys = host_top_left_phys
        + Vec2::new(
            cursor_cell.0 as f32 * cell_w_phys,
            cursor_cell.1 as f32 * cell_h_phys,
        );
    let pos_logical = cell_origin_phys / scale;

    let host_top_left_logical = host_top_left_phys / scale;
    let host_size_logical = host_size_phys / scale;
    let host_left = host_top_left_logical.x;
    let host_right = host_left + host_size_logical.x;
    let mut left = pos_logical.x;
    if left + measured_width_logical > host_right {
        left = host_right - measured_width_logical;
    }
    if left < host_left {
        left = host_left;
    }

    Vec2::new(left, pos_logical.y)
}

/// Returns `(begin_cells, end_cells)` — the per-side cell offsets of
/// the IME caret/clause range relative to the start of `text`. Uses
/// `unicode_width::UnicodeWidthStr::width` so fullwidth CJK
/// preedit counts as 2 cells per glyph, matching the renderer's own
/// width logic in `ozma_tty_renderer::grid`.
///
/// Caller is responsible for byte-offset validity (UTF-8 boundary,
/// `begin <= end <= text.len()`); `Composition::try_new` enforces these.
fn caret_cell_offsets(text: &str, (begin, end): (usize, usize)) -> (f32, f32) {
    let begin_cells = UnicodeWidthStr::width(&text[..begin]) as f32;
    let end_cells = UnicodeWidthStr::width(&text[..end]) as f32;
    (begin_cells, end_cells)
}

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
fn layout_preedit_cells(
    text: &str,
    cell_w_logical: f32,
    origin_x: f32,
) -> (Vec<CellPlacement>, u32) {
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

    // NOTE: caret/clause/cells are hidden upfront each frame so they don't leak
    // past a commit, cancel, or focus loss. bg and underline are intentionally
    // excluded here: they are shown whenever composition is active, so pre-hiding
    // them would toggle display on every stable-composition frame and mark them
    // Changed unconditionally. Instead they are hidden in each early-return path.
    for entity in [caret_entity, clause_entity].into_iter().flatten() {
        set_node_display(&mut nodes, entity, Display::None);
    }
    for index in 0..pool.0.len() {
        set_node_display(&mut nodes, pool.0[index], Display::None);
    }

    let Some(comp) = state.composition() else {
        set_node_display(&mut nodes, bg_entity, Display::None);
        if let Some(e) = underline_entity {
            set_node_display(&mut nodes, e, Display::None);
        }
        return;
    };
    let Some(entity) = resolve_focused_surface(&focused) else {
        set_node_display(&mut nodes, bg_entity, Display::None);
        if let Some(e) = underline_entity {
            set_node_display(&mut nodes, e, Display::None);
        }
        return;
    };
    let Ok((node, ui_xform, grid)) = anchors.get(entity) else {
        set_node_display(&mut nodes, bg_entity, Display::None);
        if let Some(e) = underline_entity {
            set_node_display(&mut nodes, e, Display::None);
        }
        return;
    };
    let Ok(window) = primary_window.single() else {
        set_node_display(&mut nodes, bg_entity, Display::None);
        if let Some(e) = underline_entity {
            set_node_display(&mut nodes, e, Display::None);
        }
        return;
    };

    let scale = window.resolution.scale_factor();
    let cursor_cell = grid.cursor.as_ref().map(|c| (c.x, c.y)).unwrap_or((0, 0));
    let cell_w_logical = metrics.metrics.advance_phys.floor().max(1.0) / scale;
    let line_h_logical = metrics.metrics.line_height_phys.floor().max(1.0) / scale;

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

    set_node_rect(
        &mut nodes,
        bg_entity,
        pos.x,
        pos.y,
        total_width_logical,
        line_h_logical,
    );
    set_node_display(&mut nodes, bg_entity, Display::Flex);
    if let Ok(mut bg) = overlay_bg.single_mut() {
        let occluding = Color::srgb_u8(grid.default_bg[0], grid.default_bg[1], grid.default_bg[2]);
        if bg.0 != occluding {
            bg.0 = occluding;
        }
    }

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

    // NOTE: `underline_position_phys` is baseline-relative and negative; subtract it from ascent so the bar lands below the baseline, not above the cell top.
    if let Some(underline_entity) = underline_entity {
        let underline_top =
            pos.y + (metrics.metrics.ascent_phys - metrics.metrics.underline_position_phys) / scale;
        let underline_h = (metrics.metrics.underline_thickness_phys / scale).max(1.0);
        set_node_rect(
            &mut nodes,
            underline_entity,
            pos.x,
            underline_top,
            total_width_logical,
            underline_h,
        );
        set_node_display(&mut nodes, underline_entity, Display::Flex);
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
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
    }

    if has_clause && let Some(clause_entity) = clause_entity {
        set_node_rect(
            &mut nodes,
            clause_entity,
            pos.x + begin_cells * cell_w_logical,
            pos.y,
            (end_cells - begin_cells) * cell_w_logical,
            line_h_logical,
        );
        set_node_display(&mut nodes, clause_entity, Display::Flex);
    }
}

/// Sets `TerminalGrid.suppress_cursor = true` on the keyboard-focused
/// terminal surface while IME composition is active; clears it on all
/// other grids. Runs in `PostUpdate.before(TerminalMaterialSystems::UpdateMaterial)`
/// so the override takes effect in the same frame the IME caret
/// appears (and clears the same frame composition ends).
///
/// If there is no keyboard-focused surface while composition is active (e.g., a
/// race window between focus loss and `Ime::Disabled`), every grid gets
/// `suppress_cursor = false`. The safe default is "show cursors" rather
/// than blanket-hide.
fn suppress_terminal_cursor_during_ime(
    state: Res<ImeState>,
    focused: Query<Entity, With<KeyboardFocused>>,
    mut grids: Query<(Entity, &mut TerminalGrid)>,
) {
    let focused_surface = if state.is_composing() {
        resolve_focused_surface(&focused)
    } else {
        None
    };
    for (entity, mut grid) in &mut grids {
        let want = Some(entity) == focused_surface;
        if grid.suppress_cursor != want {
            grid.suppress_cursor = want;
        }
    }
}

/// Global z-index for the IME overlay; placed high enough to float
/// above all other UI nodes.
const IME_OVERLAY_Z: i32 = 200;

/// Spawns the overlay entity tree.
///
/// NOTE: `Text` root and caret bar are spawned as INDEPENDENT
/// top-level UI entities (no `ChildOf` between them). Required by
/// Bevy 0.18 + Taffy 0.9.2: `NodeMeasure::Text` is only consulted by
/// `compute_leaf_layout`, dispatched at `(_, has_children == false)`
/// in `taffy-0.9.2/src/tree/taffy_tree.rs:364-394`. Adding any UI
/// child (even an absolute-positioned one that contributes 0 to flex
/// container size) puts the Text node on the `compute_flexbox_layout`
/// branch, which ignores the measure function — `ComputedNode.size`
/// becomes (0, 0), `bevy_ui_render` then skips text + underline at
/// `uinode.is_empty()` (`bevy_ui_render-0.18.1/src/lib.rs:939, 1197`),
/// while the child caret bar still renders because its own
/// `ComputedNode` is non-empty (explicit width/height). Both
/// entities are positioned in window-absolute coords each frame in
/// `position_ime_overlay`.
///
/// `LineBreak::NoWrap` is set as defense-in-depth: with it,
/// `measure_text_system` uses `FixedMeasure { size: measure.max }`
/// (`bevy_ui-0.18.1/src/widget/text.rs:292-295`) and `text_system`
/// uses `TextBounds::UNBOUNDED` (`:351-353`) — bypassing any residual
/// zero-bound shaping. Single-line preedit text never needs wrapping
/// anyway.
///
/// TODO: bind TextColor / UnderlineColor / BackgroundColor to theme
/// tokens (`text-foreground` / `bg-background`) once the theme-token
/// helper is integrated. Placeholder white for now.
fn spawn_ime_overlay_once(
    mut commands: Commands,
    mut pool: ResMut<ImeGraphemePool>,
    ui_font: Res<TerminalUiFont>,
    font_size: Res<TerminalFontSize>,
) {
    let color = Color::WHITE;

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

    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            width: Val::Px(1.0),
            height: Val::Px(16.0), // TODO: refined at runtime from CellMetrics
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            ..default()
        },
        BackgroundColor(color),
        GlobalZIndex(IME_OVERLAY_Z),
        ImeCaretBar,
    ));

    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            border: UiRect::all(Val::Px(1.0)),
            width: Val::Px(0.0),
            height: Val::Px(16.0), // TODO: refined at runtime from CellMetrics
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            ..default()
        },
        BorderColor::all(color),
        GlobalZIndex(IME_OVERLAY_Z),
        ImeClauseHighlight,
    ));

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
}

/// Sets `node.display` on `entity` only when it differs, so change detection
/// fires only on a real change.
fn set_node_display(nodes: &mut Query<&mut Node>, entity: Entity, display: Display) {
    if let Ok(mut node) = nodes.get_mut(entity)
        && node.display != display
    {
        node.display = display;
    }
}

/// Writes `left`/`top`/`width`/`height` (logical px) into `entity`'s `Node`,
/// each guarded by an equality check so change detection fires only on a real
/// change. Resolving the node inside the helper (rather than accepting an
/// already-dereferenced `&mut Node`) keeps the `DerefMut` — and thus the change
/// tick — from firing on an unchanged frame.
fn set_node_rect(
    nodes: &mut Query<&mut Node>,
    entity: Entity,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
) {
    let Ok(mut node) = nodes.get_mut(entity) else {
        return;
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::ime::apply_event;
    use bevy::app::App;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::MinimalPlugins;
    use bevy::window::Ime;
    use ozma_terminal::KeyboardFocused;
    use ozmux_tmux::PaneId;
    use ozmux_tmux::{ActivePane, TmuxPane};
    use tmux_control_parser::CellDims;

    #[test]
    fn suppresses_cursor_on_active_tmux_pane_while_composing() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let mut state = ImeState::default();
        apply_event(
            &mut state,
            &Ime::Preedit {
                window: Entity::from_bits(1),
                value: "こんに".into(),
                cursor: Some((3, 3)),
            },
        );
        assert!(state.is_composing(), "preedit must set the composition");
        app.insert_resource(state);

        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 0,
                        height: 0,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                ActivePane,
                KeyboardFocused,
                TerminalGrid::default(),
            ))
            .id();

        app.world_mut()
            .run_system_once(suppress_terminal_cursor_during_ime)
            .unwrap();

        let grid = app.world().get::<TerminalGrid>(pane).expect("grid");
        assert!(
            grid.suppress_cursor,
            "the active tmux pane must suppress its cursor while composing"
        );
    }

    #[test]
    fn suppresses_cursor_on_focused_terminal_without_tmux() {
        use ozma_terminal::OzmaTerminal;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let mut state = ImeState::default();
        apply_event(
            &mut state,
            &Ime::Preedit {
                window: bevy::ecs::entity::Entity::PLACEHOLDER,
                value: "あ".into(),
                cursor: Some((3, 3)),
            },
        );
        app.insert_resource(state);

        let focused = app
            .world_mut()
            .spawn((OzmaTerminal, KeyboardFocused, TerminalGrid::default()))
            .id();
        let other = app.world_mut().spawn(TerminalGrid::default()).id();

        app.world_mut()
            .run_system_once(suppress_terminal_cursor_during_ime)
            .unwrap();

        assert!(
            app.world()
                .get::<TerminalGrid>(focused)
                .unwrap()
                .suppress_cursor,
            "the focused terminal must suppress its cursor while composing"
        );
        assert!(
            !app.world()
                .get::<TerminalGrid>(other)
                .unwrap()
                .suppress_cursor,
            "an unfocused terminal must not suppress its cursor"
        );
    }

    /// Builds a `CellMetrics` literal for tests. `CellMetrics` does not
    /// derive `Default`, so callers must provide every field; this
    /// helper takes the two that `compute_overlay_pos` actually reads
    /// (`advance_phys`, `line_height_phys`) and fills the rest with
    /// arbitrary non-zero values that don't affect the function under
    /// test.
    fn metrics(advance: f32, line_height: f32) -> CellMetrics {
        CellMetrics {
            advance_phys: advance,
            line_height_phys: line_height,
            ascent_phys: 12.0,
            descent_phys: 4.0,
            underline_position_phys: -2.0,
            underline_thickness_phys: 1.0,
            max_overflow_phys: 0.0,
        }
    }

    /// Builds the inputs `compute_overlay_pos` expects from a more
    /// intuitive `(top_left_logical, size_logical, scale)` spec.
    /// `UiGlobalTransform.translation` is in physical px and points
    /// at the node's CENTER; `ComputedNode.size` is in physical px.
    /// Tests express their setup in logical-px top-left because
    /// that's how a reader thinks about pane geometry; this helper
    /// does the conversion.
    fn host_inputs(top_left_logical: Vec2, size_logical: Vec2, scale: f32) -> (Vec2, Vec2) {
        let size_phys = size_logical * scale;
        let top_left_phys = top_left_logical * scale;
        let center_phys = top_left_phys + 0.5 * size_phys;
        (center_phys, size_phys)
    }

    #[test]
    fn places_overlay_at_cursor_row() {
        let (translation_phys, size_phys) = host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (3, 5),
            &metrics(10.0, 16.0),
            0.0,
            1.0,
        );
        // y = row 5 × 16 = 80
        assert_eq!(pos.y, 80.0);
        // x = col 3 × 10 = 30, no clamp
        assert_eq!(pos.x, 30.0);
    }

    #[test]
    fn divides_by_scale_factor_for_logical_px() {
        // Logical top-left (100, 0), logical size 800×600, scale 2.0.
        // At scale 2.0: physical size = (1600, 1200), physical
        // top-left = (200, 0), physical center = (1000, 600).
        // Cursor (0, 0) at cursor row → cell_origin_phys = (200, 0) →
        // pos_logical = (100, 0).
        let (translation_phys, size_phys) =
            host_inputs(Vec2::new(100.0, 0.0), Vec2::new(800.0, 600.0), 2.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (0, 0),
            &metrics(10.0, 16.0),
            0.0,
            2.0,
        );
        assert_eq!(pos.x, 100.0);
        assert_eq!(pos.y, 0.0);
    }

    #[test]
    fn floors_subpixel_cell_pitch() {
        // advance 10.4 → floor 10; col 10 → x = 100
        // line_height 16.4 → floor 16; cursor row 1 → y = 1 × 16 = 16
        let (translation_phys, size_phys) = host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (10, 1),
            &metrics(10.4, 16.4),
            0.0,
            1.0,
        );
        assert_eq!(pos.x, 100.0);
        assert_eq!(pos.y, 16.0);
    }

    #[test]
    fn clamps_right_when_overlay_overflows() {
        // Cursor at col 78, cell width 10 → cell_origin x = 780.
        // Measured width 100 → would extend to 880, host right = 800.
        // Shift left by 80 → left = 700.
        let (translation_phys, size_phys) = host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (78, 0),
            &metrics(10.0, 16.0),
            100.0,
            1.0,
        );
        assert_eq!(pos.x, 700.0);
    }

    #[test]
    fn clamps_left_when_composition_too_wide_to_fit() {
        // host_size 80 (very narrow), measured 200, cursor at col 7 →
        // cell_origin x = 70, would overflow right → shift to
        // host_right - measured = 80 - 200 = -120, then left clamp →
        // 0 (host_left).
        let (translation_phys, size_phys) = host_inputs(Vec2::ZERO, Vec2::new(80.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (7, 0),
            &metrics(10.0, 16.0),
            200.0,
            1.0,
        );
        assert_eq!(pos.x, 0.0);
    }

    #[test]
    fn caret_cell_offsets_ascii_caret_at_start() {
        assert_eq!(caret_cell_offsets("hello", (0, 0)), (0.0, 0.0));
    }

    #[test]
    fn caret_cell_offsets_ascii_caret_at_end() {
        assert_eq!(caret_cell_offsets("hello", (5, 5)), (5.0, 5.0));
    }

    #[test]
    fn caret_cell_offsets_ascii_clause_range() {
        // begin=2 ("he|llo"), end=4 ("hel|lo"). Width 2 cells.
        assert_eq!(caret_cell_offsets("hello", (2, 4)), (2.0, 4.0));
    }

    #[test]
    fn caret_cell_offsets_cjk_fullwidth() {
        // "にほん" is 3 hiragana × 3 bytes each = 9 bytes total;
        // each hiragana takes 2 monospace cells. begin=0, end=9 →
        // begin_cells=0, end_cells=6.
        assert_eq!(caret_cell_offsets("にほん", (0, 9)), (0.0, 6.0));
    }

    #[test]
    fn caret_cell_offsets_mixed_ascii_and_cjk() {
        // "a" (1 byte, 1 cell) + "あ" (3 bytes, 2 cells) = 4 bytes, 3 cells.
        // begin=0, end=4 → (0.0, 3.0). begin=1 (after "a"), end=4 → (1.0, 3.0).
        assert_eq!(caret_cell_offsets("aあ", (0, 4)), (0.0, 3.0));
        assert_eq!(caret_cell_offsets("aあ", (1, 4)), (1.0, 3.0));
    }

    #[test]
    fn ime_overlay_uses_terminal_font_size() {
        use bevy::asset::Handle;
        use bevy::text::TextFont;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont(Handle::default()));
        app.insert_resource(TerminalFontSize(9.0));
        app.init_resource::<ImeGraphemePool>();
        app.add_systems(Startup, spawn_ime_overlay_once);
        app.update();

        let mut query = app.world_mut().query::<&TextFont>();
        let matched = query
            .iter(app.world())
            .any(|tf| (tf.font_size - 9.0).abs() < f32::EPSILON);
        assert!(
            matched,
            "IME overlay TextFont must use TerminalFontSize (9.0), not the constant"
        );
    }

    #[test]
    fn overlay_background_matches_pane_default_bg_while_composing() {
        use bevy::asset::Handle;
        use bevy::window::WindowResolution;
        use ozma_terminal::OzmaTerminal;
        use ozma_tty_renderer::prelude::Cursor;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont(Handle::default()));
        app.insert_resource(TerminalFontSize(12.0));
        app.insert_resource(TerminalCellMetricsResource {
            metrics: metrics(8.0, 16.0),
            phys_font_size: 12,
        });

        let mut state = ImeState::default();
        apply_event(
            &mut state,
            &Ime::Preedit {
                window: Entity::PLACEHOLDER,
                value: "あ".into(),
                cursor: Some((3, 3)),
            },
        );
        app.insert_resource(state);

        app.init_resource::<ImeGraphemePool>();
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
                default_bg: [10, 20, 30],
                ..TerminalGrid::default()
            },
        ));

        app.update();
        app.world_mut()
            .run_system_once(position_ime_overlay)
            .unwrap();

        let mut overlays = app
            .world_mut()
            .query_filtered::<Entity, With<ImeOverlayNode>>();
        let overlay = overlays.single(app.world()).expect("overlay entity");

        let bg = app
            .world()
            .get::<BackgroundColor>(overlay)
            .expect("overlay must carry an opaque BackgroundColor while composing");
        assert_eq!(
            bg.0,
            Color::srgb_u8(10, 20, 30),
            "overlay background must match the focused pane's default_bg so it occludes the underlying line",
        );
        assert_eq!(
            app.world().get::<Node>(overlay).unwrap().display,
            Display::Flex,
            "overlay must be shown while composing",
        );
    }

    #[test]
    fn host_translated_to_window_offset_does_not_leak_into_cell_origin() {
        // Regression guard for the bug visible in bug1.png: prior
        // implementation treated `translation` as top-left, so a
        // host centered in a 1265×720 window would push the cell
        // origin by (host_w/2, host_h/2). With the fix, the cell
        // origin must be the host's top-left + cursor offset, no
        // matter where the host sits in the window.
        //
        // Host top-left at logical (10, 20), size 1200×640, cursor (5, 3),
        // metrics 10×16, scale 1.0.
        // Expected: pos = (10 + 5*10, 20 + 3*16) = (60, 68).
        let (translation_phys, size_phys) =
            host_inputs(Vec2::new(10.0, 20.0), Vec2::new(1200.0, 640.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (5, 3),
            &metrics(10.0, 16.0),
            0.0,
            1.0,
        );
        assert_eq!(pos.x, 60.0);
        assert_eq!(pos.y, 68.0);
    }

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
        let mut underlines = app
            .world_mut()
            .query_filtered::<Entity, With<ImeUnderline>>();
        assert_eq!(
            underlines.iter(app.world()).count(),
            1,
            "exactly one underline bar must be spawned"
        );
    }

    #[test]
    fn overlay_geometry_not_rechanged_on_unchanged_composition() {
        use bevy::app::Update;
        use bevy::asset::Handle;
        use bevy::ecs::query::{Changed, Or};
        use bevy::window::WindowResolution;
        use ozma_terminal::OzmaTerminal;
        use ozma_tty_renderer::prelude::Cursor;

        #[derive(Resource, Default)]
        struct ChangedOverlayNodes(usize);

        fn count_changed(
            mut count: ResMut<ChangedOverlayNodes>,
            changed: Query<
                Entity,
                (
                    Changed<Node>,
                    Or<(With<ImeOverlayNode>, With<ImeUnderline>)>,
                ),
            >,
        ) {
            count.0 = changed.iter().count();
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont(Handle::default()));
        app.insert_resource(TerminalFontSize(12.0));
        app.insert_resource(TerminalCellMetricsResource {
            metrics: metrics(10.0, 16.0),
            phys_font_size: 12,
        });
        app.init_resource::<ImeGraphemePool>();
        app.init_resource::<ChangedOverlayNodes>();

        let mut state = ImeState::default();
        apply_event(
            &mut state,
            &Ime::Preedit {
                window: Entity::PLACEHOLDER,
                value: "abc".into(),
                cursor: Some((3, 3)),
            },
        );
        app.insert_resource(state);

        app.add_systems(Startup, spawn_ime_overlay_once);
        app.add_systems(Update, (position_ime_overlay, count_changed).chain());
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

        app.update(); // frame 1: spawn + initial positioning (legitimately Changed)
        app.update(); // frame 2: identical composition → no real geometry change

        assert_eq!(
            app.world().resource::<ChangedOverlayNodes>().0,
            0,
            "background/underline Nodes must not be re-marked Changed on an unchanged composition",
        );
    }
}
