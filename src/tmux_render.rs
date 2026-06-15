//! Render layer for tmux panes: attaches a PTY-less `TerminalHandle` plus the
//! GPU render bundle to each projected `TmuxPane`, then routes tmux `%output`
//! into the handle. Lives in the binary so `ozmux_tmux` stays renderer-free.

use crate::theme;
use crate::ui::WorkspaceUiRoot;
use bevy::ecs::message::MessageReader;
use bevy::math::Rect;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::material::{TerminalBgPadding, TerminalUiMaterial};
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use ozma_tty_renderer::schema::TerminalGrid;
use ozmux_tmux::{
    ActivePane, ActiveWindow, PaneOutput, TmuxConnection, TmuxPane, TmuxProjectionSet, TmuxWindow,
    TmuxWindowLayout, refresh_client_command,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tmux_control_parser::{Cell, PaneId, SplitDir};

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
        // Over-sized split-filling panes paint padding beyond their grid (see
        // `layout_panes`). Match it to the terminal's default background so no
        // band shows — `ozma_tty_engine` maps `NamedColor::Background` to black.
        app.insert_resource(TerminalBgPadding(LinearRgba::BLACK));
        app.add_systems(
            Update,
            (
                attach_tmux_window_container,
                attach_tmux_pane_terminal,
                route_tmux_output.run_if(on_message::<PaneOutput>),
                sync_active_window,
                layout_tmux_panes,
                sync_active_pane_outline,
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
                ..default()
            },
            BackgroundColor(theme::BORDER),
            ChildOf(root),
        ));
    }
}

/// Attaches a detached `TerminalHandle`, a `TerminalRenderBundle`, and a
/// placeholder absolute `Node` to each `TmuxPane` that lacks a
/// `TerminalHandle`. Runs every frame but targets each pane exactly once.
/// The grid is sized from the pane's projected `dims`. `ChildOf` is NOT set
/// here — the projection observers already establish the correct
/// `ChildOf(window)` parent.
/// `layout_tmux_panes` sets the real rect every frame.
fn attach_tmux_pane_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    panes: Query<(Entity, &TmuxPane), Without<TerminalHandle>>,
) {
    for (entity, pane) in panes.iter() {
        let (cols, rows) = grid_dims(pane.dims.width, pane.dims.height);
        let handle = TerminalHandle::detached(cols, rows, Arc::new(AtomicBool::new(false)));
        let material = materials.add(TerminalUiMaterial::default());
        commands.entity(entity).insert((
            handle,
            TerminalRenderBundle::new(material),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Outline::new(Val::Px(theme::PANE_BORDER_PX), Val::Px(0.0), Color::NONE),
        ));
    }
}

/// Routes tmux `%output` into each pane's handle. Groups a frame's
/// `PaneOutput` messages by pane, advances all of a pane's bytes, then emits
/// once per pane (immediate emit, coalesced per pane).
///
/// The per-pane alacritty handle is display-only: tmux is the real terminal and
/// already answers the program's device queries (DSR/DA) itself. So this drains
/// the handle's reply queue and discards it — see the `take_replies` `NOTE`.
fn route_tmux_output(
    mut commands: Commands,
    mut reader: MessageReader<PaneOutput>,
    mut handles: Query<&mut TerminalHandle>,
    panes: Query<(Entity, &TmuxPane)>,
) {
    let mut by_pane: HashMap<_, Vec<u8>> = HashMap::new();
    for msg in reader.read() {
        by_pane
            .entry(msg.pane)
            .or_default()
            .extend_from_slice(&msg.data);
    }
    let entity_of: HashMap<_, _> = panes.iter().map(|(e, p)| (p.id, e)).collect();
    for (pane, data) in by_pane {
        let Some(&entity) = entity_of.get(&pane) else {
            continue;
        };
        let Ok(mut handle) = handles.get_mut(entity) else {
            continue;
        };
        handle.advance(&data);
        handle.flush_emit(&mut commands, entity);
        // NOTE: drain and DISCARD — never forward these replies to tmux. The
        // handle is a display-only renderer; tmux already answered the program's
        // DSR/DA query. Injecting alacritty's duplicate answer via `send-keys -H`
        // delivers it to the program as phantom keystrokes (e.g. the DSR-OK reply
        // `ESC[0n` makes readline self-insert a stray `n` and desyncs arrow-key
        // history recall). Draining still matters: the reply channel is unbounded.
        let _ = handle.take_replies();
    }
}

/// Converts tmux cell dims (`u32`) to an alacritty grid size, clamping into
/// `1..=u16::MAX` so a pathological width/height cannot truncate to 0 (a 0-col
/// `Term::resize` would panic).
fn grid_dims(width: u32, height: u32) -> (u16, u16) {
    let clamp = |v: u32| v.clamp(1, u16::MAX as u32) as u16;
    (clamp(width), clamp(height))
}

/// The axis a split distributes its children along.
#[derive(Clone, Copy)]
enum Axis {
    Horizontal,
    Vertical,
}

/// Lays every pane in `root` into `rect` (logical px), filling it exactly. Each
/// split distributes its rect among children in proportion to their tmux cell
/// extent, separated by `gap`-px dividers, and children fill the cross axis —
/// so sibling subtrees always align regardless of internal nesting (no
/// reserved-separator height/width mismatch). Returns each pane's rect keyed by
/// tmux pane id. A pane's rect is >= its cell content size; the small leftover
/// renders as terminal-background padding inside the pane (see
/// `TerminalBgPadding`).
fn layout_panes(
    root: &Cell,
    rect: Rect,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
) -> HashMap<PaneId, Rect> {
    let mut out = HashMap::new();
    place(&mut out, root, rect, cell_w, cell_h, gap);
    out
}

/// Places `cell` into `rect`, recording leaf rects into `out`. The Floating arm
/// diverges: popups are positioned at their absolute tmux `xoff`/`yoff` and
/// sized to their cell content, floating over the layout rather than tiling.
fn place(
    out: &mut HashMap<PaneId, Rect>,
    cell: &Cell,
    rect: Rect,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
) {
    match cell {
        Cell::Leaf { pane_id, .. } => {
            if let Some(id) = pane_id {
                out.insert(PaneId(*id), rect);
            }
        }
        Cell::Split {
            dir: SplitDir::LeftRight,
            children,
            ..
        } => split_axis(out, children, rect, cell_w, cell_h, gap, Axis::Horizontal),
        Cell::Split {
            dir: SplitDir::TopBottom,
            children,
            ..
        } => split_axis(out, children, rect, cell_w, cell_h, gap, Axis::Vertical),
        Cell::Split {
            dir: SplitDir::Floating,
            children,
            ..
        } => {
            for child in children {
                let d = child.dims();
                let min = Vec2::new(d.xoff as f32 * cell_w, d.yoff as f32 * cell_h);
                let max = min + Vec2::new(d.width as f32 * cell_w, d.height as f32 * cell_h);
                place(
                    out,
                    child,
                    Rect::from_corners(min, max),
                    cell_w,
                    cell_h,
                    gap,
                );
            }
        }
    }
}

/// Distributes `rect` among `children` along `axis`, proportional to each
/// child's tmux cell extent on that axis, with a `gap`-px divider between
/// siblings. Children fill the cross axis. The last child absorbs rounding so
/// the children exactly tile `rect` (no sub-pixel gap or overlap).
fn split_axis(
    out: &mut HashMap<PaneId, Rect>,
    children: &[Cell],
    rect: Rect,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
    axis: Axis,
) {
    let n = children.len();
    if n == 0 {
        return;
    }
    let weight = |c: &Cell| match axis {
        Axis::Horizontal => c.dims().width as f32,
        Axis::Vertical => c.dims().height as f32,
    };
    let sum: f32 = children.iter().map(weight).sum::<f32>().max(1.0);
    let (total, start, end) = match axis {
        Axis::Horizontal => (rect.width(), rect.min.x, rect.max.x),
        Axis::Vertical => (rect.height(), rect.min.y, rect.max.y),
    };
    let avail = (total - gap * (n - 1) as f32).max(0.0);
    let mut pos = start;
    for (i, child) in children.iter().enumerate() {
        let extent = if i == n - 1 {
            end - pos
        } else {
            (avail * weight(child) / sum).round()
        };
        let child_rect = match axis {
            Axis::Horizontal => Rect::from_corners(
                Vec2::new(pos, rect.min.y),
                Vec2::new(pos + extent, rect.max.y),
            ),
            Axis::Vertical => Rect::from_corners(
                Vec2::new(rect.min.x, pos),
                Vec2::new(rect.max.x, pos + extent),
            ),
        };
        place(out, child, child_rect, cell_w, cell_h, gap);
        pos += extent + gap;
    }
}

fn layout_tmux_panes(
    mut commands: Commands,
    mut windows: Query<(&TmuxWindowLayout, &mut Node, &Children), With<TmuxWindow>>,
    mut panes: Query<
        (&TmuxPane, &mut Node, &mut TerminalHandle, &mut TerminalGrid),
        Without<TmuxWindow>,
    >,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let dpr = window.scale_factor().max(0.5);
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);
    let cell_w = cell_w_phys / dpr;
    let cell_h = cell_h_phys / dpr;

    // The pane area handed to tmux (cols × rows-1), in logical px — the same
    // budget `sync_client_size` sends via `refresh-client`.
    let (cols, total_rows) = cells_for(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        cell_w_phys,
        cell_h_phys,
    );
    let area_w = cols as f32 * cell_w;
    let area_h = rows_for_panes(total_rows) as f32 * cell_h;

    let root_rect = Rect::from_corners(Vec2::ZERO, Vec2::new(area_w, area_h));

    for (layout, mut container, children) in windows.iter_mut() {
        let rects = layout_panes(
            &layout.0.root,
            root_rect,
            cell_w,
            cell_h,
            theme::PANE_GAP_PX,
        );

        // The grey container fills the full pane area; panes tile it exactly, so
        // the only grey that shows is the 1px divider between split children.
        if container.width != Val::Px(area_w) || container.height != Val::Px(area_h) {
            container.width = Val::Px(area_w);
            container.height = Val::Px(area_h);
        }

        for &child in children {
            let Ok((pane, mut node, mut handle, mut grid)) = panes.get_mut(child) else {
                continue;
            };
            let Some(rect) = rects.get(&pane.id) else {
                continue;
            };
            let left = rect.min.x;
            let top = rect.min.y;
            let width = rect.width();
            let height = rect.height();
            // NOTE: only write the Node fields when they actually change — writing
            // through `Mut<Node>` every frame would mark the component changed and
            // force a full UI relayout pass each tick even when nothing moved.
            if node.left != Val::Px(left)
                || node.top != Val::Px(top)
                || node.width != Val::Px(width)
                || node.height != Val::Px(height)
            {
                node.left = Val::Px(left);
                node.top = Val::Px(top);
                node.width = Val::Px(width);
                node.height = Val::Px(height);
            }
            let (cols, rows) = grid_dims(pane.dims.width, pane.dims.height);
            let (cur_cols, cur_rows, _) = handle.read_geometry();
            if (cur_cols, cur_rows) != (cols, rows) {
                handle.resize_grid_only(cols, rows);
                grid.cols = cols;
                grid.rows = rows;
                handle.emit_pending(&mut commands, child);
            }
        }
    }
}

fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}

fn rows_for_panes(total_rows: u16) -> u16 {
    total_rows.saturating_sub(1).max(1)
}

/// Sends `refresh-client -C <cols>,<rows>` to tmux so it lays out panes for
/// the current window size. One row is reserved for the ozmux window status
/// bar via [`rows_for_panes`], since tmux `-CC` does not reserve a status row.
/// Results are deduped via [`LastClientSize`].
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
    let rows = rows_for_panes(rows);
    if (cols, rows) == (last.cols, last.rows) {
        return;
    }
    // NOTE: only record the size as sent AFTER a successful send — otherwise a
    // transient send failure would poison the dedupe and permanently suppress
    // re-sending this size, leaving tmux stuck at the stale client dimensions.
    match client.handle().send(&refresh_client_command(cols, rows)) {
        Ok(_) => {
            last.cols = cols;
            last.rows = rows;
        }
        Err(e) => tracing::warn!(?e, cols, rows, "refresh-client send failed"),
    }
}

fn sync_active_window(mut windows: Query<(&mut Node, Has<ActiveWindow>), With<TmuxWindow>>) {
    for (mut node, active) in windows.iter_mut() {
        let want = if active { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
}

/// Recolors each pane's `Outline`: the accent color on the pane carrying
/// `ActivePane`, transparent otherwise. Recoloring (not insert/remove) avoids
/// ECS table moves on every active-pane change.
fn sync_active_pane_outline(mut panes: Query<(Has<ActivePane>, &mut Outline), With<TmuxPane>>) {
    for (active, mut outline) in panes.iter_mut() {
        let want = if active { theme::ACCENT } else { Color::NONE };
        if outline.color != want {
            outline.color = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_renderer::prelude::TerminalGridPlugin;
    use ozmux_tmux::PaneOutput;
    use tmux_control_parser::CellDims;

    #[test]
    fn rows_for_panes_reserves_one_row_for_the_bar() {
        assert_eq!(rows_for_panes(24), 23);
        assert_eq!(rows_for_panes(1), 1); // never zero
        assert_eq!(rows_for_panes(2), 1);
    }

    #[test]
    fn cells_for_divides_and_floors() {
        assert_eq!(cells_for(800, 600, 8.0, 16.0), (100, 37));
        assert_eq!(cells_for(1, 1, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(0, 0, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(807, 607, 8.0, 16.0), (100, 37));
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
    fn active_pane_outline_is_accent_inactive_is_none() {
        use bevy::prelude::*;
        use ozmux_tmux::{ActivePane, TmuxPane};

        let mut app = App::new();
        app.add_systems(Update, sync_active_pane_outline);
        let active = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: dims(),
                },
                ActivePane,
                Outline::new(Val::Px(1.0), Val::Px(0.0), Color::NONE),
            ))
            .id();
        let inactive = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(2),
                    dims: dims(),
                },
                Outline::new(Val::Px(1.0), Val::Px(0.0), Color::NONE),
            ))
            .id();
        app.update();

        assert_eq!(
            app.world().get::<Outline>(active).unwrap().color,
            theme::ACCENT
        );
        assert_eq!(
            app.world().get::<Outline>(inactive).unwrap().color,
            Color::NONE
        );
    }

    #[test]
    fn output_routed_into_pane_grid_renders_text() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_message::<PaneOutput>();
        app.insert_non_send_resource(TmuxConnection::default());

        let pane_id = PaneId(1);
        let pane_entity = app
            .world_mut()
            .spawn(TmuxPane {
                id: pane_id,
                dims: dims(),
            })
            .id();

        app.add_systems(
            Update,
            (
                attach_tmux_pane_terminal,
                route_tmux_output.run_if(on_message::<PaneOutput>),
            )
                .chain(),
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
        use tmux_control_parser::{WindowId, WindowLayout};

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
        let window_e = app
            .world_mut()
            .spawn((
                TmuxWindow {
                    id: WindowId(1),
                    index: 0,
                    name: String::new(),
                },
                TmuxWindowLayout(WindowLayout::parse(b"abcd,40x10,0,0,1").unwrap()),
                Node::default(),
            ))
            .id();
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
                ChildOf(window_e),
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

    #[test]
    fn layout_packs_two_panes_and_sizes_container() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};
        use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};
        use tmux_control_parser::{WindowId, WindowLayout};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
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
        let window_e = app
            .world_mut()
            .spawn((
                TmuxWindow {
                    id: WindowId(1),
                    index: 0,
                    name: String::new(),
                },
                TmuxWindowLayout(
                    WindowLayout::parse(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").unwrap(),
                ),
                Node::default(),
            ))
            .id();
        let pane1 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 40,
                        height: 24,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                Node::default(),
                TerminalHandle::detached(40, 24, Arc::new(AtomicBool::new(false))),
                TerminalGrid::default(),
                ChildOf(window_e),
            ))
            .id();
        let pane2 = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(2),
                    dims: CellDims {
                        width: 39,
                        height: 24,
                        xoff: 41,
                        yoff: 0,
                    },
                },
                Node::default(),
                TerminalHandle::detached(39, 24, Arc::new(AtomicBool::new(false))),
                TerminalGrid::default(),
                ChildOf(window_e),
            ))
            .id();

        app.add_systems(Update, layout_tmux_panes);
        app.update();

        let container = app
            .world()
            .get::<Node>(window_e)
            .expect("window container has a Node");
        // Container fills the full pane area (cols 100 × cell_w 8 = 800;
        // rows-1 = 36 × cell_h 16 = 576). Panes tile it exactly.
        assert_eq!(
            container.width,
            Val::Px(800.0),
            "container fills pane-area width"
        );
        assert_eq!(
            container.height,
            Val::Px(576.0),
            "container fills pane-area height"
        );

        // weights 40,39 over avail 799 → 405, last absorbs remainder; 1px
        // divider at x=405..406; both panes fill the full 576 height.
        let p1 = app.world().get::<Node>(pane1).expect("pane1 has a Node");
        assert_eq!(p1.left, Val::Px(0.0), "pane1 left");
        assert_eq!(p1.top, Val::Px(0.0), "pane1 top");
        assert_eq!(p1.width, Val::Px(405.0), "pane1 width");
        assert_eq!(p1.height, Val::Px(576.0), "pane1 height");

        let p2 = app.world().get::<Node>(pane2).expect("pane2 has a Node");
        assert_eq!(p2.left, Val::Px(406.0), "pane2 left (405 + 1px divider)");
        assert_eq!(p2.top, Val::Px(0.0), "pane2 top");
        assert_eq!(p2.width, Val::Px(394.0), "pane2 width");
        assert_eq!(
            p2.height,
            Val::Px(576.0),
            "pane2 height (fills, aligned with pane1)"
        );
    }

    #[test]
    #[ignore = "requires a real tmux binary and a controlling PTY"]
    fn display_only_pane_does_not_inject_phantom_device_replies() {
        use ozmux_tmux::{ConnectionState, TmuxSessionPlugin};
        use std::time::{Duration, Instant};
        use tmux_control::TmuxServer;

        #[derive(Resource, Default)]
        struct Captured(Vec<u8>);

        fn capture(mut sink: ResMut<Captured>, mut reader: MessageReader<PaneOutput>) {
            for msg in reader.read() {
                sink.0.extend_from_slice(&msg.data);
            }
        }

        let socket = format!("ozmux-replyloop-{}", std::process::id());
        let server = TmuxServer::new().socket_name(&socket);
        let client = server.new_session().expect("spawn tmux -CC new-session");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_plugins(TmuxSessionPlugin);
        app.init_resource::<Captured>();
        app.add_systems(
            Update,
            (
                attach_tmux_pane_terminal,
                route_tmux_output.run_if(on_message::<PaneOutput>),
                capture,
            )
                .chain()
                .after(TmuxProjectionSet),
        );
        app.world_mut()
            .get_non_send_resource_mut::<TmuxConnection>()
            .expect("TmuxConnection inserted by the plugin")
            .set(client);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut pane_id: Option<PaneId> = None;
        while Instant::now() < deadline {
            app.update();
            if *app.world().resource::<ConnectionState>() == ConnectionState::Attached {
                let mut q = app
                    .world_mut()
                    .query_filtered::<&TmuxPane, With<TerminalHandle>>();
                if let Some(id) = q.iter(app.world()).next().map(|p| p.id) {
                    pane_id = Some(id);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let pane_id = pane_id.expect("a pane should be projected with a TerminalHandle attached");
        let target = format!("%{}", pane_id.0);

        // Run `cat` so the pane TTY is in cooked mode with echo on: any bytes
        // injected into the pane's input are reflected back as %output, making
        // a spurious injected device-report reply observable.
        let handle = app
            .world()
            .get_non_send_resource::<TmuxConnection>()
            .unwrap()
            .client()
            .unwrap()
            .handle();
        handle
            .send(&format!("send-keys -t {target} -l -- cat"))
            .expect("send-keys cat");
        handle
            .send(&format!("send-keys -t {target} Enter"))
            .expect("send-keys Enter");

        let cat_deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < cat_deadline {
            app.update();
            std::thread::sleep(Duration::from_millis(50));
        }
        app.world_mut().resource_mut::<Captured>().0.clear();

        // NOTE: inject the DSR-5 query as a *synthetic* %output — tmux never saw
        // a real query, so it answers nothing. Any `[0n` (the DSR-OK reply)
        // echoed back can therefore only be ozmux's display-only renderer
        // answering the probe and injecting it into the pane via send-keys -H.
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"\x1b[5n".to_vec(),
            });

        let echo_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < echo_deadline {
            app.update();
            std::thread::sleep(Duration::from_millis(50));
        }

        let (injected, captured_len) = {
            let captured = &app.world().resource::<Captured>().0;
            (captured.windows(3).any(|w| w == b"[0n"), captured.len())
        };

        if let Some(client) = app
            .world_mut()
            .get_non_send_resource_mut::<TmuxConnection>()
            .unwrap()
            .take()
        {
            client.handle().send("kill-server").ok();
        }

        assert!(
            !injected,
            "display-only pane must not answer device queries: an echoed DSR reply ([0n) was \
             injected back into the tmux pane (captured {captured_len} bytes)"
        );
    }

    #[test]
    fn resize_fires_fresh_snapshot_after_first_emit() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};
        use ozma_tty_renderer::schema::FrameSnapshot;
        use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};
        use tmux_control_parser::{WindowId, WindowLayout};

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

        let window_e = app
            .world_mut()
            .spawn((
                TmuxWindow {
                    id: WindowId(1),
                    index: 0,
                    name: String::new(),
                },
                TmuxWindowLayout(WindowLayout::parse(b"abcd,20x5,0,0,2").unwrap()),
                Node::default(),
            ))
            .id();
        let pane_id = PaneId(2);
        let entity = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: pane_id,
                    dims: CellDims {
                        width: 20,
                        height: 5,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                Node::default(),
                TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
                TerminalGrid::default(),
                ChildOf(window_e),
            ))
            .id();

        app.add_systems(
            Update,
            (
                |mut commands: Commands, mut q: Query<(Entity, &mut TerminalHandle)>| {
                    for (e, mut h) in q.iter_mut() {
                        h.advance(b"x");
                        h.flush_emit(&mut commands, e);
                    }
                },
                layout_tmux_panes,
            )
                .chain(),
        );

        // Frame 1: emit once so first_emit flips to false (matching production state).
        app.update();
        let hits_after_first = app.world().resource::<SnapHits>().0;
        assert!(
            hits_after_first >= 1,
            "first flush_emit must fire a FrameSnapshot (first_emit path)"
        );

        // Now change pane dims so layout_tmux_panes will resize.
        app.world_mut()
            .entity_mut(entity)
            .get_mut::<TmuxPane>()
            .unwrap()
            .dims = CellDims {
            width: 40,
            height: 10,
            xoff: 0,
            yoff: 0,
        };

        // Frame 2: layout_tmux_panes sees the dim change, calls resize_grid_only +
        // emit_pending — must fire a NEW FrameSnapshot even though first_emit is now false.
        app.update();
        let hits_after_resize = app.world().resource::<SnapHits>().0;

        let grid = app
            .world()
            .get::<TerminalGrid>(entity)
            .expect("pane has a TerminalGrid");
        assert_eq!(
            (grid.cols, grid.rows),
            (40, 10),
            "grid dims updated on second resize",
        );
        assert!(
            hits_after_resize > hits_after_first,
            "resize must fire a fresh FrameSnapshot when first_emit is already false \
             (got {hits_after_resize} total, was {hits_after_first} before resize)",
        );
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_corners(Vec2::new(x0, y0), Vec2::new(x1, y1))
    }

    #[test]
    fn layout_single_pane_fills_rect() {
        let root = Cell::Leaf {
            dims: CellDims {
                width: 80,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
            pane_id: Some(0),
        };
        let rects = layout_panes(&root, rect(0.0, 0.0, 640.0, 384.0), 8.0, 16.0, 1.0);
        assert_eq!(rects[&PaneId(0)], rect(0.0, 0.0, 640.0, 384.0));
    }

    #[test]
    fn layout_horizontal_split_fills_with_1px_divider() {
        let root = Cell::Split {
            dims: CellDims {
                width: 80,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
            dir: SplitDir::LeftRight,
            children: vec![
                Cell::Leaf {
                    dims: CellDims {
                        width: 40,
                        height: 24,
                        xoff: 0,
                        yoff: 0,
                    },
                    pane_id: Some(1),
                },
                Cell::Leaf {
                    dims: CellDims {
                        width: 39,
                        height: 24,
                        xoff: 41,
                        yoff: 0,
                    },
                    pane_id: Some(2),
                },
            ],
        };
        let rects = layout_panes(&root, rect(0.0, 0.0, 640.0, 384.0), 8.0, 16.0, 1.0);
        // weights 40,39 over avail 639 → 324, last absorbs remainder (315);
        // 1px divider at x=324..325; both panes fill the full height.
        assert_eq!(rects[&PaneId(1)], rect(0.0, 0.0, 324.0, 384.0));
        assert_eq!(rects[&PaneId(2)], rect(325.0, 0.0, 640.0, 384.0));
    }

    #[test]
    fn layout_nested_split_columns_stay_aligned() {
        // LeftRight[ pane1(40x24), TopBottom[ pane2(39x12), pane3(39x11) ] ]
        let root = Cell::Split {
            dims: CellDims {
                width: 80,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
            dir: SplitDir::LeftRight,
            children: vec![
                Cell::Leaf {
                    dims: CellDims {
                        width: 40,
                        height: 24,
                        xoff: 0,
                        yoff: 0,
                    },
                    pane_id: Some(1),
                },
                Cell::Split {
                    dims: CellDims {
                        width: 39,
                        height: 24,
                        xoff: 41,
                        yoff: 0,
                    },
                    dir: SplitDir::TopBottom,
                    children: vec![
                        Cell::Leaf {
                            dims: CellDims {
                                width: 39,
                                height: 12,
                                xoff: 41,
                                yoff: 0,
                            },
                            pane_id: Some(2),
                        },
                        Cell::Leaf {
                            dims: CellDims {
                                width: 39,
                                height: 11,
                                xoff: 41,
                                yoff: 13,
                            },
                            pane_id: Some(3),
                        },
                    ],
                },
            ],
        };
        let rects = layout_panes(&root, rect(0.0, 0.0, 640.0, 384.0), 8.0, 16.0, 1.0);
        assert_eq!(rects[&PaneId(1)], rect(0.0, 0.0, 324.0, 384.0));
        assert_eq!(rects[&PaneId(2)], rect(325.0, 0.0, 640.0, 200.0));
        assert_eq!(rects[&PaneId(3)], rect(325.0, 201.0, 640.0, 384.0));
        // The split column (panes 2+3) and the unsplit column (pane 1) both
        // reach the bottom — no reserved-separator height mismatch.
        assert_eq!(rects[&PaneId(1)].max.y, rects[&PaneId(3)].max.y);
    }

    #[test]
    fn layout_skips_leaf_without_pane_id() {
        let root = Cell::Split {
            dims: CellDims {
                width: 80,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
            dir: SplitDir::LeftRight,
            children: vec![
                Cell::Leaf {
                    dims: CellDims {
                        width: 40,
                        height: 24,
                        xoff: 0,
                        yoff: 0,
                    },
                    pane_id: None,
                },
                Cell::Leaf {
                    dims: CellDims {
                        width: 39,
                        height: 24,
                        xoff: 41,
                        yoff: 0,
                    },
                    pane_id: Some(2),
                },
            ],
        };
        let rects = layout_panes(&root, rect(0.0, 0.0, 640.0, 384.0), 8.0, 16.0, 1.0);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[&PaneId(2)], rect(325.0, 0.0, 640.0, 384.0));
    }

    #[test]
    fn layout_floating_uses_literal_offsets() {
        let root = Cell::Split {
            dims: CellDims {
                width: 80,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
            dir: SplitDir::Floating,
            children: vec![Cell::Leaf {
                dims: CellDims {
                    width: 10,
                    height: 5,
                    xoff: 4,
                    yoff: 2,
                },
                pane_id: Some(7),
            }],
        };
        let rects = layout_panes(&root, rect(0.0, 0.0, 640.0, 384.0), 8.0, 16.0, 1.0);
        assert_eq!(rects[&PaneId(7)], rect(32.0, 32.0, 112.0, 112.0));
    }
}
