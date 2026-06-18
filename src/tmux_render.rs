//! Render layer for tmux panes: attaches a PTY-less `TerminalHandle` plus the
//! GPU render bundle to each projected `TmuxPane`, then routes tmux `%output`
//! into the handle. Lives in the binary so `ozmux_tmux` stays renderer-free.

use crate::osc_webview::OscWebviewGate;
use crate::theme;
use crate::ui::WorkspaceUiRoot;
use bevy::ecs::message::MessageReader;
use bevy::math::Rect;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_tty_engine::{TerminalHandle, TerminalTitle};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use ozma_tty_renderer::schema::TerminalGrid;
use ozmux_tmux::{
    ActiveWindow, PaneOutput, TmuxConnection, TmuxPane, TmuxProjectionSet, TmuxWindow,
    TmuxWindowLayout, WindowId, refresh_client_command, resize_window_command,
    window_refresh_client_command,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tmux_control_parser::{Cell, DividerAxis, PaneId, SplitDir};

#[derive(Resource, Default)]
struct LastClientSize {
    window: Option<WindowId>,
    cols: u16,
    rows: u16,
    last_reported: Option<(u16, u16)>,
}

/// Wires the tmux pane render systems after the projection chain.
pub struct OzmuxTmuxRenderPlugin;

impl Plugin for OzmuxTmuxRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastClientSize>();
        app.insert_resource(ClearColor(theme::PANE_GAP));
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

/// Pixel-space position of a 1px inter-pane gap. Positions are in logical px
/// (the same coordinate space as Bevy `Node` rects).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DividerPixelRect {
    pub(crate) axis: DividerAxis,
    /// Pane on the "before" side; used to identify the resize target.
    pub(crate) primary: PaneId,
    /// Logical-px leading edge of the 1px gap on the major axis.
    pub(crate) pos_px: f32,
    /// Orthogonal-axis start of the divider line in logical px.
    pub(crate) span_start_px: f32,
    /// Orthogonal-axis end of the divider line in logical px.
    pub(crate) span_end_px: f32,
}

/// Pixel-space layout derived from a `TmuxWindowLayout` tree, stored on the
/// window entity by `layout_tmux_panes`.
#[derive(Component, Clone, Default, PartialEq)]
pub(crate) struct PackedTmuxLayout {
    pub(crate) panes: HashMap<PaneId, Rect>,
    pub(crate) dividers: Vec<DividerPixelRect>,
    /// Total layout bounding box in logical px. Width equals root cell width × cell_w.
    /// Height equals the workspace height passed to `collapse()` — the full logical
    /// pixel area available for panes (window height minus window bar, including slack).
    pub(crate) bbox: Vec2,
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
/// placeholder absolute `Node` to each `TmuxPane` that lacks a `TerminalHandle`.
/// The `TerminalGrid` lives on the pane entity itself, so `flush_emit` /
/// `emit_pending` and the inline-webview overlay projection all target it
/// directly (an inline webview mounts as a `ChildOf` the pane).
///
/// Runs every frame but targets each pane exactly once. `ChildOf` is NOT set
/// here — the projection observers already establish the correct
/// `ChildOf(window)` parent. `layout_tmux_panes` sets the real rect every frame.
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
///
/// While a pane is in copy mode the live bytes are still advanced (tmux keeps
/// streaming `%output` — copy mode does not pause it), but the emit to the
/// rendered grid is gated: the capture-fed refresh path
/// (`OzmuxTmuxCopyModePlugin`) paints the scrolled view instead. On exit, the
/// `OzmuxTmuxCopyModePlugin` `On<Remove, CopyModeState>` observer forces a full
/// repaint of the live handle so the grid switches back from capture content
/// (an idle pane emits no new `%output`, and a later delta would paint over the
/// captured rows).
fn route_tmux_output(
    mut commands: Commands,
    mut reader: MessageReader<PaneOutput>,
    mut handles: Query<(&mut TerminalHandle, &mut TerminalTitle)>,
    panes: Query<(Entity, &TmuxPane)>,
    copy_modes: Query<(), With<crate::ui::copy_mode::CopyModeState>>,
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
        // NOTE: skip the live flush while in copy mode — `apply_copy_overlay` paints
        // the captured scrollback onto the grid, and a late %output delta must not
        // overwrite it. The exit observer forces a full repaint on return.
        if copy_modes.get(entity).is_err() {
            handle.flush_emit(&mut commands, entity);
        }
        // NOTE: drain and DISCARD — never forward these replies to tmux. The
        // handle is a display-only renderer; tmux already answered the program's
        // DSR/DA query. Injecting alacritty's duplicate answer via `send-keys -H`
        // delivers it to the program as phantom keystrokes (e.g. the DSR-OK reply
        // `ESC[0n` makes readline self-insert a stray `n` and desyncs arrow-key
        // history recall). Draining still matters: the reply channel is unbounded.
        let _ = handle.take_replies();
    }
}

/// Computes packed pixel rects, 1px-gap divider positions, and the total bbox
/// for a layout tree. Returns `(pane rects, dividers, bbox)`.
///
/// The last child in each split fills the remaining available space so no blank
/// strip appears at window edges.
fn collapse(
    root: &Cell,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
    workspace_h: f32,
) -> (HashMap<PaneId, Rect>, Vec<DividerPixelRect>, Vec2) {
    let dims = root.dims();
    let available = Vec2::new(dims.width as f32 * cell_w, workspace_h);
    let mut panes = HashMap::new();
    let mut dividers = Vec::new();
    place(
        &mut panes,
        &mut dividers,
        root,
        Vec2::ZERO,
        available,
        cell_w,
        cell_h,
        gap,
    );
    let actual_h = panes.values().map(|r| r.max.y).fold(0.0f32, f32::max);
    let bbox = Vec2::new(available.x, actual_h.max(available.y));
    (panes, dividers, bbox)
}

fn place(
    panes: &mut HashMap<PaneId, Rect>,
    dividers: &mut Vec<DividerPixelRect>,
    cell: &Cell,
    origin: Vec2,
    available: Vec2,
    cell_w: f32,
    cell_h: f32,
    gap: f32,
) -> Vec2 {
    match cell {
        Cell::Leaf { dims, pane_id } => {
            let node_size = Vec2::new(
                available.x.max(dims.width as f32 * cell_w),
                available.y.max(dims.height as f32 * cell_h),
            );
            if let Some(id) = pane_id {
                panes.insert(
                    PaneId(*id),
                    Rect::from_corners(origin.round(), (origin + node_size).round()),
                );
            }
            node_size
        }
        Cell::Split {
            dir: SplitDir::LeftRight,
            children,
            ..
        } => {
            let last = children.len().saturating_sub(1);
            let mut x = origin.x;
            for (i, child) in children.iter().enumerate() {
                let child_avail_w = if i == last {
                    available.x - (x - origin.x)
                } else {
                    child.dims().width as f32 * cell_w
                };
                let csz = place(
                    panes,
                    dividers,
                    child,
                    Vec2::new(x, origin.y),
                    Vec2::new(child_avail_w, available.y),
                    cell_w,
                    cell_h,
                    gap,
                );
                x += csz.x;
                if i < last {
                    dividers.push(DividerPixelRect {
                        axis: DividerAxis::Vertical,
                        primary: last_pane_id(child).unwrap_or(PaneId(0)),
                        pos_px: x,
                        span_start_px: origin.y,
                        span_end_px: origin.y + available.y,
                    });
                    x += gap;
                }
            }
            Vec2::new(x - origin.x, available.y)
        }
        Cell::Split {
            dir: SplitDir::TopBottom,
            children,
            ..
        } => {
            let n = children.len();
            let last = n.saturating_sub(1);
            let gaps_total = last as f32 * gap;
            let natural_h: f32 = children
                .iter()
                .map(|c| c.dims().height as f32 * cell_h)
                .sum();
            let slack = (available.y - natural_h - gaps_total).max(0.0);
            let mut y = origin.y;
            for (i, child) in children.iter().enumerate() {
                let child_avail_h = if i == last {
                    available.y - (y - origin.y)
                } else {
                    let extra = (slack * (i + 1) as f32 / n as f32).round()
                        - (slack * i as f32 / n as f32).round();
                    child.dims().height as f32 * cell_h + extra
                };
                let csz = place(
                    panes,
                    dividers,
                    child,
                    Vec2::new(origin.x, y),
                    Vec2::new(available.x, child_avail_h),
                    cell_w,
                    cell_h,
                    gap,
                );
                y += csz.y;
                if i < last {
                    dividers.push(DividerPixelRect {
                        axis: DividerAxis::Horizontal,
                        primary: last_pane_id(child).unwrap_or(PaneId(0)),
                        pos_px: y,
                        span_start_px: origin.x,
                        span_end_px: origin.x + available.x,
                    });
                    y += gap;
                }
            }
            Vec2::new(available.x, y - origin.y)
        }
        Cell::Split {
            dir: SplitDir::Floating,
            children,
            dims,
        } => {
            for child in children {
                let d = child.dims();
                let lit_origin = Vec2::new(d.xoff as f32 * cell_w, d.yoff as f32 * cell_h);
                let lit_avail = Vec2::new(d.width as f32 * cell_w, d.height as f32 * cell_h);
                place(
                    panes,
                    dividers,
                    child,
                    lit_origin,
                    lit_avail,
                    cell_w,
                    cell_h,
                    gap,
                );
            }
            Vec2::new(dims.width as f32 * cell_w, dims.height as f32 * cell_h)
        }
    }
}

fn last_pane_id(cell: &Cell) -> Option<PaneId> {
    match cell {
        Cell::Leaf {
            pane_id: Some(id), ..
        } => Some(PaneId(*id)),
        Cell::Leaf { pane_id: None, .. } => None,
        Cell::Split { children, .. } => children.iter().rev().find_map(last_pane_id),
    }
}

/// Converts tmux cell dims (`u32`) to an alacritty grid size, clamping into
/// `1..=u16::MAX` so a pathological width/height cannot truncate to 0 (a 0-col
/// `Term::resize` would panic).
fn grid_dims(width: u32, height: u32) -> (u16, u16) {
    let clamp = |v: u32| v.clamp(1, u16::MAX as u32) as u16;
    (clamp(width), clamp(height))
}

fn layout_tmux_panes(
    mut commands: Commands,
    mut windows: Query<
        (
            Entity,
            &TmuxWindowLayout,
            &mut Node,
            &Children,
            Option<&PackedTmuxLayout>,
        ),
        With<TmuxWindow>,
    >,
    mut panes: Query<
        (
            &TmuxPane,
            &mut Node,
            &mut TerminalHandle,
            Option<&mut TerminalGrid>,
        ),
        Without<TmuxWindow>,
    >,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let dpr = window.scale_factor().max(0.5);
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0) / dpr;
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0) / dpr;
    let workspace_h = (window.resolution.physical_height() as f32 / dpr - cell_h).max(cell_h);

    for (window_entity, layout, mut container, children, maybe_packed) in windows.iter_mut() {
        let (rects, dividers, bbox) = collapse(
            &layout.0.root,
            cell_w,
            cell_h,
            theme::PANE_GAP_PX,
            workspace_h,
        );

        // NOTE: only write the Node fields when they actually change — writing
        // through `Mut<Node>` every frame marks the component changed and forces
        // a full UI relayout pass each tick even when nothing moved.
        if container.width != Val::Px(bbox.x) || container.height != Val::Px(bbox.y) {
            container.width = Val::Px(bbox.x);
            container.height = Val::Px(bbox.y);
        }

        let packed_changed = maybe_packed
            .is_none_or(|p| p.panes != rects || p.dividers != dividers || p.bbox != bbox);
        if packed_changed {
            commands.entity(window_entity).insert(PackedTmuxLayout {
                panes: rects.clone(),
                dividers: dividers.clone(),
                bbox,
            });
        }

        for child in children.iter() {
            let Ok((pane, mut node, mut handle, grid)) = panes.get_mut(child) else {
                continue;
            };
            let Some(rect) = rects.get(&pane.id) else {
                continue;
            };
            let (left, top, width, height) = (rect.min.x, rect.min.y, rect.width(), rect.height());
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
                if let Some(mut grid) = grid {
                    grid.cols = cols;
                    grid.rows = rows;
                }
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

/// Returns the size-declaration command for `win` based on the tmux per-window
/// `refresh-client` capability. `None` (capability not yet known) falls back to
/// the global `refresh-client -C W,H`.
fn pin_command(per_window: Option<bool>, win: WindowId, cols: u16, rows: u16) -> String {
    match per_window {
        Some(true) => window_refresh_client_command(win, cols, rows),
        Some(false) => resize_window_command(win, cols, rows),
        None => refresh_client_command(cols, rows),
    }
}

/// Decides whether to (re)send the active window's size pin.
///
/// Sends when the active window or desired size changed (normal dedupe), or
/// when tmux's reported size drifted to a NEW value away from desired (foreign
/// resize → recovery). Does NOT send when the reported size is stuck at the same
/// drifted value after a pin — tmux is refusing to grow the window (a smaller
/// foreign client holds `w->latest`), so resending would spin.
fn reconcile_decision(
    desired: (u16, u16),
    active: WindowId,
    reported: Option<(u16, u16)>,
    prev_reported: Option<(u16, u16)>,
    last_window: Option<WindowId>,
    last_desired: (u16, u16),
) -> bool {
    let window_or_size_changed = last_window != Some(active) || last_desired != desired;
    let drift = reported.is_some_and(|r| r != desired);
    let reported_changed = reported != prev_reported;
    window_or_size_changed || (drift && reported_changed)
}

/// Pins the active tmux window to ozmux's cell size via per-window
/// `refresh-client -C @win:WxH` (tmux ≥ 3.4) so a smaller foreign client cannot
/// collapse it. One row is reserved for the ozmux status bar. Uses
/// [`reconcile_decision`] gated on [`LastClientSize`] to detect foreign-resize
/// drift and recover without spinning when tmux refuses to grow the window.
fn sync_client_size(
    mut last: ResMut<LastClientSize>,
    connection: NonSend<TmuxConnection>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
    active: Query<(&TmuxWindow, Option<&TmuxWindowLayout>), With<ActiveWindow>>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    let Ok(window) = window.single() else {
        return;
    };
    let Ok((tmux_window, layout)) = active.single() else {
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
    let desired = (cols, rows);
    let reported = layout.map(|l| {
        let d = l.0.root.dims();
        (
            d.width.clamp(1, u16::MAX as u32) as u16,
            d.height.clamp(1, u16::MAX as u32) as u16,
        )
    });
    let prev_reported = last.last_reported;
    if !reconcile_decision(
        desired,
        tmux_window.id,
        reported,
        prev_reported,
        last.window,
        (last.cols, last.rows),
    ) {
        if last.last_reported != reported {
            last.last_reported = reported;
        }
        return;
    }
    let cmd = pin_command(connection.supports_per_window_refresh(), tmux_window.id, cols, rows);
    // NOTE: only record the size as sent AFTER a successful send — otherwise a
    // transient send failure would poison the dedupe and permanently suppress
    // re-sending this size, leaving tmux stuck at the stale client dimensions.
    match client.handle().send(&cmd) {
        Ok(_) => {
            last.window = Some(tmux_window.id);
            last.cols = cols;
            last.rows = rows;
            // NOTE: advance the observation only after a successful pin; a failed
            // send must leave last_reported stale so the next tick re-detects the
            // drift and retries — otherwise a failed recovery is permanently
            // suppressed (reported_changed would be false next tick).
            if last.last_reported != reported {
                last.last_reported = reported;
            }
        }
        Err(e) => tracing::warn!(?e, cols, rows, "window refresh-client send failed"),
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
    use bevy::math::{Rect, Vec2};
    use ozma_tty_renderer::prelude::TerminalGridPlugin;
    use ozmux_tmux::PaneOutput;
    use tmux_control_parser::{Cell, CellDims, SplitDir};

    #[test]
    fn pin_command_selects_form_by_capability() {
        use ozmux_tmux::WindowId;
        assert_eq!(
            pin_command(Some(true), WindowId(3), 80, 24),
            "refresh-client -C @3:80x24"
        );
        assert_eq!(
            pin_command(Some(false), WindowId(3), 80, 24),
            "resize-window -x 80 -y 24 -t @3"
        );
        assert_eq!(
            pin_command(None, WindowId(3), 80, 24),
            "refresh-client -C 80,24"
        );
    }

    #[test]
    fn reconcile_sends_on_first_call_and_on_changes() {
        use ozmux_tmux::WindowId;
        let w = WindowId(1);
        // First call: nothing pinned yet → send.
        assert!(reconcile_decision((80, 24), w, None, None, None, (0, 0)));
        // Active window changed → send.
        assert!(reconcile_decision((80, 24), WindowId(2), None, None, Some(w), (80, 24)));
        // Desired size changed → send.
        assert!(reconcile_decision((100, 30), w, None, None, Some(w), (80, 24)));
        // Steady state, reported matches desired → skip.
        assert!(!reconcile_decision((80, 24), w, Some((80, 24)), Some((80, 24)), Some(w), (80, 24)));
    }

    #[test]
    fn reconcile_recovers_on_foreign_drift_but_does_not_spin() {
        use ozmux_tmux::WindowId;
        let w = WindowId(1);
        // Foreign just shrank the window: reported drifted to a NEW value → send (recovery).
        assert!(reconcile_decision((80, 24), w, Some((40, 12)), Some((80, 24)), Some(w), (80, 24)));
        // tmux refuses to grow (smaller foreign holds w->latest): reported STUCK at the
        // same drifted value after our pin → skip (no resend spam, residual limitation).
        assert!(!reconcile_decision((80, 24), w, Some((40, 12)), Some((40, 12)), Some(w), (80, 24)));
    }

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
            .expect("pane has TerminalGrid");
        let row0: String = grid.cells[0].iter().map(|c| c.text.as_str()).collect();
        assert!(
            row0.starts_with("hi"),
            "rendered grid row 0 should start with 'hi', got {row0:?}",
        );
    }

    #[test]
    fn copy_mode_pane_advances_but_gates_the_emit() {
        use crate::ui::copy_mode::CopyModeState;

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

        // Frame 1: attach the handle, then paint "hi" so there is a baseline grid.
        app.update();
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"hi".to_vec(),
            });
        app.update();
        let baseline: String = app
            .world()
            .get::<TerminalGrid>(pane_entity)
            .unwrap()
            .cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(baseline.starts_with("hi"), "baseline grid painted 'hi'");

        // Enter copy mode, then deliver more output: the live handle must advance
        // but the rendered grid must NOT change (emit gated).
        app.world_mut()
            .entity_mut(pane_entity)
            .insert(CopyModeState);
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"\r\nXY".to_vec(),
            });
        app.update();
        let gated: String = app
            .world()
            .get::<TerminalGrid>(pane_entity)
            .unwrap()
            .cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            gated.starts_with("hi"),
            "grid stays at the baseline while in copy mode, got {gated:?}",
        );

        // Exit copy mode and deliver one more (empty) output batch: now that the
        // marker is gone, route_tmux_output emits, so the gated-but-advanced live
        // content ("XY" on row 1) reaches the rendered grid.
        app.world_mut()
            .entity_mut(pane_entity)
            .remove::<CopyModeState>();
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: Vec::new(),
            });
        app.update();
        let resumed: String = app
            .world()
            .get::<TerminalGrid>(pane_entity)
            .unwrap()
            .cells[1]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            resumed.starts_with("XY"),
            "the live handle advanced under copy mode; after exit + emit row 1 shows 'XY', got {resumed:?}",
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

        use ozmux_tmux::{TmuxWindow, TmuxWindowLayout};
        use tmux_control_parser::{WindowId, WindowLayout};

        let window_e = app
            .world_mut()
            .spawn((
                TmuxWindow {
                    id: WindowId(1),
                    index: 0,
                    name: String::new(),
                },
                TmuxWindowLayout(WindowLayout::parse(b"0000,40x10,0,0,1").unwrap()),
                Node::default(),
            ))
            .id();
        let pane = app
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
                TerminalGrid::default(),
                TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
                ChildOf(window_e),
            ))
            .id();

        app.add_systems(Update, layout_tmux_panes);
        app.update();

        let grid = app
            .world()
            .get::<TerminalGrid>(pane)
            .expect("pane has TerminalGrid");
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

        use ozmux_tmux::{TmuxWindow, TmuxWindowLayout};
        use tmux_control_parser::{WindowId, WindowLayout};

        let pane_id = PaneId(2);
        let window_e = app
            .world_mut()
            .spawn((
                TmuxWindow {
                    id: WindowId(1),
                    index: 0,
                    name: String::new(),
                },
                TmuxWindowLayout(WindowLayout::parse(b"0000,20x5,0,0,2").unwrap()),
                Node::default(),
            ))
            .id();
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
                TerminalGrid::default(),
                TerminalHandle::detached(20, 5, Arc::new(AtomicBool::new(false))),
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

        app.update();
        let hits_after_first = app.world().resource::<SnapHits>().0;
        assert!(
            hits_after_first >= 1,
            "first flush_emit must fire a FrameSnapshot (first_emit path)"
        );

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

        app.update();
        let hits_after_resize = app.world().resource::<SnapHits>().0;

        let grid = app
            .world()
            .get::<TerminalGrid>(entity)
            .expect("pane has TerminalGrid");
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

    #[test]
    fn collapse_single_pane_fills_available() {
        let root = Cell::Leaf {
            dims: CellDims {
                width: 80,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
            pane_id: Some(0),
        };
        let (rects, _, bbox) = collapse(&root, 8.0, 16.0, 1.0, 384.0);
        assert_eq!(
            rects[&PaneId(0)],
            Rect::from_corners(Vec2::ZERO, Vec2::new(640.0, 384.0))
        );
        assert_eq!(bbox, Vec2::new(640.0, 384.0));
    }

    #[test]
    fn collapse_left_right_produces_one_px_gap() {
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
        let (rects, dividers, bbox) = collapse(&root, 8.0, 16.0, 1.0, 384.0);
        assert_eq!(
            rects[&PaneId(1)],
            Rect::from_corners(Vec2::ZERO, Vec2::new(320.0, 384.0))
        );
        assert_eq!(
            rects[&PaneId(2)],
            Rect::from_corners(Vec2::new(321.0, 0.0), Vec2::new(640.0, 384.0)),
        );
        assert_eq!(bbox, Vec2::new(640.0, 384.0));
        assert_eq!(dividers.len(), 1);
        assert_eq!(dividers[0].axis, DividerAxis::Vertical);
        assert!((dividers[0].pos_px - 320.0).abs() < 0.5);
        assert_eq!(dividers[0].primary, PaneId(1));
    }

    #[test]
    fn collapse_nested_split_fills_without_blank_strips() {
        let right_child = Cell::Split {
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
        };
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
                right_child,
            ],
        };
        // natural_h=192+176=368, gap=1, slack=384-368-1=15
        // slack distributed: pane2 gets round(15/2)=8px, pane3 (last) gets remaining 7px
        // pane2: 12*16+8=200, pane3: 384-200-1=183 (11*16+7=183)
        let (rects, _, bbox) = collapse(&root, 8.0, 16.0, 1.0, 384.0);
        assert_eq!(
            rects[&PaneId(1)],
            Rect::from_corners(Vec2::ZERO, Vec2::new(320.0, 384.0))
        );
        assert_eq!(
            rects[&PaneId(2)],
            Rect::from_corners(Vec2::new(321.0, 0.0), Vec2::new(640.0, 200.0)),
        );
        assert_eq!(
            rects[&PaneId(3)],
            Rect::from_corners(Vec2::new(321.0, 201.0), Vec2::new(640.0, 384.0)),
        );
        assert_eq!(bbox, Vec2::new(640.0, 384.0));
    }

    #[test]
    fn collapse_compound_non_last_child_no_overlap() {
        let inner_lr = Cell::Split {
            dims: CellDims {
                width: 40,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
            dir: SplitDir::LeftRight,
            children: vec![
                Cell::Leaf {
                    dims: CellDims {
                        width: 20,
                        height: 24,
                        xoff: 0,
                        yoff: 0,
                    },
                    pane_id: Some(1),
                },
                Cell::Leaf {
                    dims: CellDims {
                        width: 19,
                        height: 24,
                        xoff: 21,
                        yoff: 0,
                    },
                    pane_id: Some(2),
                },
            ],
        };
        let root = Cell::Split {
            dims: CellDims {
                width: 80,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
            dir: SplitDir::LeftRight,
            children: vec![
                inner_lr,
                Cell::Leaf {
                    dims: CellDims {
                        width: 39,
                        height: 24,
                        xoff: 41,
                        yoff: 0,
                    },
                    pane_id: Some(3),
                },
            ],
        };
        let (rects, dividers, bbox) = collapse(&root, 8.0, 16.0, 1.0, 384.0);
        assert_eq!(
            rects[&PaneId(1)],
            Rect::from_corners(Vec2::ZERO, Vec2::new(160.0, 384.0))
        );
        assert_eq!(
            rects[&PaneId(2)],
            Rect::from_corners(Vec2::new(161.0, 0.0), Vec2::new(320.0, 384.0)),
        );
        assert_eq!(
            rects[&PaneId(3)],
            Rect::from_corners(Vec2::new(321.0, 0.0), Vec2::new(640.0, 384.0)),
        );
        assert_eq!(bbox, Vec2::new(640.0, 384.0));
        assert_eq!(dividers.len(), 2);
        let outer = dividers
            .iter()
            .find(|d| (d.pos_px - 320.0).abs() < 0.5)
            .unwrap();
        assert_eq!(outer.axis, DividerAxis::Vertical);
        assert_eq!(
            outer.primary,
            PaneId(2),
            "outer divider primary must be the rightmost pane of the before-child",
        );
    }

    #[test]
    fn collapse_skips_leaf_without_pane_id() {
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
        let (rects, _, _) = collapse(&root, 8.0, 16.0, 1.0, 384.0);
        assert_eq!(rects.len(), 1, "pane_id:None leaf is not placed");
        assert!(rects.contains_key(&PaneId(2)));
    }

    #[test]
    fn collapse_workspace_h_fills_slack() {
        let root = Cell::Leaf {
            dims: CellDims {
                width: 20,
                height: 5,
                xoff: 0,
                yoff: 0,
            },
            pane_id: Some(1),
        };
        // workspace_h = 5*16 (natural) + 24 (slack) = 104px; the single leaf
        // stretches to fill the whole workspace height.
        let workspace_h = 5.0 * 16.0 + 24.0;
        let (rects, _, bbox) = collapse(&root, 8.0, 16.0, 1.0, workspace_h);
        assert_eq!(
            rects[&PaneId(1)].height(),
            workspace_h,
            "pane fills workspace including slack"
        );
        assert_eq!(bbox.y, workspace_h, "bbox height equals workspace_h");
    }

    #[test]
    fn collapse_slack_distributed_across_top_bottom_panes() {
        let root = Cell::Split {
            dims: CellDims {
                width: 80,
                height: 26,
                xoff: 0,
                yoff: 0,
            },
            dir: SplitDir::TopBottom,
            children: vec![
                Cell::Leaf {
                    dims: CellDims {
                        width: 80,
                        height: 8,
                        xoff: 0,
                        yoff: 0,
                    },
                    pane_id: Some(1),
                },
                Cell::Leaf {
                    dims: CellDims {
                        width: 80,
                        height: 8,
                        xoff: 0,
                        yoff: 9,
                    },
                    pane_id: Some(2),
                },
                Cell::Leaf {
                    dims: CellDims {
                        width: 80,
                        height: 8,
                        xoff: 0,
                        yoff: 18,
                    },
                    pane_id: Some(3),
                },
            ],
        };
        // cell_h=16, gap=1
        // natural_h = 3*(8*16) = 384, gaps = 2, slack = 440-384-2 = 54
        // each pane gets 18px extra: 8*16+18 = 146
        // pane1: y=0..146, pane2: y=147..293, pane3: y=294..440
        let (rects, _, bbox) = collapse(&root, 8.0, 16.0, 1.0, 440.0);
        assert_eq!(
            rects[&PaneId(1)].height(),
            146.0,
            "pane 1 gets its share of slack"
        );
        assert_eq!(
            rects[&PaneId(2)].height(),
            146.0,
            "pane 2 gets its share of slack"
        );
        assert_eq!(
            rects[&PaneId(3)].height(),
            146.0,
            "pane 3 gets its share of slack"
        );
        assert_eq!(bbox.y, 440.0, "bounding box matches workspace_h");
    }
}
