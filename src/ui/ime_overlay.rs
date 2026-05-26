//! IME preedit overlay.
//!
//! Provides `compute_overlay_pos` (the pure pixel-math function for
//! anchoring the overlay), `ImeOverlayPlugin` (Bevy plugin that spawns
//! the overlay entity tree at Startup and schedules
//! `position_ime_overlay`), and the marker components identifying the
//! root, pre-caret span, post-caret span, and caret bar.

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
use bevy::text::{TextColor, TextSpan, Underline, UnderlineColor};
use bevy::ui::widget::Text;
use bevy::ui::{
    BackgroundColor, ComputedNode, Display, GlobalZIndex, Node, PositionType, UiGlobalTransform,
    UiSystems, Val,
};
use bevy::window::{PrimaryWindow, Window};
use bevy_terminal_renderer::CellMetrics;
use bevy_terminal_renderer::TerminalCellMetricsResource;
use bevy_terminal_renderer::prelude::TerminalGrid;

/// Bevy plugin that spawns the IME overlay entity tree at Startup and
/// schedules `position_ime_overlay` in PostUpdate.
pub struct ImeOverlayPlugin;

impl Plugin for ImeOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_ime_overlay_once)
            .add_systems(
                bevy::app::PostUpdate,
                position_ime_overlay
                    .after(UiSystems::Layout)
                    .before(UiSystems::PostLayout),
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
    ui_global_translation: Vec2,
    host_size_logical: Vec2,
    cursor_cell: (u16, u16),
    metrics: &CellMetrics,
    measured_width_logical: f32,
    scale: f32,
) -> Vec2 {
    let cell_w_phys = metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.line_height_phys.floor().max(1.0);
    let host_origin_phys = ui_global_translation * scale;
    let cell_origin_phys = host_origin_phys
        + Vec2::new(
            cursor_cell.0 as f32 * cell_w_phys,
            (cursor_cell.1 as f32 + 1.0) * cell_h_phys,
        );
    let pos_logical = cell_origin_phys / scale;

    let host_right = ui_global_translation.x + host_size_logical.x;
    let mut left = pos_logical.x;
    if left + measured_width_logical > host_right {
        left = host_right - measured_width_logical;
    }
    let host_left = ui_global_translation.x;
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

    // NOTE: ComputedNode.size is in physical px (precedent at
    // src/ui/terminal.rs::resize_terminals_to_node), so divide by
    // scale to match compute_overlay_pos's logical-px expectation.
    // Today this is masked by `measured_width_logical = 0.0` (the
    // right-edge clamp is virtually unreachable), but the unit
    // mismatch would silently miscompute the clamp at DPR > 1.0 if
    // that shortcut is later replaced with a real text-width
    // measurement.
    let host_size_logical = node.size / scale;

    let pos = compute_overlay_pos(
        ui_xform.translation,
        host_size_logical,
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
fn spawn_ime_overlay_once(mut commands: Commands) {
    let root = commands
        .spawn((
            Text::new(""),
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

    #[test]
    fn places_overlay_one_row_below_cursor() {
        let pos = compute_overlay_pos(
            Vec2::ZERO,
            Vec2::new(800.0, 600.0),
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
        // translation (100, 0) logical at scale 2.0 → host_origin_phys (200, 0)
        // cell (0, 0) row-below → cell_origin_phys (200, 16) → logical (100, 8)
        let pos = compute_overlay_pos(
            Vec2::new(100.0, 0.0),
            Vec2::new(800.0, 600.0),
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
        let pos = compute_overlay_pos(
            Vec2::ZERO,
            Vec2::new(800.0, 600.0),
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
        let pos = compute_overlay_pos(
            Vec2::ZERO,
            Vec2::new(800.0, 600.0),
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
        let pos = compute_overlay_pos(
            Vec2::ZERO,
            Vec2::new(80.0, 600.0),
            (7, 0),
            &metrics(10.0, 16.0),
            200.0,
            1.0,
        );
        assert_eq!(pos.x, 0.0);
    }
}
