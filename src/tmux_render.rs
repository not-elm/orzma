//! Render layer for tmux panes: attaches a PTY-less `TerminalHandle` plus the
//! GPU render bundle to each projected `TmuxPane`, then routes tmux `%output`
//! into the handle. Lives in the binary so `ozmux_tmux` stays renderer-free.

use crate::ui::WorkspaceUiRoot;
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use ozma_tty_renderer::schema::TerminalGrid;
use ozmux_tmux::{
    PaneOutput, TmuxConnection, TmuxPane, TmuxProjection, TmuxProjectionSet, TmuxWindow,
    refresh_client_command,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Resource, Default)]
struct LastClientSize {
    cols: u16,
    rows: u16,
}

/// Wires the tmux pane render systems after the projection chain.
pub struct OzmuxTmuxRenderPlugin;

impl Plugin for OzmuxTmuxRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastClientSize>();
        app.add_systems(
            Update,
            (
                attach_tmux_window_container,
                attach_tmux_pane_terminal,
                route_tmux_output,
                sync_active_window,
                layout_tmux_panes,
            )
                .chain()
                .after(TmuxProjectionSet),
        );
        app.add_systems(Update, sync_client_size.after(TmuxProjectionSet));
    }
}

fn attach_tmux_window_container(
    mut commands: Commands,
    windows: Query<Entity, (With<TmuxWindow>, Without<Node>)>,
    ui_root: Query<Entity, With<WorkspaceUiRoot>>,
) {
    let Ok(root) = ui_root.single() else {
        return;
    };
    for window in windows.iter() {
        commands.entity(window).insert((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ChildOf(root),
        ));
    }
}

/// Attaches a detached `TerminalHandle`, a `TerminalRenderBundle`, and a
/// placeholder absolute `Node` to each `TmuxPane` that lacks a
/// `TerminalHandle`. Runs every frame but targets each pane exactly once.
/// The grid is sized from the pane's projected `dims`. `ChildOf` is NOT set
/// here — `reconcile` already establishes the correct `ChildOf(window)` parent.
/// `layout_tmux_panes` sets the real rect every frame.
fn attach_tmux_pane_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    panes: Query<(Entity, &TmuxPane), Without<TerminalHandle>>,
) {
    for (entity, pane) in panes.iter() {
        let cols = pane.dims.width.max(1) as u16;
        let rows = pane.dims.height.max(1) as u16;
        let handle = TerminalHandle::detached(cols, rows, Arc::new(AtomicBool::new(false)));
        let material = materials.add(TerminalUiMaterial::default());
        commands.entity(entity).insert((
            handle,
            TerminalRenderBundle::new(material),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
        ));
    }
}

/// Routes tmux `%output` into each pane's handle. Groups a frame's
/// `PaneOutput` messages by pane, advances all of a pane's bytes, then emits
/// once per pane (immediate emit, coalesced per pane).
fn route_tmux_output(
    mut commands: Commands,
    mut reader: MessageReader<PaneOutput>,
    mut handles: Query<&mut TerminalHandle>,
    index: Res<TmuxProjection>,
) {
    let mut by_pane: HashMap<_, Vec<u8>> = HashMap::new();
    for msg in reader.read() {
        by_pane
            .entry(msg.pane)
            .or_default()
            .extend_from_slice(&msg.data);
    }
    for (pane, data) in by_pane {
        let Some(&entity) = index.panes.get(&pane) else {
            continue;
        };
        let Ok(mut handle) = handles.get_mut(entity) else {
            continue;
        };
        handle.advance(&data);
        handle.flush_emit(&mut commands, entity);
        // TODO: Phase 3 — forward handle.take_replies() (DSR/DA answers) back
        // to tmux as pane input; in Phase 2a they are intentionally dropped.
    }
}

fn pane_rect(
    xoff: i32,
    yoff: i32,
    width: u32,
    height: u32,
    cell_w: f32,
    cell_h: f32,
) -> (f32, f32, f32, f32) {
    (
        xoff as f32 * cell_w,
        yoff as f32 * cell_h,
        width as f32 * cell_w,
        height as f32 * cell_h,
    )
}

fn layout_tmux_panes(
    mut commands: Commands,
    mut panes: Query<(
        Entity,
        &TmuxPane,
        &mut Node,
        &mut TerminalHandle,
        &mut TerminalGrid,
    )>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let dpr = window.scale_factor().max(0.5);
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0) / dpr;
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0) / dpr;
    for (entity, pane, mut node, mut handle, mut grid) in panes.iter_mut() {
        let d = pane.dims;
        let (left, top, width, height) =
            pane_rect(d.xoff, d.yoff, d.width, d.height, cell_w, cell_h);
        node.left = Val::Px(left);
        node.top = Val::Px(top);
        node.width = Val::Px(width);
        node.height = Val::Px(height);
        let cols = pane.dims.width.max(1) as u16;
        let rows = pane.dims.height.max(1) as u16;
        let (cur_cols, cur_rows, _) = handle.read_geometry();
        if (cur_cols, cur_rows) != (cols, rows) {
            handle.resize_grid_only(cols, rows);
            grid.cols = cols;
            grid.rows = rows;
            handle.flush_emit(&mut commands, entity);
        }
    }
}

fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}

fn sync_client_size(
    mut last: ResMut<LastClientSize>,
    connection: NonSend<TmuxConnection>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    let Ok(window) = window.single() else {
        return;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let (cols, rows) = cells_for(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        cell_w,
        cell_h,
    );
    if (cols, rows) == (last.cols, last.rows) {
        return;
    }
    last.cols = cols;
    last.rows = rows;
    if let Err(e) = client.handle().send(&refresh_client_command(cols, rows)) {
        tracing::warn!(?e, cols, rows, "refresh-client send failed");
    }
}

fn sync_active_window(mut windows: Query<(&TmuxWindow, &mut Node)>) {
    for (w, mut node) in windows.iter_mut() {
        let want = if w.active {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_renderer::prelude::TerminalGridPlugin;
    use ozmux_tmux::PaneOutput;
    use tmux_control_parser::{CellDims, PaneId};

    #[test]
    fn cells_for_divides_and_floors() {
        assert_eq!(cells_for(800, 600, 8.0, 16.0), (100, 37));
        assert_eq!(cells_for(1, 1, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(0, 0, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(807, 607, 8.0, 16.0), (100, 37));
    }

    #[test]
    fn pane_rect_scales_cell_dims_to_pixels() {
        let dims = CellDims {
            width: 10,
            height: 4,
            xoff: 2,
            yoff: 1,
        };
        assert_eq!(
            pane_rect(dims.xoff, dims.yoff, dims.width, dims.height, 8.0, 16.0),
            (16.0, 16.0, 80.0, 64.0),
        );
    }

    fn dims() -> CellDims {
        CellDims {
            width: 20,
            height: 5,
            xoff: 0,
            yoff: 0,
        }
    }

    #[test]
    fn output_routed_into_pane_grid_renders_text() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.init_resource::<TmuxProjection>();
        app.add_message::<PaneOutput>();

        // A projected pane entity + its index mapping.
        let pane_id = PaneId(1);
        let pane_entity = app
            .world_mut()
            .spawn(TmuxPane {
                id: pane_id,
                dims: dims(),
            })
            .id();
        app.world_mut()
            .resource_mut::<TmuxProjection>()
            .panes
            .insert(pane_id, pane_entity);

        app.add_systems(
            Update,
            (attach_tmux_pane_terminal, route_tmux_output).chain(),
        );

        // Frame 1: attach the handle (no output yet).
        app.update();
        assert!(
            app.world().get::<TerminalHandle>(pane_entity).is_some(),
            "handle attached on first frame",
        );

        // Frame 2: deliver output and route it.
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"hi".to_vec(),
            });
        app.update();

        let grid = app
            .world()
            .get::<TerminalGrid>(pane_entity)
            .expect("pane has a TerminalGrid");
        let row0: String = grid.cells[0].iter().map(|c| c.text.as_str()).collect();
        assert!(
            row0.starts_with("hi"),
            "rendered grid row 0 should start with 'hi', got {row0:?}",
        );
    }

    #[test]
    fn resize_only_updates_grid_dims_and_emits() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};
        use ozma_tty_renderer::schema::FrameSnapshot;
        use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};

        #[derive(Resource, Default)]
        struct SnapHits(u32);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<SnapHits>();
        app.add_observer(|_snap: On<FrameSnapshot>, mut hits: ResMut<SnapHits>| {
            hits.0 += 1;
        });
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 13.0,
                descent_phys: 3.0,
                underline_position_phys: -1.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        });
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        let entity = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 40,
                        height: 10,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                Node::default(),
                TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
                TerminalGrid::default(),
            ))
            .id();

        app.add_systems(Update, layout_tmux_panes);
        app.update();

        let grid = app
            .world()
            .get::<TerminalGrid>(entity)
            .expect("pane has a TerminalGrid");
        assert_eq!(
            (grid.cols, grid.rows),
            (40, 10),
            "grid dims updated immediately on resize (#2)",
        );
        assert!(
            app.world().resource::<SnapHits>().0 >= 1,
            "resize emitted a FrameSnapshot (#1)",
        );
    }
}
