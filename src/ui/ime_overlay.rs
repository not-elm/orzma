//! IME preedit overlay.
//!
//! Provides `ImeOverlayPlugin` (Bevy plugin that spawns the overlay
//! entity tree at Startup and schedules `position_ime_overlay`) and the
//! marker components identifying the root, caret bar, clause highlight,
//! underline, and grapheme-cell pool.

mod layout;

use crate::font::TerminalUiFont;
use crate::input::focus::KeyboardFocused;
use crate::input::ime::ImeState;
use crate::input::ime::resolve_focused_surface;
use bevy::app::{App, Plugin, PostUpdate, Startup};
use bevy::color::Color;
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::schedule::SystemCondition;
use bevy::ecs::schedule::common_conditions::{not, resource_exists_and_changed};
use bevy::ecs::system::{Commands, Query, Res, ResMut};
use bevy::prelude::default;
use bevy::text::{FontSize, LineBreak, TextColor, TextLayout};
use bevy::ui::widget::Text;
use bevy::ui::{
    BackgroundColor, BorderColor, ComputedNode, Display, GlobalZIndex, Node, PositionType,
    UiGlobalTransform, UiRect, UiSystems, Val,
};
use bevy::window::{PrimaryWindow, Window};
use layout::{CaretVisual, PlacedCell, compute_overlay_layout};
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::TerminalFontInitSet;
use orzma_tty_renderer::TerminalFontSize;
use orzma_tty_renderer::material::TerminalMaterialSystems;
use orzma_tty_renderer::prelude::TerminalGrid;

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
            // `detect_text_needs_rerender` and `measure_text_system`
            // run in `UiSystems::Content` (`bevy_ui-0.19.0/src/lib.rs:236-252`)
            // and that's where changed ECS `TextSpan`s are detected
            // (`bevy_text-0.19.0/src/text.rs:1223`). If we mutate
            // `TextSpan.0` after `Content`, detection sees no change
            // this frame, `text_system` in `PostLayout` is gated off
            // (`bevy_ui-0.19.0/src/widget/text.rs:357`), the root's
            // `ComputedNode` stays at 0×0, and `bevy_ui_render` skips empty
            // nodes (`bevy_ui_render-0.19.0/src/lib.rs:997`) —
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
                    position_ime_overlay
                        .run_if(ime_is_composing)
                        .before(UiSystems::Content),
                    hide_ime_overlay
                        .run_if(
                            resource_exists_and_changed::<ImeState>.and_then(not(ime_is_composing)),
                        )
                        .before(UiSystems::Content),
                    suppress_terminal_cursor_during_ime
                        .run_if(resource_exists_and_changed::<ImeState>.or_else(ime_is_composing))
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

/// Run condition: true while an IME preedit composition is active. Takes
/// `Option<Res<ImeState>>` so it returns `false` rather than panicking when
/// `ImeState` is absent, keeping the `.or_else()` / `.and_then()` gates safe.
fn ime_is_composing(state: Option<Res<ImeState>>) -> bool {
    state.is_some_and(|state| state.is_composing())
}

/// PostUpdate system that grid-aligns the IME preedit overlay at the attached
/// terminal's cursor cell. Lays out the composition as one cell-anchored
/// `Text` node per grapheme cluster (pooled in [`ImeGraphemePool`], grown on
/// demand), draws an occluding background rect and a continuous underline bar,
/// and positions the caret beam (`begin == end`) or clause highlight
/// (`begin != end`). Every visible element uses the same cell arithmetic, so
/// the caret cannot drift from the text.
///
/// Gated by `run_if(ime_is_composing)`, so it runs only while a composition is
/// active; the end-of-composition hide (commit / cancel / `Ime::Disabled`) is
/// owned by [`hide_ime_overlay`]. If the focused surface, its anchor, or the
/// window is missing while composing, it hides every overlay part defensively
/// and returns.
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

    // NOTE: the end-of-composition hide (commit / cancel / disable) is owned by
    // `hide_ime_overlay` — `run_if(ime_is_composing)` means this system runs only
    // WHILE composing, so deleting `hide_ime_overlay` would leak the overlay past
    // commit. The guards below hide defensively on a showing-precondition failure
    // (missing focused surface / anchor / window); the `composition()` arm is an
    // unreachable fallback the gate already guarantees.
    let Some(comp) = state.composition() else {
        hide_all_overlay_parts(
            &mut nodes,
            bg_entity,
            underline_entity,
            caret_entity,
            clause_entity,
            &pool,
        );
        return;
    };
    let Some(entity) = resolve_focused_surface(&focused) else {
        hide_all_overlay_parts(
            &mut nodes,
            bg_entity,
            underline_entity,
            caret_entity,
            clause_entity,
            &pool,
        );
        return;
    };
    let Ok((node, ui_xform, grid)) = anchors.get(entity) else {
        hide_all_overlay_parts(
            &mut nodes,
            bg_entity,
            underline_entity,
            caret_entity,
            clause_entity,
            &pool,
        );
        return;
    };
    let Ok(window) = primary_window.single() else {
        hide_all_overlay_parts(
            &mut nodes,
            bg_entity,
            underline_entity,
            caret_entity,
            clause_entity,
            &pool,
        );
        return;
    };

    // NOTE: clamp the scale away from zero (matching `ime_policy_system`); a 0
    // scale factor would make every cell metric inf/NaN and fling the overlay
    // off-screen during composition.
    let scale = window.resolution.scale_factor().max(f32::EPSILON);
    let cursor_cell = grid.cursor.as_ref().map(|c| (c.x, c.y)).unwrap_or((0, 0));

    let layout = compute_overlay_layout(
        comp.text(),
        comp.caret(),
        ui_xform.translation,
        node.size,
        cursor_cell,
        &metrics.metrics,
        scale,
    );

    set_node_rect(
        &mut nodes,
        bg_entity,
        layout.background.left,
        layout.background.top,
        layout.background.width,
        layout.background.height,
    );
    set_node_display(&mut nodes, bg_entity, Display::Flex);
    if let Ok(mut bg) = overlay_bg.single_mut() {
        let occluding = Color::srgb_u8(grid.default_bg[0], grid.default_bg[1], grid.default_bg[2]);
        if bg.0 != occluding {
            bg.0 = occluding;
        }
    }

    apply_grapheme_cells(
        &mut commands,
        &mut nodes,
        &mut cell_texts,
        &mut pool,
        &ui_font,
        &font_size,
        &layout.cells,
    );

    if let Some(underline_entity) = underline_entity {
        set_node_rect(
            &mut nodes,
            underline_entity,
            layout.underline.left,
            layout.underline.top,
            layout.underline.width,
            layout.underline.height,
        );
        set_node_display(&mut nodes, underline_entity, Display::Flex);
    }

    apply_caret_visual(&mut nodes, caret_entity, clause_entity, &layout.caret);
}

/// Applies the placed grapheme cells to the pooled `Text` nodes: reuses pool
/// entries by index (equality-guarded writes), grows the pool for any overflow,
/// and hides the unused tail.
fn apply_grapheme_cells(
    commands: &mut Commands,
    nodes: &mut Query<&mut Node>,
    cell_texts: &mut Query<&mut Text, With<ImeGraphemeCell>>,
    pool: &mut ImeGraphemePool,
    ui_font: &TerminalUiFont,
    font_size: &TerminalFontSize,
    cells: &[PlacedCell],
) {
    for (index, cell) in cells.iter().enumerate() {
        if let Some(&entity) = pool.0.get(index) {
            if let Ok(mut node) = nodes.get_mut(entity) {
                let left = Val::Px(cell.left);
                if node.left != left {
                    node.left = left;
                }
                let top = Val::Px(cell.top);
                if node.top != top {
                    node.top = top;
                }
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
            }
            if let Ok(mut text) = cell_texts.get_mut(entity)
                && text.0 != cell.text
            {
                text.0 = cell.text.clone();
            }
        } else {
            // NOTE: grown entities are not in `nodes` / `cell_texts` this frame,
            // so they are spawned already configured; their tail appears one
            // frame late only on the growth frame.
            let entity = spawn_grapheme_cell(
                commands,
                ui_font,
                font_size,
                &cell.text,
                cell.left,
                cell.top,
                Display::Flex,
            );
            pool.0.push(entity);
        }
    }
    for index in cells.len()..pool.0.len() {
        set_node_display(nodes, pool.0[index], Display::None);
    }
}

/// Shows the caret beam or the clause highlight per `caret`, hiding the other.
/// Every write is equality-guarded so an unchanged caret marks nothing changed.
fn apply_caret_visual(
    nodes: &mut Query<&mut Node>,
    caret_entity: Option<Entity>,
    clause_entity: Option<Entity>,
    caret: &CaretVisual,
) {
    match caret {
        CaretVisual::Beam(beam) => {
            if let Some(caret_entity) = caret_entity
                && let Ok(mut node) = nodes.get_mut(caret_entity)
            {
                let left = Val::Px(beam.left);
                if node.left != left {
                    node.left = left;
                }
                let top = Val::Px(beam.top);
                if node.top != top {
                    node.top = top;
                }
                let height = Val::Px(beam.height);
                if node.height != height {
                    node.height = height;
                }
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
            }
            if let Some(clause_entity) = clause_entity {
                set_node_display(nodes, clause_entity, Display::None);
            }
        }
        CaretVisual::Clause(rect) => {
            if let Some(clause_entity) = clause_entity {
                set_node_rect(
                    nodes,
                    clause_entity,
                    rect.left,
                    rect.top,
                    rect.width,
                    rect.height,
                );
                set_node_display(nodes, clause_entity, Display::Flex);
            }
            if let Some(caret_entity) = caret_entity {
                set_node_display(nodes, caret_entity, Display::None);
            }
        }
        CaretVisual::None => {
            if let Some(caret_entity) = caret_entity {
                set_node_display(nodes, caret_entity, Display::None);
            }
            if let Some(clause_entity) = clause_entity {
                set_node_display(nodes, clause_entity, Display::None);
            }
        }
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

/// PostUpdate system that hides every IME overlay part. Gated to run only on
/// the frame composition ends (commit / cancel / `Ime::Disabled`); see
/// `ImeOverlayPlugin`. Shares `hide_all_overlay_parts` with
/// `position_ime_overlay`'s internal precondition guards.
fn hide_ime_overlay(
    mut nodes: Query<&mut Node>,
    pool: Res<ImeGraphemePool>,
    background: Query<Entity, With<ImeOverlayNode>>,
    underline: Query<Entity, With<ImeUnderline>>,
    caret: Query<Entity, With<ImeCaretBar>>,
    clause: Query<Entity, With<ImeClauseHighlight>>,
) {
    let Ok(bg_entity) = background.single() else {
        return;
    };
    hide_all_overlay_parts(
        &mut nodes,
        bg_entity,
        underline.single().ok(),
        caret.single().ok(),
        clause.single().ok(),
        &pool,
    );
}

/// Global z-index for the IME overlay; placed high enough to float
/// above all other UI nodes.
const IME_OVERLAY_Z: i32 = 200;

/// Z-index for the opaque occluding background rect — one below
/// [`IME_OVERLAY_Z`] so the preedit glyph cells, underline, caret, and clause
/// box (all at [`IME_OVERLAY_Z`]) always render in front of it, rather than
/// relying on Bevy's equal-z spawn-order tie-break (which entity reuse as the
/// pool grows/shrinks could otherwise flip, hiding the composition).
const IME_OVERLAY_BG_Z: i32 = IME_OVERLAY_Z - 1;

/// Spawns the overlay entity tree.
///
/// NOTE: `Text` root and caret bar are spawned as INDEPENDENT
/// top-level UI entities (no `ChildOf` between them). Required by
/// Bevy 0.19 + Taffy 0.10.1: `NodeMeasure::Text` is only consulted by
/// `compute_leaf_layout`, dispatched at `(_, has_children == false)`
/// in `taffy-0.10.1/src/tree/taffy_tree.rs:304-330`. Adding any UI
/// child (even an absolute-positioned one that contributes 0 to flex
/// container size) puts the Text node on the `compute_flexbox_layout`
/// branch, which ignores the measure function — `ComputedNode.size`
/// becomes (0, 0), `bevy_ui_render` then skips text + underline at
/// `uinode.is_empty()` (`bevy_ui_render-0.19.0/src/lib.rs:997, 1308`),
/// while the child caret bar still renders because its own
/// `ComputedNode` is non-empty (explicit width/height). Both
/// entities are positioned in window-absolute coords each frame in
/// `position_ime_overlay`.
///
/// `LineBreak::NoWrap` is set as defense-in-depth: with it,
/// `measure_text_system` uses `FixedMeasure { size: measure.max }`
/// (`bevy_ui-0.19.0/src/widget/text.rs:301-302`) and `text_system`
/// uses `TextBounds::UNBOUNDED` (`:363-365`) — bypassing any residual
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
        GlobalZIndex(IME_OVERLAY_BG_Z),
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

/// Hides every IME overlay part (background, underline, caret, clause, and all
/// pooled grapheme cells). Called on every path where the overlay must not be
/// shown, so no part leaks past a commit, cancel, or focus loss.
fn hide_all_overlay_parts(
    nodes: &mut Query<&mut Node>,
    bg: Entity,
    underline: Option<Entity>,
    caret: Option<Entity>,
    clause: Option<Entity>,
    pool: &ImeGraphemePool,
) {
    set_node_display(nodes, bg, Display::None);
    for entity in [underline, caret, clause].into_iter().flatten() {
        set_node_display(nodes, entity, Display::None);
    }
    for &cell in &pool.0 {
        set_node_display(nodes, cell, Display::None);
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
            ui_font.text_font(FontSize::Px(font_size.0)),
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
    use bevy::math::Vec2;
    use bevy::prelude::MinimalPlugins;
    use bevy::window::Ime;
    use orzma_tmux::PaneId;
    use orzma_tmux::{ActivePane, TmuxPane};
    use orzma_tty_renderer::CellMetrics;
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
        use crate::surface::OrzmaTerminal;

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
            .spawn((OrzmaTerminal, KeyboardFocused, TerminalGrid::default()))
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

    /// Builds a `CellMetrics` literal for the overlay ECS tests.
    /// `CellMetrics` has no `Default`, so every field is set; the tests
    /// assert on geometry driven by `advance_phys` / `line_height_phys`,
    /// with the remaining fields (read by `compute_overlay_layout` for the
    /// underline rect) filled with arbitrary non-zero values.
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

    #[test]
    fn ime_overlay_uses_terminal_font_size() {
        use bevy::text::TextFont;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont::default());
        app.insert_resource(TerminalFontSize(9.0));
        app.init_resource::<ImeGraphemePool>();
        app.add_systems(Startup, spawn_ime_overlay_once);
        app.update();

        let mut query = app.world_mut().query::<&TextFont>();
        let matched = query
            .iter(app.world())
            .any(|tf| tf.font_size == FontSize::Px(9.0));
        assert!(
            matched,
            "IME overlay TextFont must use TerminalFontSize (9.0), not the constant"
        );
    }

    #[test]
    fn overlay_background_matches_pane_default_bg_while_composing() {
        use crate::surface::OrzmaTerminal;
        use bevy::window::WindowResolution;
        use orzma_tty_renderer::prelude::Cursor;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont::default());
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
            OrzmaTerminal,
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

    fn run_overlay_with_composition(value: &str, caret: Option<(usize, usize)>) -> App {
        use crate::surface::OrzmaTerminal;
        use bevy::window::WindowResolution;
        use orzma_tty_renderer::prelude::Cursor;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont::default());
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
            OrzmaTerminal,
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
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont::default());
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
        use crate::surface::OrzmaTerminal;
        use bevy::app::Update;
        use bevy::ecs::query::{Changed, Or};
        use bevy::window::WindowResolution;
        use orzma_tty_renderer::prelude::Cursor;

        #[derive(Resource, Default)]
        struct ChangedOverlayNodes(usize);

        fn count_changed(
            mut count: ResMut<ChangedOverlayNodes>,
            changed: Query<
                Entity,
                (
                    Changed<Node>,
                    Or<(
                        With<ImeOverlayNode>,
                        With<ImeUnderline>,
                        With<ImeCaretBar>,
                        With<ImeClauseHighlight>,
                        With<ImeGraphemeCell>,
                    )>,
                ),
            >,
        ) {
            count.0 = changed.iter().count();
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont::default());
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
        app.add_systems(
            Update,
            (position_ime_overlay.run_if(ime_is_composing), count_changed).chain(),
        );
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut().spawn((
            OrzmaTerminal,
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
            "no overlay Node may be re-marked Changed on an unchanged composition",
        );
    }

    #[test]
    fn hide_ime_overlay_hides_parts_after_composition_clears() {
        // Show the overlay with an active composition, then clear it and run the
        // hide system: every part must end Display::None.
        let mut app = run_overlay_with_composition("abc", Some((3, 3)));

        let mut bg = app
            .world_mut()
            .query_filtered::<&Node, With<ImeOverlayNode>>();
        assert_eq!(
            bg.single(app.world()).unwrap().display,
            Display::Flex,
            "precondition: overlay is shown while composing",
        );

        app.insert_resource(ImeState::default());
        app.world_mut().run_system_once(hide_ime_overlay).unwrap();

        let mut bg = app
            .world_mut()
            .query_filtered::<&Node, With<ImeOverlayNode>>();
        assert_eq!(
            bg.single(app.world()).unwrap().display,
            Display::None,
            "hide_ime_overlay must hide the overlay background after composition clears",
        );
        let mut caret = app.world_mut().query_filtered::<&Node, With<ImeCaretBar>>();
        assert_eq!(
            caret.single(app.world()).unwrap().display,
            Display::None,
            "the caret bar must be hidden after composition clears",
        );
        let mut clause = app
            .world_mut()
            .query_filtered::<&Node, With<ImeClauseHighlight>>();
        assert_eq!(
            clause.single(app.world()).unwrap().display,
            Display::None,
            "the clause highlight must be hidden after composition clears",
        );
        let mut underline = app
            .world_mut()
            .query_filtered::<&Node, With<ImeUnderline>>();
        assert_eq!(
            underline.single(app.world()).unwrap().display,
            Display::None,
            "the underline must be hidden after composition clears",
        );
        let pool = app.world().resource::<ImeGraphemePool>().0.clone();
        for &cell in &pool {
            assert_eq!(
                app.world().get::<Node>(cell).unwrap().display,
                Display::None,
                "every pooled grapheme cell must be hidden",
            );
        }
    }

    #[test]
    fn overlay_hidden_and_idle_after_composition_ends() {
        use crate::surface::OrzmaTerminal;
        use bevy::app::PostUpdate;
        use bevy::window::WindowResolution;
        use orzma_tty_renderer::prelude::Cursor;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont::default());
        app.insert_resource(TerminalFontSize(12.0));
        app.insert_resource(TerminalCellMetricsResource {
            metrics: metrics(10.0, 16.0),
            phys_font_size: 12,
        });
        app.init_resource::<ImeGraphemePool>();
        app.init_resource::<ImeState>();

        app.add_systems(Startup, spawn_ime_overlay_once);
        app.add_systems(
            PostUpdate,
            (
                position_ime_overlay.run_if(ime_is_composing),
                hide_ime_overlay.run_if(
                    resource_exists_and_changed::<ImeState>.and_then(not(ime_is_composing)),
                ),
            ),
        );
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut().spawn((
            OrzmaTerminal,
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

        {
            let mut state = app.world_mut().resource_mut::<ImeState>();
            apply_event(
                &mut state,
                &Ime::Preedit {
                    window: Entity::PLACEHOLDER,
                    value: "abc".into(),
                    cursor: Some((3, 3)),
                },
            );
        }
        app.update();
        let mut bg = app
            .world_mut()
            .query_filtered::<&Node, With<ImeOverlayNode>>();
        assert_eq!(
            bg.single(app.world()).unwrap().display,
            Display::Flex,
            "overlay shows while composing",
        );

        {
            let mut state = app.world_mut().resource_mut::<ImeState>();
            apply_event(
                &mut state,
                &Ime::Commit {
                    window: Entity::PLACEHOLDER,
                    value: "abc".into(),
                },
            );
        }
        app.update();
        let mut bg = app
            .world_mut()
            .query_filtered::<&Node, With<ImeOverlayNode>>();
        assert_eq!(
            bg.single(app.world()).unwrap().display,
            Display::None,
            "overlay hides the frame composition ends",
        );

        app.update();
        let mut bg = app
            .world_mut()
            .query_filtered::<&Node, With<ImeOverlayNode>>();
        assert_eq!(
            bg.single(app.world()).unwrap().display,
            Display::None,
            "overlay stays hidden while idle (neither gated system runs)",
        );
    }
}
