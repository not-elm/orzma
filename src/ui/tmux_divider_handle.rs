//! Visible drag handles in tmux's reserved inter-pane gap, plus the resize
//! mouse cursor while hovering them.
//!
//! For each `Divider` projected onto a window (`TmuxDividers`), one handle node
//! fills the 1-cell gap the divider occupies (subtle grey, brightening to the
//! accent color on hover). A hover system reuses the arbiter's `divider_at`
//! hit-test so the visible handle, the resize grab zone, and the cursor all
//! coincide, and sets a `ColResize` / `RowResize` cursor on the primary window
//! while the pointer is over a divider.

use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::theme;
use crate::tmux_mouse::divider_at;
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozmux_tmux::{ActiveWindow, TmuxDividers, TmuxProjectionSet};
use tmux_control_parser::{Divider, DividerAxis};

/// Registers the divider-handle visuals and the resize hover cursor.
pub(crate) struct OzmuxTmuxDividerHandlePlugin;

impl Plugin for OzmuxTmuxDividerHandlePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                reconcile_divider_handles.after(TmuxProjectionSet),
                position_divider_handles.after(reconcile_divider_handles),
                divider_hover_feedback.after(InputPhase::Hover),
            ),
        );
    }
}

/// A visual bar filling one divider's reserved gap. `window` is the owning
/// `TmuxWindow` entity (the handle's `ChildOf` parent), stored so a layout
/// change can despawn this window's handles before respawning the new set.
#[derive(Component)]
struct DividerHandle {
    divider: Divider,
    window: Entity,
}

/// Logical-px rect `(left, top, width, height)` for a divider's handle, filling
/// its reserved gap cell and spanning the shared edge. `cell_w` / `cell_h` are
/// logical px per cell.
fn handle_rect(divider: Divider, cell_w: f32, cell_h: f32) -> (f32, f32, f32, f32) {
    let span = (divider.span_end - divider.span_start).max(0) as f32;
    match divider.axis {
        DividerAxis::Vertical => (
            divider.pos as f32 * cell_w,
            divider.span_start as f32 * cell_h,
            cell_w,
            span * cell_h,
        ),
        DividerAxis::Horizontal => (
            divider.span_start as f32 * cell_w,
            divider.pos as f32 * cell_h,
            span * cell_w,
            cell_h,
        ),
    }
}

/// Logical px per cell from the renderer's physical metrics and the window DPR.
fn logical_cell_size(metrics: &TerminalCellMetricsResource, dpr: f32) -> (f32, f32) {
    (
        metrics.metrics.advance_phys.floor().max(1.0) / dpr,
        metrics.metrics.line_height_phys.floor().max(1.0) / dpr,
    )
}

/// Despawns and respawns a window's handle bars whenever its `TmuxDividers`
/// change (first projection + every `%layout-change`). Spawns each handle with
/// its gap rect already set so it appears in place without a one-frame stutter;
/// `position_divider_handles` keeps it in sync afterward.
fn reconcile_divider_handles(
    mut commands: Commands,
    changed: Query<(Entity, &TmuxDividers), Changed<TmuxDividers>>,
    handles: Query<(Entity, &DividerHandle)>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    if changed.is_empty() {
        return;
    }
    let dpr = window
        .single()
        .map(|w| w.scale_factor().max(0.5))
        .unwrap_or(1.0);
    let (cell_w, cell_h) = logical_cell_size(&metrics, dpr);
    for (window_entity, dividers) in changed.iter() {
        for (handle, handle_data) in handles.iter() {
            if handle_data.window == window_entity {
                commands.entity(handle).despawn();
            }
        }
        for divider in &dividers.0 {
            let (left, top, w, h) = handle_rect(*divider, cell_w, cell_h);
            commands.spawn((
                DividerHandle {
                    divider: *divider,
                    window: window_entity,
                },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(left),
                    top: Val::Px(top),
                    width: Val::Px(w),
                    height: Val::Px(h),
                    ..default()
                },
                BackgroundColor(theme::BORDER),
                ChildOf(window_entity),
            ));
        }
    }
}

/// Keeps each handle's gap rect in sync with the current cell metrics / DPR
/// (font or monitor changes), mirroring `layout_tmux_panes`. Writes the `Node`
/// only when it actually changes to avoid forcing a relayout every frame.
fn position_divider_handles(
    mut handles: Query<(&DividerHandle, &mut Node)>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let dpr = window.scale_factor().max(0.5);
    let (cell_w, cell_h) = logical_cell_size(&metrics, dpr);
    for (handle, mut node) in handles.iter_mut() {
        let (left, top, w, h) = handle_rect(handle.divider, cell_w, cell_h);
        if node.left != Val::Px(left)
            || node.top != Val::Px(top)
            || node.width != Val::Px(w)
            || node.height != Val::Px(h)
        {
            node.left = Val::Px(left);
            node.top = Val::Px(top);
            node.width = Val::Px(w);
            node.height = Val::Px(h);
        }
    }
}

/// Brightens the hovered divider's handle to the accent color and sets a
/// `ColResize` / `RowResize` cursor on the primary window. Runs after
/// `InputPhase::Hover` so it overrides the hyperlink baseline cursor; when no
/// divider is hovered it leaves the cursor for that baseline to reassert.
fn divider_hover_feedback(
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    mut handles: Query<(&DividerHandle, &mut BackgroundColor)>,
    dividers_q: Query<&TmuxDividers, With<ActiveWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    configs: Option<Res<OzmuxConfigsResource>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let tol = configs
        .as_deref()
        .map(|c| c.mouse.divider_grab_tolerance_px)
        .unwrap_or(4.0)
        * scale;
    let dividers: &[Divider] = dividers_q.single().map(|d| d.0.as_slice()).unwrap_or(&[]);
    let hovered = window
        .cursor_position()
        .map(|c| c * scale)
        .and_then(|cursor_phys| divider_at(dividers, cursor_phys, cell_w, cell_h, tol));

    for (handle, mut bg) in handles.iter_mut() {
        let want = if Some(handle.divider) == hovered {
            theme::ACCENT
        } else {
            theme::BORDER
        };
        if bg.0 != want {
            bg.0 = want;
        }
    }

    let Some(divider) = hovered else {
        return;
    };
    let icon = match divider.axis {
        DividerAxis::Vertical => SystemCursorIcon::ColResize,
        DividerAxis::Horizontal => SystemCursorIcon::RowResize,
    };
    if let Ok(mut cursor) = cursor_icons.single_mut()
        && !matches!(&*cursor, CursorIcon::System(e) if *e == icon)
    {
        *cursor = CursorIcon::System(icon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_renderer::CellMetrics;
    use tmux_control_parser::PaneId;

    fn test_metrics() -> TerminalCellMetricsResource {
        TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        }
    }

    #[test]
    fn reconcile_tracks_one_handle_per_divider() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(test_metrics());
        app.add_systems(Update, reconcile_divider_handles);
        app.world_mut().spawn((
            Window {
                ..Default::default()
            },
            PrimaryWindow,
        ));
        let window = app
            .world_mut()
            .spawn(TmuxDividers(vec![vdiv(40, 0, 24), hdiv(12, 0, 80)]))
            .id();
        app.update();

        let count = |app: &mut App| {
            app.world_mut()
                .query::<&DividerHandle>()
                .iter(app.world())
                .count()
        };
        assert_eq!(count(&mut app), 2, "one handle per divider");
        assert!(
            app.world_mut()
                .query::<&DividerHandle>()
                .iter(app.world())
                .all(|h| h.window == window),
            "handles belong to the dividers' window",
        );

        // A layout change to a single divider despawns the stale handles and
        // respawns the new set.
        app.world_mut()
            .entity_mut(window)
            .insert(TmuxDividers(vec![vdiv(20, 0, 24)]));
        app.update();
        assert_eq!(count(&mut app), 1, "reconciled to the new divider set");

        // Collapsing to a single pane (no dividers) removes all handles.
        app.world_mut()
            .entity_mut(window)
            .insert(TmuxDividers(vec![]));
        app.update();
        assert_eq!(count(&mut app), 0, "no dividers -> no handles");
    }

    fn vdiv(pos: i32, s: i32, e: i32) -> Divider {
        Divider {
            axis: DividerAxis::Vertical,
            primary: PaneId(1),
            pos,
            span_start: s,
            span_end: e,
        }
    }

    fn hdiv(pos: i32, s: i32, e: i32) -> Divider {
        Divider {
            axis: DividerAxis::Horizontal,
            primary: PaneId(1),
            pos,
            span_start: s,
            span_end: e,
        }
    }

    #[test]
    fn vertical_handle_fills_gap_column_over_its_span() {
        // pos=40 gap column, rows 2..10, 8x16 logical cells.
        let (left, top, w, h) = handle_rect(vdiv(40, 2, 10), 8.0, 16.0);
        assert_eq!((left, top, w, h), (320.0, 32.0, 8.0, 128.0));
    }

    #[test]
    fn horizontal_handle_fills_gap_row_over_its_span() {
        // pos=12 gap row, columns 0..80, 8x16 logical cells.
        let (left, top, w, h) = handle_rect(hdiv(12, 0, 80), 8.0, 16.0);
        assert_eq!((left, top, w, h), (0.0, 192.0, 640.0, 16.0));
    }
}
