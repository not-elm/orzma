//! IME preedit overlay.
//!
//! Provides `compute_overlay_pos` (the pure pixel-math function for
//! anchoring the overlay), `ImeOverlayPlugin` (Bevy plugin that spawns
//! the overlay entity tree at Startup and schedules
//! `position_ime_overlay`), and the marker components identifying the
//! root, pre-caret span, post-caret span, and caret bar.

use crate::font::TerminalUiFont;
use crate::input::ime::ImeState;
use crate::input::resolve_focused_terminal;
use crate::ui::registry::SurfaceEntityRegistry;
use bevy::app::{App, Plugin, PostUpdate, Startup};
use bevy::color::Color;
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::query::{With, Without};
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::system::{Commands, Query, Res};
use bevy::math::Vec2;
use bevy::prelude::default;
use bevy::text::{LineBreak, TextColor, TextFont, TextLayout, Underline, UnderlineColor};
use bevy::ui::widget::Text;
use bevy::ui::{
    BackgroundColor, BorderColor, ComputedNode, Display, GlobalZIndex, Node, PositionType,
    UiGlobalTransform, UiRect, UiSystems, Val,
};
use bevy::window::{PrimaryWindow, Window};
use bevy_terminal_renderer::CellMetrics;
use bevy_terminal_renderer::TerminalCellMetricsResource;
use bevy_terminal_renderer::TerminalFontInitSet;
use bevy_terminal_renderer::material::TerminalMaterialSystems;
use bevy_terminal_renderer::prelude::TerminalGrid;
use ozmux_multiplexer::{AttachedWorkspace, MultiplexerCommands, WorkspaceMarker};
use unicode_width::UnicodeWidthStr;

/// Bevy plugin that spawns the IME overlay entity tree at Startup and
/// schedules `position_ime_overlay` in PostUpdate.
pub struct ImeOverlayPlugin;

impl Plugin for ImeOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
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
                suppress_terminal_cursor_during_ime.before(TerminalMaterialSystems::UpdateMaterial),
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
/// width logic in `bevy_terminal_renderer::grid`.
///
/// Caller is responsible for byte-offset validity (UTF-8 boundary,
/// `begin <= end <= text.len()`); `Composition::try_new` enforces these.
fn caret_cell_offsets(text: &str, (begin, end): (usize, usize)) -> (f32, f32) {
    let begin_cells = UnicodeWidthStr::width(&text[..begin]) as f32;
    let end_cells = UnicodeWidthStr::width(&text[..end]) as f32;
    (begin_cells, end_cells)
}

/// PostUpdate system that positions the IME preedit overlay at the
/// attached terminal's cursor cell, writes the composition text into
/// the overlay's root `Text`, and positions the caret bar (beam when
/// `begin == end`) or clause highlight (hollow block when
/// `begin != end`).
///
/// When `ImeState` has no composition, hides the overlay and returns.
/// When the attached entity is missing or lacks the expected
/// components, hides the overlay; the next `Ime` event clears
/// `ImeState`.
pub(crate) fn position_ime_overlay(
    state: Res<ImeState>,
    mux: MultiplexerCommands,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    registry: Res<SurfaceEntityRegistry>,
    anchors: Query<(&ComputedNode, &UiGlobalTransform, &TerminalGrid)>,
    metrics: Res<TerminalCellMetricsResource>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    mut overlay_root: Query<(&mut Node, &mut Text), With<ImeOverlayNode>>,
    mut caret_bar: Query<
        &mut Node,
        (
            With<ImeCaretBar>,
            Without<ImeOverlayNode>,
            Without<ImeClauseHighlight>,
        ),
    >,
    mut clause_highlight: Query<
        &mut Node,
        (
            With<ImeClauseHighlight>,
            Without<ImeOverlayNode>,
            Without<ImeCaretBar>,
        ),
    >,
) {
    let Ok((mut root_node, mut root_text)) = overlay_root.single_mut() else {
        return;
    };
    let mut bar = caret_bar.single_mut().ok();
    let mut clause = clause_highlight.single_mut().ok();

    // Default: hide every overlay part. The success path below re-shows
    // root + (bar OR clause) as needed. Every early-return arm leaves
    // these set to Display::None so the bar/clause don't leak past a
    // commit / cancel / focus-loss.
    root_node.display = Display::None;
    if let Some(b) = bar.as_mut() {
        b.display = Display::None;
    }
    if let Some(c) = clause.as_mut() {
        c.display = Display::None;
    }

    let Some(comp) = state.composition() else {
        return;
    };
    let Some(entity) = resolve_focused_terminal(&mux, &attached_workspace, &registry) else {
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

    // NOTE: `measured_width_logical = 0.0` is a known MVP shortcut.
    // Reading `TextLayoutInfo.size.x` for accurate clamping requires
    // an additional query AND careful ordering against Bevy's text
    // layout pipeline — the right value is filled by Bevy in
    // `UiSystems::PostLayout`, but this system runs before that. The
    // overlay therefore won't clamp at the right edge until the next
    // tick after a width change. Bounded impact: at most a 1-frame
    // visual misalignment after the composition grows past the pane
    // edge. The candidate-window position (in `ime_policy_system`)
    // uses the cursor anchor only, so the OS popup is unaffected.
    let measured_width_logical = 0.0;

    // NOTE: pass `ui_xform.translation` (center, physical px) and
    // `node.size` (physical px) — `compute_overlay_pos` derives
    // both the top-left and the logical-px clamp bounds internally.
    let pos = compute_overlay_pos(
        ui_xform.translation,
        node.size,
        cursor_cell,
        &metrics.metrics,
        measured_width_logical,
        scale,
    );

    root_node.left = Val::Px(pos.x);
    root_node.top = Val::Px(pos.y);
    root_node.display = Display::Flex;

    // Write the full composition text to the root. With a single
    // `Text` entity (no `TextSpan` children), Bevy's text pipeline
    // shapes through cosmic-text directly and the registered
    // UDEVGothic35 fallback covers CJK script.
    if root_text.0 != comp.text() {
        root_text.0 = comp.text().to_string();
    }

    let cell_w_logical = metrics.metrics.advance_phys.floor().max(1.0) / scale;
    let line_h_logical = metrics.metrics.line_height_phys.floor().max(1.0) / scale;
    let (begin_cells, end_cells) = match comp.caret() {
        Some(range) => caret_cell_offsets(comp.text(), range),
        None => (0.0, 0.0),
    };
    let has_clause = comp.caret().is_some_and(|(b, e)| b != e);
    let has_beam = comp.caret().is_some() && !has_clause;
    let beam_x_logical = pos.x + end_cells * cell_w_logical;
    let clause_x_logical = pos.x + begin_cells * cell_w_logical;
    let clause_w_logical = (end_cells - begin_cells) * cell_w_logical;

    if has_beam && let Some(b) = bar.as_mut() {
        // Caret bar is a top-level UI entity (not a child of the
        // Text root), so its position is in window-absolute coords:
        // overlay origin + per-character horizontal offset.
        b.display = Display::Flex;
        b.left = Val::Px(beam_x_logical);
        b.top = Val::Px(pos.y);
        b.height = Val::Px(line_h_logical);
    }

    if has_clause && let Some(c) = clause.as_mut() {
        // Hollow block over the macOS-IME clause-selection range.
        // Positioned in window-absolute coords for the same
        // leaf-Text-no-children reason as ImeCaretBar.
        c.display = Display::Flex;
        c.left = Val::Px(clause_x_logical);
        c.top = Val::Px(pos.y);
        c.width = Val::Px(clause_w_logical);
        c.height = Val::Px(line_h_logical);
    }
}

/// Sets `TerminalGrid.suppress_cursor = true` on the currently-focused
/// terminal entity while IME composition is active; clears it on all
/// other grids. Runs in `PostUpdate.before(TerminalMaterialSystems::UpdateMaterial)`
/// so the override takes effect in the same frame the IME caret
/// appears (and clears the same frame composition ends).
///
/// If `resolve_focused_terminal` returns `None` while composition is
/// active (e.g., race window between focus loss and `Ime::Disabled`),
/// every grid gets `suppress_cursor = false`. The safe default is
/// "show cursors" rather than blanket-hide.
fn suppress_terminal_cursor_during_ime(
    state: Res<ImeState>,
    mux: MultiplexerCommands,
    attached_workspace: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>,
    registry: Res<SurfaceEntityRegistry>,
    mut grids: Query<(Entity, &mut TerminalGrid)>,
) {
    let focused = if state.is_composing() {
        resolve_focused_terminal(&mux, &attached_workspace, &registry)
    } else {
        None
    };
    for (entity, mut grid) in &mut grids {
        let want = Some(entity) == focused;
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
fn spawn_ime_overlay_once(mut commands: Commands, ui_font: Res<TerminalUiFont>) {
    let text_font = TextFont {
        font: ui_font.0.clone(),
        font_size: bevy_terminal_renderer::FONT_SIZE_PX,
        ..default()
    };
    let color = Color::WHITE;

    commands.spawn((
        Text::new(""),
        text_font.clone(),
        TextColor(color),
        TextLayout {
            linebreak: LineBreak::NoWrap,
            ..default()
        },
        Underline,
        UnderlineColor(color),
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
