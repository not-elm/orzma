//! Render layer for tmux panes: attaches a PTY-less `TerminalHandle` plus the
//! GPU render bundle to each projected `TmuxPane`, then routes tmux `%output`
//! into the handle. Lives in the binary so `orzma_tmux` stays renderer-free.

mod paint_rescue;

use crate::app_mode::TmuxActiveSet;
use crate::render::tmux::paint_rescue::PaintRescuePlugin;
use crate::surface::OrzmaTerminal;
use crate::surface::geometry::cells_for;
use crate::theme;
use crate::theme::PANE_GAP;
use crate::ui::tmux::mode_ui::WorkspaceUiRoot;
use bevy::ecs::message::MessageReader;
use bevy::math::Rect;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use orzma_tmux::{
    ActiveWindow, PaneOutput, RefreshClient, ResizeWindow, TmuxClientMut, TmuxCommand, TmuxPane,
    TmuxProjectionSet, TmuxWindow, TmuxWindowLayout, WindowId, WindowRefreshClient,
};
use orzma_tty_engine::{Coalescer, TerminalHandle, TerminalTitle};
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::TerminalPaddingFallback;
use orzma_tty_renderer::schema::TerminalGrid;
use std::collections::HashMap;
use std::collections::HashSet;
use tmux_control_parser::{Cell, DividerAxis, PaneId, SplitDir};

/// Total cap, across all panes, for bytes buffered while a pane's handle/grid
/// is not yet present. One MiB is far above any real capture seed; the cap only
/// guards a pane that never attaches.
const PENDING_PANE_OUTPUT_CAP: usize = 1 << 20;

/// Bytes routed to a pane before its `TerminalHandle` was queryable, held until
/// the pane is ready and then replayed. Prevents losing the authoritative
/// `capture-pane` seed (spec Component 1).
#[derive(Resource, Default)]
struct PendingPaneOutput {
    buf: HashMap<PaneId, Vec<u8>>,
    total: usize,
}

impl PendingPaneOutput {
    fn push(&mut self, pane: PaneId, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if self.total + data.len() > PENDING_PANE_OUTPUT_CAP {
            if let Some(old) = self.buf.remove(&pane) {
                self.total -= old.len();
            }
            tracing::warn!(
                pane = pane.0,
                "pending pane-output over cap; dropped buffered bytes"
            );
            // NOTE: the cap is global across all panes — if evicting this pane's
            // own backlog still leaves no room (another pane holds the budget),
            // drop the incoming bytes so `total` can never exceed the cap.
            if self.total + data.len() > PENDING_PANE_OUTPUT_CAP {
                return;
            }
        }
        let entry = self.buf.entry(pane).or_default();
        entry.extend_from_slice(data);
        self.total += data.len();
    }

    fn take(&mut self, pane: PaneId) -> Option<Vec<u8>> {
        let v = self.buf.remove(&pane)?;
        self.total -= v.len();
        Some(v)
    }
}

/// Ordering handle for `layout_tmux_panes` so the paint-rescue system can run
/// before this frame's grid-dims write (avoids the ≤1-frame resize transient
/// where `cells.len() != rows`).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct TmuxLayoutSet;

#[derive(Resource, Default)]
struct LastClientSize {
    window: Option<WindowId>,
    cols: u16,
    rows: u16,
    last_reported: Option<(u16, u16)>,
    last_per_window: Option<bool>,
}

/// Wires the tmux pane render systems after the projection chain.
pub(crate) struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(PaintRescuePlugin)
            .insert_resource(ClearColor(PANE_GAP))
            .init_resource::<LastClientSize>()
            .init_resource::<PendingPaneOutput>()
            .insert_resource(TerminalPaddingFallback(theme_background_bytes()))
            .add_systems(
                Update,
                (
                    attach_tmux_window_container,
                    attach_tmux_pane_terminal,
                    route_tmux_output
                        .run_if(on_message::<PaneOutput>.or(pending_pane_output_waiting)),
                    sync_active_window,
                    layout_tmux_panes.in_set(TmuxLayoutSet),
                )
                    .chain()
                    .after(TmuxProjectionSet)
                    .in_set(TmuxActiveSet),
            )
            .add_systems(
                Update,
                sync_client_size
                    .after(TmuxProjectionSet)
                    .in_set(TmuxActiveSet),
            )
            .add_observer(prune_pending_on_pane_removed);
    }
}

/// Pixel-space position of a 1px inter-pane gap.
/// Positions are in logical px (the same coordinate space as Bevy `Node` rects).
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

/// Drops a despawned pane's buffered output so its `PaneId` cannot keep
/// `pending_pane_output_waiting` true (which would spin `route_tmux_output`
/// every frame for the rest of the session).
fn prune_pending_on_pane_removed(
    ev: On<Remove, TmuxPane>,
    mut pending: ResMut<PendingPaneOutput>,
    panes: Query<&TmuxPane>,
) {
    if let Ok(pane) = panes.get(ev.entity) {
        pending.take(pane.id);
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

/// Attaches a detached `TerminalHandle`, a `Coalescer`, a `TerminalTitle`, the
/// `OrzmaTerminal` marker, and a placeholder absolute `Node` to each `TmuxPane`
/// that lacks a `TerminalHandle`.
/// The `On<Add, OrzmaTerminal>` observer in `crate::surface` injects the
/// `TerminalRenderBundle` (one per entity, no duplicate material creation here).
/// The `TerminalGrid` lives on the pane entity itself, so `flush_emit` /
/// `emit_pending` and the webview overlay projection all target it
/// directly (a webview mounts as a `ChildOf` the pane).
///
/// Runs every frame but targets each pane exactly once. `ChildOf` is NOT set
/// here — the projection observers already establish the correct
/// `ChildOf(window)` parent. `layout_tmux_panes` sets the real rect every frame.
fn attach_tmux_pane_terminal(
    mut commands: Commands,
    panes: Query<(Entity, &TmuxPane), Without<TerminalHandle>>,
) {
    for (entity, pane) in panes.iter() {
        let (cols, rows) = grid_dims(pane.dims.width, pane.dims.height);
        let handle = TerminalHandle::detached(cols, rows);

        commands.entity(entity).insert((
            handle,
            Coalescer::default(),
            TerminalTitle::default(),
            OrzmaTerminal,
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
        ));
    }
}

fn pending_pane_output_waiting(pending: Res<PendingPaneOutput>) -> bool {
    !pending.buf.is_empty()
}

/// Routes tmux `%output` into each pane's handle. Groups a frame's
/// `PaneOutput` messages by pane, advances all of a pane's bytes, then emits
/// once per pane (immediate emit, coalesced per pane).
///
/// The per-pane alacritty handle is display-only: tmux is the real terminal and
/// already answers the program's device queries (DSR/DA) itself. So this drains
/// the handle's reply queue and discards it — see the `take_replies` `NOTE`.
///
/// The live bytes are advanced and emitted unconditionally, in and out of vi
/// mode: the local vi applier (`crate::action::vi::applier`) scrolls and
/// selects on this same `TerminalHandle`, so the handle's own scrolled view is
/// what the rendered grid always shows — there is no separate capture-fed
/// refresh path to defer to.
fn route_tmux_output(
    mut commands: Commands,
    mut reader: MessageReader<PaneOutput>,
    mut handles: Query<(&mut TerminalHandle, &mut TerminalTitle)>,
    mut pending: ResMut<PendingPaneOutput>,
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
    let mut pane_ids: HashSet<PaneId> = by_pane.keys().copied().collect();
    pane_ids.extend(pending.buf.keys().copied());
    for pane in pane_ids {
        let tmux_output = by_pane.remove(&pane).unwrap_or_default();
        let Some(&entity) = entity_of.get(&pane) else {
            pending.push(pane, &tmux_output);
            continue;
        };
        let Ok((mut handle, mut title)) = handles.get_mut(entity) else {
            pending.push(pane, &tmux_output);
            continue;
        };
        if let Some(buffered) = pending.take(pane) {
            handle.advance(&buffered);
        }
        if !tmux_output.is_empty() {
            handle.advance(&tmux_output);
        }
        // NOTE: drain captured control frames — crucially the OSC 5379
        // mount/unmount verbs — into observer triggers. The engine's own control
        // drain runs only for PtyHandle-backed terminals; tmux panes have no
        // PtyHandle, so without this call a pane program's webview mount
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
                    panes, dividers, child, lit_origin, lit_avail, cell_w, cell_h, gap,
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

fn rows_for_panes(total_rows: u16) -> u16 {
    total_rows.saturating_sub(1).max(1)
}

/// The pinned tmux pane cell size (`(cols, rows)`) for the current GUI window,
/// with one row reserved for the orzma status bar. [`sync_client_size`] pins
/// tmux to this. The gateway PTY is sized to the full window (no bar
/// reservation) by `sync_gateway_size`, so reconciliation to this size is
/// always a shrink, never a grow.
fn client_cell_size(phys_w: u32, phys_h: u32, cell_w_phys: f32, cell_h_phys: f32) -> (u16, u16) {
    let (cols, rows) = cells_for(phys_w, phys_h, cell_w_phys, cell_h_phys);
    (cols, rows_for_panes(rows))
}

/// Size-declaration command for `win` chosen by the tmux per-window
/// `refresh-client` capability. `None` (capability not yet known) falls back to
/// the global `refresh-client -C W,H`.
struct Pin {
    per_window: Option<bool>,
    win: WindowId,
    cols: u16,
    rows: u16,
}
impl TmuxCommand for Pin {
    fn into_raw_command(self) -> String {
        match self.per_window {
            Some(true) => WindowRefreshClient {
                win: self.win,
                cols: self.cols,
                rows: self.rows,
            }
            .into_raw_command(),
            Some(false) => ResizeWindow {
                win: self.win,
                cols: self.cols,
                rows: self.rows,
            }
            .into_raw_command(),
            None => RefreshClient {
                cols: self.cols,
                rows: self.rows,
            }
            .into_raw_command(),
        }
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

/// Pins the active tmux window to orzma's cell size via per-window
/// `refresh-client -C @win:WxH` (tmux ≥ 3.4) so a smaller foreign client cannot
/// collapse it. One row is reserved for the orzma status bar. Uses
/// [`reconcile_decision`] gated on [`LastClientSize`] to detect foreign-resize
/// drift and recover without spinning when tmux refuses to grow the window.
fn sync_client_size(
    mut last: ResMut<LastClientSize>,
    mut client: TmuxClientMut<'_, '_>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
    active: Query<(&TmuxWindow, Option<&TmuxWindowLayout>), With<ActiveWindow>>,
) {
    let Ok(window) = window.single() else {
        return;
    };
    let Ok((tmux_window, layout)) = active.single() else {
        return;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let (cols, rows) = client_cell_size(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        cell_w,
        cell_h,
    );
    let desired = (cols, rows);
    let reported = layout.map(|l| {
        let d = l.0.root.dims();
        grid_dims(d.width, d.height)
    });
    let prev_reported = last.last_reported;
    // NOTE: a capability change (None -> Some after the version reply) must force
    // a re-pin even when window/size/reported are unchanged — otherwise the first
    // pin sent with the global fallback (capability still unknown) is never
    // upgraded to the per-window form, defeating multi-client isolation.
    let per_window = client.supports_per_window_refresh();
    let capability_changed = last.last_per_window != per_window;
    if !capability_changed
        && !reconcile_decision(
            desired,
            tmux_window.id,
            reported,
            prev_reported,
            last.window,
            (last.cols, last.rows),
        )
    {
        if last.last_reported != reported {
            last.last_reported = reported;
        }
        return;
    }
    // NOTE: only record the size as sent AFTER a successful send — otherwise a
    // transient send failure would poison the dedupe and permanently suppress
    // re-sending this size, leaving tmux stuck at the stale client dimensions.
    match client.send(Pin {
        per_window,
        win: tmux_window.id,
        cols,
        rows,
    }) {
        Ok(_) => {
            last.window = Some(tmux_window.id);
            last.cols = cols;
            last.rows = rows;
            last.last_per_window = per_window;
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

fn theme_background_bytes() -> [u8; 3] {
    let s = theme::BACKGROUND.to_srgba();
    [
        (s.red * 255.0).round() as u8,
        (s.green * 255.0).round() as u8,
        (s.blue * 255.0).round() as u8,
    ]
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
    use crate::surface::SurfacePlugin;
    use bevy::math::{Rect, Vec2};
    use orzma_tmux::PaneOutput;
    use orzma_tty_renderer::material::TerminalUiMaterial;
    use orzma_tty_renderer::prelude::TerminalGridPlugin;
    use tmux_control_parser::{Cell, CellDims, SplitDir};

    #[test]
    fn pin_selects_form_by_capability() {
        use orzma_tmux::WindowId;
        assert_eq!(
            Pin {
                per_window: Some(true),
                win: WindowId(3),
                cols: 80,
                rows: 24
            }
            .into_raw_command(),
            "refresh-client -C @3:80x24"
        );
        assert_eq!(
            Pin {
                per_window: Some(false),
                win: WindowId(3),
                cols: 80,
                rows: 24
            }
            .into_raw_command(),
            "resize-window -x 80 -y 24 -t @3"
        );
        assert_eq!(
            Pin {
                per_window: None,
                win: WindowId(3),
                cols: 80,
                rows: 24
            }
            .into_raw_command(),
            "refresh-client -C 80,24"
        );
    }

    #[test]
    fn reconcile_sends_on_first_call_and_on_changes() {
        use orzma_tmux::WindowId;
        let w = WindowId(1);
        // First call: nothing pinned yet → send.
        assert!(reconcile_decision((80, 24), w, None, None, None, (0, 0)));
        // Active window changed → send.
        assert!(reconcile_decision(
            (80, 24),
            WindowId(2),
            None,
            None,
            Some(w),
            (80, 24)
        ));
        // Desired size changed → send.
        assert!(reconcile_decision(
            (100, 30),
            w,
            None,
            None,
            Some(w),
            (80, 24)
        ));
        // Steady state, reported matches desired → skip.
        assert!(!reconcile_decision(
            (80, 24),
            w,
            Some((80, 24)),
            Some((80, 24)),
            Some(w),
            (80, 24)
        ));
    }

    #[test]
    fn reconcile_recovers_on_foreign_drift_but_does_not_spin() {
        use orzma_tmux::WindowId;
        let w = WindowId(1);
        // Foreign just shrank the window: reported drifted to a NEW value → send (recovery).
        assert!(reconcile_decision(
            (80, 24),
            w,
            Some((40, 12)),
            Some((80, 24)),
            Some(w),
            (80, 24)
        ));
        // tmux refuses to grow (smaller foreign holds w->latest): reported STUCK at the
        // same drifted value after our pin → skip (no resend spam, residual limitation).
        assert!(!reconcile_decision(
            (80, 24),
            w,
            Some((40, 12)),
            Some((40, 12)),
            Some(w),
            (80, 24)
        ));
    }

    #[test]
    fn rows_for_panes_reserves_one_row_for_the_bar() {
        assert_eq!(rows_for_panes(24), 23);
        assert_eq!(rows_for_panes(1), 1); // never zero
        assert_eq!(rows_for_panes(2), 1);
    }

    #[test]
    fn client_cell_size_matches_the_pinned_size_one_row_reserved() {
        // 1280x752 phys, 8x16 cells -> 160 cols x 47 rows; one row reserved for
        // the orzma status bar -> 46 pane rows. Birthing the new session's PTY at
        // this size makes the first pane geometry equal the size sync_client_size
        // will pin, so no grow happens.
        assert_eq!(client_cell_size(1280, 752, 8.0, 16.0), (160, 46));
        // Degenerate tiny window still yields a usable >=1 size.
        assert_eq!(client_cell_size(1, 1, 8.0, 16.0), (1, 1));
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
        app.add_plugins(SurfacePlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_message::<PaneOutput>();
        app.init_resource::<PendingPaneOutput>();

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
    fn vi_mode_pane_output_still_reaches_the_grid() {
        use crate::ui::vi_mode::ViModeState;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.add_plugins(SurfacePlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_message::<PaneOutput>();
        app.init_resource::<PendingPaneOutput>();

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
        let baseline: String = app.world().get::<TerminalGrid>(pane_entity).unwrap().cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(baseline.starts_with("hi"), "baseline grid painted 'hi'");

        // Enter vi mode, then deliver more output: the emit is no longer
        // gated on ViModeState, so the new content reaches the grid in the
        // SAME frame it arrives — the local vi applier scrolls this same
        // handle, so its own view is what must render.
        app.world_mut().entity_mut(pane_entity).insert(ViModeState);
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"\r\nXY".to_vec(),
            });
        app.update();
        let row0: String = app.world().get::<TerminalGrid>(pane_entity).unwrap().cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        let row1: String = app.world().get::<TerminalGrid>(pane_entity).unwrap().cells[1]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(row0.starts_with("hi"), "row 0 is untouched, got {row0:?}");
        assert!(
            row1.starts_with("XY"),
            "row 1 reflects the new output immediately, while still in vi mode, got {row1:?}",
        );
    }

    #[test]
    fn mount_osc_from_pane_triggers_webview_request() {
        use orzma_tty_engine::OscWebviewRequest;

        #[derive(Resource, Default)]
        struct Seen(u32);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_message::<PaneOutput>();
        app.init_resource::<PendingPaneOutput>();
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

        // Frame 2: deliver a mount OSC and route it.
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"\x1b]5379;mount;memo;3;10\x1b\\".to_vec(),
            });
        app.update();

        assert_eq!(
            app.world().resource::<Seen>().0,
            1,
            "a mount OSC from a tmux pane must trigger exactly one OscWebviewRequest",
        );
    }

    #[test]
    fn resize_only_updates_grid_dims_and_emits() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};
        use orzma_tty_renderer::schema::FrameSnapshot;
        use orzma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};

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

        use orzma_tmux::{TmuxWindow, TmuxWindowLayout};
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
                TerminalHandle::detached(20, 5),
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
    fn resize_fires_fresh_snapshot_after_first_emit() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};
        use orzma_tty_renderer::schema::FrameSnapshot;
        use orzma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};

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

        use orzma_tmux::{TmuxWindow, TmuxWindowLayout};
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
                TerminalHandle::detached(20, 5),
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

    #[test]
    fn pending_output_accumulates_then_takes() {
        let mut p = PendingPaneOutput::default();
        p.push(PaneId(1), b"ab");
        p.push(PaneId(1), b"cd");
        p.push(PaneId(2), b"xy");
        assert_eq!(p.take(PaneId(1)).as_deref(), Some(&b"abcd"[..]));
        assert_eq!(p.take(PaneId(1)), None);
        assert_eq!(p.take(PaneId(2)).as_deref(), Some(&b"xy"[..]));
    }

    #[test]
    fn pending_output_drops_pane_buffer_over_cap() {
        let mut p = PendingPaneOutput::default();
        let big = vec![0u8; PENDING_PANE_OUTPUT_CAP];
        p.push(PaneId(1), &big);
        p.push(PaneId(1), b"z");
        assert!(
            p.total <= PENDING_PANE_OUTPUT_CAP,
            "total must not grow past the cap"
        );
        let got = p.take(PaneId(1)).unwrap_or_default();
        assert!(
            got.len() <= PENDING_PANE_OUTPUT_CAP,
            "buffer must not grow past the cap"
        );
        assert_eq!(p.total, 0, "taking the only pane drains total to zero");
    }

    #[test]
    fn pending_output_enforces_global_cap_across_panes() {
        let mut p = PendingPaneOutput::default();
        let big = vec![0u8; PENDING_PANE_OUTPUT_CAP];
        p.push(PaneId(2), &big);
        // pane 2 already holds the whole budget; a push to pane 1 must not let
        // `total` exceed the global cap even though pane 1 has no backlog to evict.
        p.push(PaneId(1), b"hello");
        assert!(
            p.total <= PENDING_PANE_OUTPUT_CAP,
            "global cap holds across panes, got total = {}",
            p.total
        );
        // The incoming bytes were dropped (no room), so pane 1 holds nothing.
        assert_eq!(p.take(PaneId(1)), None);
        // Draining pane 2 returns the budget exactly.
        let got = p.take(PaneId(2)).unwrap_or_default();
        assert_eq!(got.len(), PENDING_PANE_OUTPUT_CAP);
        assert_eq!(p.total, 0, "draining all panes returns total to zero");
    }

    #[test]
    fn attach_adds_orzma_terminal_and_render_bundle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.add_plugins(SurfacePlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();

        let pane_entity = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(42),
                dims: dims(),
            })
            .id();

        app.add_systems(Update, attach_tmux_pane_terminal);

        app.update();

        assert!(
            app.world().entity(pane_entity).contains::<OrzmaTerminal>(),
            "projected pane must carry OrzmaTerminal after attach",
        );
        assert!(
            app.world().entity(pane_entity).contains::<TerminalGrid>(),
            "On<Add, OrzmaTerminal> must inject TerminalRenderBundle (TerminalGrid proves exactly one render bundle)",
        );
    }

    #[test]
    fn attach_inserts_coalescer_on_tmux_pane() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let entity = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 20,
                    height: 5,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();
        app.add_systems(Update, attach_tmux_pane_terminal);
        app.update();
        assert!(app.world().get::<Coalescer>(entity).is_some());
        assert!(app.world().get::<TerminalHandle>(entity).is_some());
    }

    /// Composed regression test for the bootstrap-rescue exposure the Task 1
    /// code review flagged: once a tmux pane carries `Coalescer` (needed for
    /// shared local vi mode), it also matches
    /// `orzma_tty_engine::flush_due_terminals`'s bootstrap rescue, which has no
    /// `PtyHandle` filter. That rescue can now fire on a tmux pane before
    /// tmux's `capture-pane` seed lands, flipping `first_emit` to `false` on a
    /// still-blank `Term`. Proves what actually happens end to end: the
    /// bootstrap rescue fires exactly once (a blank `Snapshot{Initial}`), the
    /// real seed then arrives classified as a `Delta` (not a
    /// `Snapshot{Initial}`), and the FINAL rendered `TerminalGrid` still ends
    /// up correct — because the bootstrap snapshot already sized `grid.cells`
    /// to `rows`, so the later Delta's per-row bounds-checked write lands
    /// exactly right.
    #[test]
    fn tmux_pane_bootstrap_rescue_before_seed_yields_correct_final_grid() {
        use orzma_tty_engine::TerminalHandlePlugin;
        use orzma_tty_renderer::schema::{
            Cell as GridCell, FrameDelta, FrameSnapshot, SnapshotReason,
        };

        #[derive(Debug, Clone, PartialEq)]
        enum LoggedFrame {
            Snapshot(SnapshotReason),
            Delta { dirty_rows: usize },
        }

        #[derive(Resource, Default)]
        struct FrameLog(Vec<LoggedFrame>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.add_plugins(SurfacePlugin);
        app.add_plugins(TerminalHandlePlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_message::<PaneOutput>();
        app.init_resource::<PendingPaneOutput>();
        app.init_resource::<FrameLog>();
        app.add_observer(|ev: On<FrameSnapshot>, mut log: ResMut<FrameLog>| {
            log.0.push(LoggedFrame::Snapshot(ev.reason));
        });
        app.add_observer(|ev: On<FrameDelta>, mut log: ResMut<FrameLog>| {
            log.0.push(LoggedFrame::Delta {
                dirty_rows: ev.dirty_rows.len(),
            });
        });

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

        // Simulate several frames ticking BEFORE tmux's capture-pane seed reply
        // lands: no PaneOutput is ever sent, exactly the window the review
        // flagged.
        for _ in 0..4 {
            app.update();
        }

        let pre_seed_log = app.world().resource::<FrameLog>().0.clone();
        println!("pre-seed frame log: {pre_seed_log:?}");
        assert_eq!(
            pre_seed_log,
            vec![LoggedFrame::Snapshot(SnapshotReason::Initial)],
            "flush_due_terminals' bootstrap rescue fires exactly once on a tmux \
             pane before any PaneOutput arrives, confirming the review's premise \
             — got {pre_seed_log:?}",
        );

        let grid_before = app
            .world()
            .get::<TerminalGrid>(pane_entity)
            .expect("bootstrap snapshot must have created a correctly-sized grid");
        assert_eq!(
            grid_before.rows as usize,
            grid_before.cells.len(),
            "bootstrap snapshot must size grid.cells to the real row count",
        );
        assert!(
            grid_before.cells.iter().flatten().all(GridCell::is_blank),
            "the bootstrap-rescue snapshot must be blank — nothing real has arrived yet",
        );

        // The capture-pane seed lands: two lines of real content on a 5-row
        // pane (2/5 = 40%, under the 85% snapshot-promotion threshold), so
        // decide_frame_kind classifies it as a Delta once first_emit is false.
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: b"hello world\r\nsecond line".to_vec(),
            });
        app.update();

        let post_seed_log = app.world().resource::<FrameLog>().0.clone();
        println!("post-seed frame log: {post_seed_log:?}");
        assert_eq!(
            post_seed_log.len(),
            2,
            "expected exactly one more frame after the seed lands, got {post_seed_log:?}",
        );
        assert!(
            matches!(post_seed_log[1], LoggedFrame::Delta { .. }),
            "confirms the review's exact concern: the pane's first REAL content \
             arrives as a FrameDelta, not a FrameSnapshot{{reason: Initial}} — got {post_seed_log:?}",
        );

        // The real question: does the renderer end up with the right content
        // regardless of the Delta/Snapshot classification? Check the FINAL grid.
        let grid = app
            .world()
            .get::<TerminalGrid>(pane_entity)
            .expect("pane still has TerminalGrid");
        assert_eq!(
            grid.rows as usize,
            grid.cells.len(),
            "grid must stay structurally sized (rows == cells.len()) after a Delta-classified seed",
        );
        let row0: String = grid.cells[0].iter().map(|c| c.text.as_str()).collect();
        let row1: String = grid.cells[1].iter().map(|c| c.text.as_str()).collect();
        println!("row0={row0:?} row1={row1:?}");
        assert!(
            row0.starts_with("hello world"),
            "row 0 must carry the seed content despite the Delta classification, got {row0:?}",
        );
        assert!(
            row1.starts_with("second line"),
            "row 1 must carry the seed content despite the Delta classification, got {row1:?}",
        );
        for (i, row) in grid.cells.iter().enumerate().skip(2) {
            assert!(
                row.iter().all(GridCell::is_blank),
                "row {i} must remain blank — a Delta must not corrupt untouched rows, got {row:?}",
            );
        }
    }

    /// End-to-end proof that local tmux copy mode is fully wired, on a real
    /// tmux-pane entity carrying the exact bundle `attach_tmux_pane_terminal`
    /// gives every projected pane: enter vi mode, scroll into pre-seeded
    /// history, toggle + extend a selection, yank to the clipboard, and land
    /// back on the live tail. Drives the shared `crate::action::vi` events
    /// directly (the applier pipeline), not the keymap/key-gather layer.
    #[test]
    fn tmux_vi_mode_is_fully_local() {
        use crate::action::vi::{
            ViActionPlugin, ViMotionRequest, ViScrollRequest, ViSelectionToggleRequest,
            ViYankRequest,
        };
        use crate::clipboard::Clipboard;
        use crate::configs::OrzmaConfigsResource;
        use crate::ui::vi_mode::{EnterViModeActionEvent, ViModePlugin, ViModeState};
        use bevy::ecs::message::Messages;
        use orzma_configs::OrzmaConfigs;
        use orzma_configs::vi_mode::ViModeScroll;
        use orzma_tty_engine::{SelectionType, TermMode, TerminalHandlePlugin, ViMotion};
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.add_plugins(SurfacePlugin);
        app.add_plugins(TerminalHandlePlugin);
        app.add_plugins(ViModePlugin);
        app.add_plugins(ViActionPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.init_resource::<PendingPaneOutput>();
        app.add_message::<PaneOutput>();
        app.insert_resource(OrzmaConfigsResource(OrzmaConfigs::default()));
        app.insert_resource(Clipboard::in_memory());

        let pane_id = PaneId(99);
        let entity = app
            .world_mut()
            .spawn(TmuxPane {
                id: pane_id,
                dims: CellDims {
                    width: 20,
                    height: 6,
                    xoff: 0,
                    yoff: 0,
                },
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

        // Frame 1: `[vi-mode]` key table resolution (Startup) plus the real
        // tmux-pane attach bundle (TerminalHandle, Coalescer, TerminalTitle,
        // OrzmaTerminal, Node) — the same bundle a projected tmux pane gets.
        // No output has arrived yet.
        app.update();
        assert!(
            app.world().get::<TerminalHandle>(entity).is_some(),
            "tmux-pane bundle must be attached before seeding history"
        );

        // Frame 2: seed several screens of distinct, recognizable history via
        // a `PaneOutput` message, exactly how tmux's capture-pane seed and
        // subsequent `%output` reach a real pane's handle.
        let mut seed = String::new();
        for i in 0..40 {
            seed.push_str(&format!("SEEDLINE-{i:03}\r\n"));
        }
        app.world_mut()
            .resource_mut::<Messages<PaneOutput>>()
            .write(PaneOutput {
                pane: pane_id,
                data: seed.into_bytes(),
            });
        app.update();
        let tail_row0: String = app.world().get::<TerminalGrid>(entity).unwrap().cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            tail_row0.contains("SEEDLINE-"),
            "pre-vi-mode grid must show the seeded live tail, got {tail_row0:?}"
        );

        // 1. Enter vi mode.
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        assert!(
            app.world().get::<ViModeState>(entity).is_some(),
            "entering vi mode must insert ViModeState"
        );
        let entered_vi = app
            .world()
            .get::<TerminalHandle>(entity)
            .unwrap()
            .current_modes()
            .contains(TermMode::VI);
        assert!(
            entered_vi,
            "entering vi mode must flip the handle into vi mode"
        );

        // 2. Scroll into the pre-seeded history via the shared vi applier.
        app.world_mut().trigger(ViScrollRequest {
            entity,
            kind: ViModeScroll::PageUp,
        });
        app.update();
        let scroll_offset = app
            .world()
            .get::<TerminalHandle>(entity)
            .unwrap()
            .vi_indicator_snapshot()
            .scroll_offset;
        assert!(
            scroll_offset > 0,
            "PageUp must scroll the handle back into scrollback history"
        );

        // `vi_motion`/`scroll` arm the Coalescer instead of emitting
        // immediately (`stage_full_damage_and_arm`); sleep past its 3ms IDLE
        // debounce so `flush_due_terminals` actually flushes the scrolled
        // frame to the render-facing TerminalGrid — proving the render leg of
        // the pipeline (not just the handle's own internal state) reflects
        // the scroll.
        std::thread::sleep(Duration::from_millis(15));
        app.update();
        let scrolled_row0: String = app.world().get::<TerminalGrid>(entity).unwrap().cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            scrolled_row0.contains("SEEDLINE-"),
            "scrolled grid row must still show seeded history text, got {scrolled_row0:?}"
        );
        assert_ne!(
            scrolled_row0, tail_row0,
            "scrolling into history must change what the grid renders"
        );

        // 3. Toggle a selection, extend it, then yank.
        app.world_mut().trigger(ViSelectionToggleRequest {
            entity,
            ty: SelectionType::Simple,
        });
        app.update();
        app.world_mut().trigger(ViMotionRequest {
            entity,
            motion: ViMotion::Right,
        });
        app.update();
        app.world_mut().trigger(ViMotionRequest {
            entity,
            motion: ViMotion::Down,
        });
        app.update();

        let expected_yank = app
            .world()
            .get::<TerminalHandle>(entity)
            .unwrap()
            .selection_to_string();
        assert!(
            expected_yank
                .as_deref()
                .is_some_and(|t| t.contains("SEEDLINE")),
            "selection must cover seeded history text before yanking, got {expected_yank:?}"
        );

        app.world_mut().trigger(ViYankRequest { entity });
        app.update();

        assert!(
            app.world().get::<ViModeState>(entity).is_none(),
            "yank must exit vi mode"
        );
        let clipboard_text = app.world_mut().resource_mut::<Clipboard>().read();
        assert_eq!(
            clipboard_text, expected_yank,
            "yank must write exactly the selected text to the clipboard"
        );

        // 4. Exiting vi mode (via yank) snaps the handle back to the live
        // tail and leaves vi mode.
        let handle = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            handle.is_at_bottom(),
            "exiting vi mode via yank must snap the viewport back to the live tail"
        );
        assert!(
            !handle.current_modes().contains(TermMode::VI),
            "exiting vi mode must leave vi mode"
        );
    }
}
