//! Multiplexer layout application: converts the active window's layout tree
//! into per-pane `Node` rects and PTY sizes, writing each only when it changes.
//! Multi-pane generalization of the old single-terminal `resize_to_window`.

use crate::multiplexer::bootstrap::WindowContainer;
use crate::multiplexer::layout::PaneRect;
use crate::multiplexer::pane::MultiplexerPane;
use crate::multiplexer::window::{ActiveMultiplexerWindow, MultiplexerLayoutComp};
use crate::surface::geometry::cells_for;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiSystems};
use orzma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};
use orzma_tty_renderer::TerminalCellMetricsResource;

/// Pixel gap left between sibling panes when the layout tree splits an area.
pub(crate) const PANE_GAP_PX: f32 = 1.0;

/// Registers layout application: reflows every pane's `Node` rect and PTY
/// size from the active window's layout tree whenever the workspace resizes,
/// the tree changes, or the cell metrics change.
///
/// Runs in `PostUpdate`, after `UiSystems::Layout`: that set is where
/// `ui_layout_system` writes `ComputedNode`, so scheduling from `Update`
/// would read a stale size a frame late.
pub(in crate::multiplexer) struct LayoutPlugin;

impl Plugin for LayoutPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            apply_layout
                .after(UiSystems::Layout)
                .run_if(resource_exists::<TerminalCellMetricsResource>)
                .run_if(
                    window_container_computed_changed
                        .or_else(active_layout_tree_changed)
                        .or_else(resource_exists_and_changed::<TerminalCellMetricsResource>),
                ),
        );
    }
}

/// The pane's most recently applied PTY cell size, guarding redundant
/// resizes the same way `OrzmaLastSize` guarded the old single-terminal path.
#[derive(Component, Default)]
pub(super) struct PaneLastCells(Option<(u16, u16)>);

/// Whether the active window's `WindowContainer` was resized this frame.
pub(crate) fn window_container_computed_changed(
    containers: Query<(), (With<WindowContainer>, Changed<ComputedNode>)>,
) -> bool {
    !containers.is_empty()
}

/// Whether the active window's layout tree changed this frame — a split,
/// removal, resize, zoom, or the tree's first insertion at bootstrap.
pub(crate) fn active_layout_tree_changed(
    windows: Query<
        (),
        (
            With<ActiveMultiplexerWindow>,
            Changed<MultiplexerLayoutComp>,
        ),
    >,
) -> bool {
    !windows.is_empty()
}

/// Recomputes every pane's rect from the active window's layout tree and the
/// active `WindowContainer`'s computed size, writing each pane's `Node` and
/// resizing its PTY only when the value actually differs.
fn apply_layout(
    mut commands: Commands,
    mut panes: Query<
        (
            Entity,
            &mut Node,
            &mut TerminalHandle,
            &mut PtyHandle,
            &mut Coalescer,
            &mut PaneLastCells,
        ),
        With<MultiplexerPane>,
    >,
    active_windows: Query<(Entity, &MultiplexerLayoutComp), With<ActiveMultiplexerWindow>>,
    containers: Query<(&WindowContainer, &ComputedNode)>,
    metrics: Res<TerminalCellMetricsResource>,
) {
    let Ok((window, layout)) = active_windows.single() else {
        return;
    };
    let Some((_, computed)) = containers.iter().find(|(c, _)| c.window == window) else {
        return;
    };
    let area = PaneRect {
        x: 0.0,
        y: 0.0,
        w: computed.size.x,
        h: computed.size.y,
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);

    for (pane, display) in layout.0.display_rects(area, PANE_GAP_PX) {
        let Ok((entity, mut node, mut handle, mut pty, mut coalescer, mut last_cells)) =
            panes.get_mut(pane)
        else {
            continue;
        };
        let Some(rect) = display else {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        // NOTE: this Display::Flex restore must run BEFORE the last_cells
        // early-continue below. On un-zoom a pane's rect is unchanged (only
        // the zoom flag cleared), so cols/rows still match last_cells and the
        // loop continues right after — if the restore were placed after that
        // guard, an un-zoomed pane would stay Display::None forever.
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
        apply_rect(&mut node, rect, computed.inverse_scale_factor);
        let (cols, rows) = pane_cells(rect, cell_w, cell_h);
        if last_cells.0 == Some((cols, rows)) {
            continue;
        }
        match handle.resize(&mut pty, &mut coalescer, cols, rows) {
            Ok(()) => {
                last_cells.0 = Some((cols, rows));
                handle.emit_pending(&mut commands, entity);
            }
            Err(e) => {
                tracing::warn!(?e, cols, rows, ?entity, "failed to resize multiplexer pane");
            }
        }
    }
}

/// Writes `rect` (physical px) into `node`'s absolute-position fields as
/// `Val::Px` (a logical-px unit — the layout system multiplies it back by the
/// node's scale factor), one field at a time, only where the value actually
/// differs.
fn apply_rect(node: &mut Node, rect: PaneRect, inverse_scale_factor: f32) {
    let left = Val::Px(rect.x * inverse_scale_factor);
    let top = Val::Px(rect.y * inverse_scale_factor);
    let width = Val::Px(rect.w * inverse_scale_factor);
    let height = Val::Px(rect.h * inverse_scale_factor);
    if node.left != left {
        node.left = left;
    }
    if node.top != top {
        node.top = top;
    }
    if node.width != width {
        node.width = width;
    }
    if node.height != height {
        node.height = height;
    }
}

/// Converts a pane's pixel rect into terminal cell dimensions.
fn pane_cells(r: PaneRect, cell_w: f32, cell_h: f32) -> (u16, u16) {
    cells_for(
        r.w.floor() as u32,
        r.h.floor() as u32,
        cell_w.floor().max(1.0),
        cell_h.floor().max(1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_rect_to_cells_matches_cells_for() {
        let r = PaneRect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        };
        let (cols, rows) = pane_cells(r, 8.0, 16.0);
        assert_eq!((cols, rows), cells_for(800, 600, 8.0, 16.0));
    }

    #[test]
    fn pane_last_cells_starts_none() {
        assert!(PaneLastCells::default().0.is_none());
    }

    #[test]
    fn apply_rect_converts_physical_px_to_logical_px() {
        let mut node = Node::default();
        let rect = PaneRect {
            x: 10.0,
            y: 20.0,
            w: 800.0,
            h: 600.0,
        };
        // A HiDPI scale factor of 2.0 halves physical px down to logical px.
        apply_rect(&mut node, rect, 0.5);
        assert_eq!(node.left, Val::Px(5.0));
        assert_eq!(node.top, Val::Px(10.0));
        assert_eq!(node.width, Val::Px(400.0));
        assert_eq!(node.height, Val::Px(300.0));
    }

    #[test]
    fn window_container_computed_changed_gates_on_real_change_only() {
        use bevy::math::Vec2;

        #[derive(Resource, Default)]
        struct RunCount(u32);

        fn probe(mut count: ResMut<RunCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<RunCount>();
        app.add_systems(Update, probe.run_if(window_container_computed_changed));

        let window = app.world_mut().spawn_empty().id();
        let container = app
            .world_mut()
            .spawn((WindowContainer { window }, ComputedNode::DEFAULT))
            .id();

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a freshly-spawned ComputedNode counts as changed"
        );

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "an unchanged ComputedNode must not re-trigger the gate"
        );

        app.world_mut()
            .entity_mut(container)
            .get_mut::<ComputedNode>()
            .unwrap()
            .size = Vec2::new(10.0, 20.0);
        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            2,
            "mutating ComputedNode.size must re-trigger the gate"
        );
    }

    #[test]
    fn active_layout_tree_changed_gates_on_real_change_only() {
        use crate::multiplexer::layout::MultiplexerLayout;

        #[derive(Resource, Default)]
        struct RunCount(u32);

        fn probe(mut count: ResMut<RunCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<RunCount>();
        app.add_systems(Update, probe.run_if(active_layout_tree_changed));

        let pane = app.world_mut().spawn_empty().id();
        let window = app
            .world_mut()
            .spawn((
                MultiplexerLayoutComp(MultiplexerLayout::new(pane)),
                ActiveMultiplexerWindow,
            ))
            .id();

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a freshly-inserted MultiplexerLayoutComp counts as changed"
        );

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "an untouched layout tree must not re-trigger the gate"
        );

        app.world_mut()
            .entity_mut(window)
            .get_mut::<MultiplexerLayoutComp>()
            .unwrap()
            .0
            .set_zoom(Some(pane));
        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            2,
            "mutating the layout tree must re-trigger the gate"
        );
    }
}
