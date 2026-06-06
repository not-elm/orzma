//! Sizes each pane LEAF node to its Mux-resolved cell rect in LOGICAL px.
//! Split containers stay `flex_direction`-only; taffy sizes them from the
//! fixed-px children (drift-free via taffy cumulative-edge rounding).
//!
//! Runs in `PostUpdate` **before** `UiSystems::Layout` so the new sizes
//! take effect within the same frame: `geometry_feed` measured the viewport
//! last frame → `PaneResized` → `PaneDimensions` stamped → this system
//! converts cells to logical px → taffy lays out on this frame.

use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy::window::{PrimaryWindow, Window};
use bevy_terminal_renderer::TerminalCellMetricsResource;
use ozmux_multiplexer::{PaneDimensions, PaneMarker};

/// Bevy plugin that registers `size_pane_leaves`.
pub(crate) struct PaneLayoutPlugin;

impl Plugin for PaneLayoutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, size_pane_leaves.before(UiSystems::Layout));
    }
}

/// Converts each pane leaf's `PaneDimensions` to a logical-px `Node` size.
///
/// `advance_phys` and `line_height_phys` are in PHYSICAL pixels (DPR-baked);
/// dividing by `scale_factor` yields logical pixels, which is what `Val::Px`
/// expects.  Uses `floor(advance_phys)` to match the renderer's cell pitch
/// (see `src/ui/terminal.rs`).
///
/// Runs on ALL panes every frame (no `Changed` filter) so that a DPR or cell-
/// metrics change re-sizes every pane even when `PaneDimensions` did not change.
/// A set-if-neq guard on each field avoids marking `Node` dirty on frames where
/// nothing has actually changed.
pub(crate) fn size_pane_leaves(
    mut panes: Query<(&PaneDimensions, &mut Node), With<PaneMarker>>,
    metrics: Res<TerminalCellMetricsResource>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let raw_scale = window.scale_factor();
    let scale = if raw_scale.is_finite() && raw_scale > 0.0 {
        raw_scale
    } else {
        1.0
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0) / scale;
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0) / scale;
    for (dim, mut node) in panes.iter_mut() {
        let target_w = Val::Px(f32::from(dim.cols) * cell_w);
        let target_h = Val::Px(f32::from(dim.rows) * cell_h);
        if node.width != target_w {
            node.width = target_w;
        }
        if node.height != target_h {
            node.height = target_h;
        }
        if node.flex_grow != 0.0 {
            node.flex_grow = 0.0;
        }
        if node.flex_shrink != 0.0 {
            node.flex_shrink = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::window::WindowResolution;
    use bevy_terminal_renderer::{CellMetrics, TerminalCellMetricsResource};
    use ozmux_multiplexer::{PaneDimensions, PaneMarker};

    use super::*;

    #[test]
    fn size_pane_leaves_sets_logical_px_from_dims() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        // Insert a primary window with scale_factor = 2.0 (HiDPI).
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        // NOTE: WindowResolution::new does not set scale_factor; it stays 1.0
        // by default in headless tests.  Override to 2.0 explicitly.
        {
            let mut q = app
                .world_mut()
                .query_filtered::<&mut Window, With<PrimaryWindow>>();
            let mut win = q.single_mut(app.world_mut()).unwrap();
            win.resolution.set_scale_factor(2.0);
        }

        // advance_phys = 16.0, line_height_phys = 32.0 (physical px at DPR 2)
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 16.0,
                line_height_phys: 32.0,
                ascent_phys: 24.0,
                descent_phys: 8.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        });

        let pane = app
            .world_mut()
            .spawn((
                PaneMarker,
                PaneDimensions { cols: 80, rows: 24 },
                Node::default(),
            ))
            .id();

        app.world_mut().run_system_once(size_pane_leaves).unwrap();

        let node = app.world().get::<Node>(pane).unwrap();
        // logical cell_w = floor(16.0) / 2.0 = 8.0; 80 * 8.0 = 640.0
        // logical cell_h = floor(32.0) / 2.0 = 16.0; 24 * 16.0 = 384.0
        assert_eq!(node.width, Val::Px(640.0), "width = cols × logical cell_w");
        assert_eq!(
            node.height,
            Val::Px(384.0),
            "height = rows × logical cell_h"
        );
        assert_eq!(node.flex_grow, 0.0, "flex_grow must be 0 after sizing");
        assert_eq!(node.flex_shrink, 0.0, "flex_shrink must be 0 after sizing");
    }
}
