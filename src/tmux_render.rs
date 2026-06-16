//! Render layer for tmux panes: attaches a PTY-less `TerminalHandle` plus the
//! GPU render bundle to each projected `TmuxPane`, then routes tmux `%output`
//! into the handle. Lives in the binary so `ozmux_tmux` stays renderer-free.

use crate::osc_webview::OscWebviewGate;
use crate::ui::WorkspaceUiRoot;
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_tty_engine::{TerminalHandle, TerminalTitle};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use ozma_tty_renderer::schema::TerminalGrid;
use ozmux_tmux::{
    ActiveWindow, PaneOutput, TmuxConnection, TmuxPane, TmuxProjectionSet, TmuxWindow,
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
                route_tmux_output.run_if(on_message::<PaneOutput>),
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
                display: Display::Flex,
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
/// here — the projection observers already establish the correct
/// `ChildOf(window)` parent.
/// `layout_tmux_panes` sets the real rect every frame.
fn attach_tmux_pane_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    gate: Option<Res<OscWebviewGate>>,
    panes: Query<(Entity, &TmuxPane), Without<TerminalHandle>>,
) {
    // NOTE: clone the SHARED OscWebviewGate so a tmux pane captures OSC 5379 when
    // the feature is enabled; a fresh `false` atomic would leave inline-webview
    // capture permanently off for tmux panes. The fallback is only reached in
    // tests that do not install the gate resource.
    let gate = gate
        .map(|g| g.0.clone())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    for (entity, pane) in panes.iter() {
        let (cols, rows) = grid_dims(pane.dims.width, pane.dims.height);
        let handle = TerminalHandle::detached(cols, rows, gate.clone());
        let material = materials.add(TerminalUiMaterial::default());
        commands.entity(entity).insert((
            handle,
            TerminalTitle::default(),
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
///
/// The per-pane alacritty handle is display-only: tmux is the real terminal and
/// already answers the program's device queries (DSR/DA) itself. So this drains
/// the handle's reply queue and discards it — see the `take_replies` `NOTE`.
fn route_tmux_output(
    mut commands: Commands,
    mut reader: MessageReader<PaneOutput>,
    mut handles: Query<(&mut TerminalHandle, &mut TerminalTitle)>,
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
        let Ok((mut handle, mut title)) = handles.get_mut(entity) else {
            continue;
        };
        handle.advance(&data);
        // NOTE: drain captured control frames — crucially the OSC 5379
        // mount/unmount verbs — into observer triggers. The engine's own control
        // drain runs only for PtyHandle-backed terminals; tmux panes have no
        // PtyHandle, so without this call a pane program's inline-webview mount
        // OSC is parsed and then silently dropped (no webview ever mounts).
        handle.drain_control_events(&mut commands, entity, &mut title);
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
            handle.emit_pending(&mut commands, entity);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_renderer::prelude::TerminalGridPlugin;
    use ozmux_tmux::PaneOutput;
    use tmux_control_parser::{CellDims, PaneId};

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
    fn mount_inline_osc_from_pane_triggers_webview_request() {
        use ozma_tty_engine::OscWebviewRequest;

        #[derive(Resource, Default)]
        struct Seen(u32);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_message::<PaneOutput>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.insert_resource(OscWebviewGate(Arc::new(AtomicBool::new(true))));
        app.init_resource::<Seen>();
        app.add_observer(|_ev: On<OscWebviewRequest>, mut seen: ResMut<Seen>| {
            seen.0 += 1;
        });

        let pane_id = PaneId(1);
        app.world_mut().spawn(TmuxPane {
            id: pane_id,
            dims: dims(),
        });

        app.add_systems(
            Update,
            (
                attach_tmux_pane_terminal,
                route_tmux_output.run_if(on_message::<PaneOutput>),
            )
                .chain(),
        );

        // Frame 1: attach the handle + TerminalTitle (no output yet).
        app.update();

        // Frame 2: deliver a mount-inline OSC and route it.
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"\x1b]5379;mount-inline;memo;3;10\x1b\\".to_vec(),
            });
        app.update();

        assert_eq!(
            app.world().resource::<Seen>().0,
            1,
            "a mount-inline OSC from a tmux pane must trigger exactly one OscWebviewRequest",
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
}
