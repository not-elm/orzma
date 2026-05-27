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
use crate::multiplexer::{AttachedSession, Multiplexer, SessionEntityId};
use crate::ui::registry::ActivityEntityRegistry;
use bevy::app::{App, Plugin, Startup};
use bevy::color::Color;
use bevy::ecs::component::Component;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::query::{With, Without};
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::system::{Commands, Query, Res};
use bevy::math::Vec2;
use bevy::prelude::default;
use bevy::text::{TextColor, TextFont, TextSpan, Underline, UnderlineColor};
use bevy::ui::widget::Text;
use bevy::ui::{
    BackgroundColor, ComputedNode, Display, GlobalZIndex, Node, PositionType, UiGlobalTransform,
    UiSystems, Val,
};
use bevy::window::{PrimaryWindow, Window};
use bevy_terminal_renderer::CellMetrics;
use bevy_terminal_renderer::TerminalCellMetricsResource;
use bevy_terminal_renderer::TerminalFontInitSet;
use bevy_terminal_renderer::prelude::TerminalGrid;

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
            bevy::app::PostUpdate,
            position_ime_overlay.before(UiSystems::Content),
        );
    }
}

/// Marker for the singleton IME preedit overlay root entity.
#[derive(Component)]
pub(crate) struct ImeOverlayNode;

/// Marker for the pre-caret `TextSpan` child of the overlay root.
#[derive(Component)]
pub(crate) struct ImePreCaretSpan;

/// Marker for the post-caret `TextSpan` child of the overlay root.
#[derive(Component)]
pub(crate) struct ImePostCaretSpan;

/// Marker for the 1-px sibling `Node` that draws the caret bar.
#[derive(Component)]
pub(crate) struct ImeCaretBar;

/// Computes the overlay's top-left logical-pixel position relative to
/// the window origin. Caller is responsible for writing this into
/// `Node.left` / `Node.top`.
///
/// All metric inputs are physical px; the function does the
/// physical→logical conversion via `scale`.
///
/// Layout: the overlay sits **one row below** the cursor cell so the
/// inline preedit doesn't overlap with the active-line glyph still
/// rendered by the terminal material. Clamps:
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
            (cursor_cell.1 as f32 + 1.0) * cell_h_phys,
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

/// PostUpdate system that positions the IME preedit overlay at the
/// attached terminal's cursor cell, sets the two `TextSpan` children
/// to the pre- and post-caret substrings, and positions the caret
/// bar.
///
/// When `ImeState` has no composition, hides the overlay and returns.
/// When the attached entity is missing or lacks the expected
/// components, hides the overlay; the next `Ime` event clears
/// `ImeState`.
pub(crate) fn position_ime_overlay(
    state: Res<ImeState>,
    attached_sid_q: Query<&SessionEntityId, With<AttachedSession>>,
    mux: Res<Multiplexer>,
    registry: Res<ActivityEntityRegistry>,
    anchor_q: Query<(&ComputedNode, &UiGlobalTransform, &TerminalGrid)>,
    metrics: Res<TerminalCellMetricsResource>,
    window_q: Query<&Window, With<PrimaryWindow>>,
    mut root_q: Query<&mut Node, With<ImeOverlayNode>>,
    mut pre_q: Query<&mut TextSpan, (With<ImePreCaretSpan>, Without<ImePostCaretSpan>)>,
    mut post_q: Query<&mut TextSpan, (With<ImePostCaretSpan>, Without<ImePreCaretSpan>)>,
    mut bar_q: Query<&mut Node, (With<ImeCaretBar>, Without<ImeOverlayNode>)>,
) {
    let Ok(mut root_node) = root_q.single_mut() else {
        return;
    };

    let Some(comp) = state.composition() else {
        root_node.display = Display::None;
        return;
    };

    let Some(entity) = resolve_focused_terminal(&attached_sid_q, &mux, &registry) else {
        root_node.display = Display::None;
        return;
    };
    let Ok((node, ui_xform, grid)) = anchor_q.get(entity) else {
        root_node.display = Display::None;
        return;
    };
    let Ok(window) = window_q.single() else {
        root_node.display = Display::None;
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

    let (pre_text, post_text) = match comp.caret() {
        Some(caret) => (&comp.text()[..caret], &comp.text()[caret..]),
        None => ("", comp.text()),
    };
    if let Ok(mut pre) = pre_q.single_mut()
        && pre.0 != pre_text
    {
        pre.0 = pre_text.to_string();
    }
    if let Ok(mut post) = post_q.single_mut()
        && post.0 != post_text
    {
        post.0 = post_text.to_string();
    }

    // NOTE: The caret bar's horizontal offset is approximated by
    // `chars().count()` × cell-width. Exact for monospace ASCII;
    // slightly off for CJK in the preedit (which is rare — the
    // preedit is usually Romaji being converted). The terminal font
    // is monospace, so the bounded error is acceptable.
    let cell_w_logical = metrics.metrics.advance_phys.floor().max(1.0) / scale;
    let approx_caret_x_logical = pre_text.chars().count() as f32 * cell_w_logical;
    let line_h_logical = metrics.metrics.line_height_phys.floor().max(1.0) / scale;

    if let Ok(mut bar) = bar_q.single_mut() {
        if comp.caret().is_some() {
            bar.display = Display::Flex;
            bar.left = Val::Px(approx_caret_x_logical);
            bar.top = Val::Px(0.0);
            bar.height = Val::Px(line_h_logical);
        } else {
            bar.display = Display::None;
        }
    }
}

/// Global z-index for the IME overlay; placed high enough to float
/// above all other UI nodes.
const IME_OVERLAY_Z: i32 = 200;

/// Spawns the single overlay entity tree.
fn spawn_ime_overlay_once(mut commands: Commands, ui_font: Res<TerminalUiFont>) {
    let text_font = TextFont {
        font: ui_font.0.clone(),
        font_size: bevy_terminal_renderer::FONT_SIZE_PX,
        ..default()
    };
    let root = commands
        .spawn((
            Text::new(""),
            text_font.clone(),
            Node {
                position_type: PositionType::Absolute,
                display: Display::None,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                ..default()
            },
            GlobalZIndex(IME_OVERLAY_Z),
            ImeOverlayNode,
        ))
        .id();

    // TODO: bind TextColor / UnderlineColor / BackgroundColor to theme
    // tokens (`text-foreground` / `bg-background`) once the
    // theme-token helper is integrated. Placeholder white for now.
    let color = Color::WHITE;

    commands.spawn((
        TextSpan::new(""),
        text_font.clone(),
        TextColor(color),
        Underline,
        UnderlineColor(color),
        ImePreCaretSpan,
        ChildOf(root),
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
        ImeCaretBar,
        ChildOf(root),
    ));
    commands.spawn((
        TextSpan::new(""),
        text_font.clone(),
        TextColor(color),
        Underline,
        UnderlineColor(color),
        ImePostCaretSpan,
        ChildOf(root),
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
    fn places_overlay_one_row_below_cursor() {
        let (translation_phys, size_phys) =
            host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (3, 5),
            &metrics(10.0, 16.0),
            0.0,
            1.0,
        );
        // y = (row 5 + 1) × 16 = 96
        assert_eq!(pos.y, 96.0);
        // x = col 3 × 10 = 30, no clamp
        assert_eq!(pos.x, 30.0);
    }

    #[test]
    fn divides_by_scale_factor_for_logical_px() {
        // Logical top-left (100, 0), logical size 800×600, scale 2.0.
        // At scale 2.0: physical size = (1600, 1200), physical
        // top-left = (200, 0), physical center = (1000, 600).
        // Cursor (0, 0) row-below → cell_origin_phys = (200, 16) →
        // pos_logical = (100, 8).
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
        assert_eq!(pos.y, 8.0);
    }

    #[test]
    fn floors_subpixel_cell_pitch() {
        // advance 10.4 → floor 10; col 10 → x = 100
        // line_height 16.4 → floor 16; row 1 row-below → y = (1+1) × 16 = 32
        let (translation_phys, size_phys) =
            host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
        let pos = compute_overlay_pos(
            translation_phys,
            size_phys,
            (10, 1),
            &metrics(10.4, 16.4),
            0.0,
            1.0,
        );
        assert_eq!(pos.x, 100.0);
        assert_eq!(pos.y, 32.0);
    }

    #[test]
    fn clamps_right_when_overlay_overflows() {
        // Cursor at col 78, cell width 10 → cell_origin x = 780.
        // Measured width 100 → would extend to 880, host right = 800.
        // Shift left by 80 → left = 700.
        let (translation_phys, size_phys) =
            host_inputs(Vec2::ZERO, Vec2::new(800.0, 600.0), 1.0);
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
        let (translation_phys, size_phys) =
            host_inputs(Vec2::ZERO, Vec2::new(80.0, 600.0), 1.0);
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
        // Expected: pos = (10 + 5*10, 20 + (3+1)*16) = (60, 84).
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
        assert_eq!(pos.y, 84.0);
    }
}
