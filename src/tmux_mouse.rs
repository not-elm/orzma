//! Mouse gesture arbiter for the tmux backend.
//!
//! Owns a single left-button state machine (`TmuxMouseGesture`) that reads raw
//! `MouseButtonInput` messages and issues `select-pane` on a focused press.
//! Divider-drag-to-resize is handled in the same state machine: a press within
//! `divider_grab_tolerance_px` of a divider line enters `Resizing` state; each
//! frame while held the pointer's major-axis cell coordinate is converted to an
//! absolute target size and a `resize-pane -x/-y` command is sent whenever that
//! target changes. The send is pointer-driven (not a reaction to
//! `%layout-change`), so there is no resize feedback loop, and the absolute
//! `-x/-y` form is idempotent so re-sends cannot accumulate drift.
//!
//! A press on a pane that drags past `drag_threshold_px` enters `Selecting`
//! state: copy mode is auto-entered on the pane under the press, then the copy
//! cursor is positioned to the press cell and a selection begun, extending as
//! the pointer moves and copying to the clipboard (via the tmux paste buffer)
//! on release. All copy-mode commands are pane-targeted (`send-keys -X -t %id`)
//! so they act on the pressed pane regardless of the client's active pane.

use crate::configs::OzmuxConfigsResource;
use crate::inline_webview::{InlineWebview, inline_hit_at, inline_local_dip};
use crate::input::InputPhase;
use crate::input::current_modifiers;
use crate::input::hyperlink::{link_modifier_held, should_open_at, try_open_uri};
use crate::osc_webview::NonInteractive;
use crate::tmux_copy_mode::{CopyModeSnapshot, cell_at_pane, cursor_deltas};
use crate::tmux_pane_hit::{cell_at_local, phys_to_pane_local, tmux_pane_at_phys};
use crate::tmux_picker::SessionPicker;
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::CopyPrompt;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::mouse::MouseButtonInput;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozma_tty_renderer::schema::TerminalGrid;
use ozmux_tmux::{
    ActiveWindow, CopyModeQueries, CopyQueryKind, PaneId, TmuxConnection, TmuxDividers, TmuxPane,
    resize_pane_x_command, resize_pane_y_command, select_pane_command, show_buffer_command,
};
use std::time::Duration;
use tmux_control_parser::{Divider, DividerAxis};

/// Bevy plugin that registers the tmux mouse gesture arbiter.
pub(crate) struct OzmuxTmuxMousePlugin;

impl Plugin for OzmuxTmuxMousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxMouseGesture>();
        app.add_systems(Update, arbiter.in_set(InputPhase::Dispatch));
        app.add_systems(
            Update,
            forward_tmux_inline_mouse_moves.in_set(InputPhase::Hover),
        );
    }
}

/// Modal-input gate: the resources whose presence means another surface owns
/// input and the arbiter must drain events without mutating tmux. The
/// focused-webview case is NOT gated here — the inline click pre-step
/// (`route_tmux_inline_left_click`) owns webview focus instead.
#[derive(SystemParam)]
struct ModalGate<'w> {
    picker: Res<'w, SessionPicker>,
    copy_prompt: Res<'w, CopyPrompt>,
}

/// Hyperlink-open inputs for the arbiter's Cmd/Ctrl-click branch, bundled to
/// stay within Bevy's system-parameter limit.
#[derive(SystemParam)]
struct HyperlinkGate<'w, 's> {
    grids: Query<'w, 's, &'static TerminalGrid>,
    keys: Res<'w, ButtonInput<KeyCode>>,
}

/// Word- vs line-granularity selection for a double/triple click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MultiSelectKind {
    Word,
    Line,
}

/// The current phase of a left-button gesture over a tmux pane.
#[derive(Default, Debug, PartialEq)]
enum GestureState {
    /// No button is held; the arbiter is waiting for the next press.
    #[default]
    Idle,
    /// Left button is held; `pane`/`pane_id` is the pane that received the press
    /// and `origin_phys` is the physical-pixel cursor position at press time.
    /// Becomes `Selecting` once the pointer drags past `drag_threshold_px`.
    Pressed {
        pane: Entity,
        pane_id: PaneId,
        origin_phys: Vec2,
        click_count: u8,
    },
    /// A double/triple click awaiting its copy-mode snapshot before positioning
    /// the copy cursor and selecting a word/line.
    PendingMultiSelect {
        pane: Entity,
        pane_id: PaneId,
        cell: (u16, u16),
        kind: MultiSelectKind,
    },
    /// Selecting text in a pane via tmux copy-mode (entered on drag-start).
    Selecting {
        pane: Entity,
        pane_id: PaneId,
        anchor: (u16, u16),
        begun: bool,
        last_target: Option<(u16, u16)>,
    },
    /// Dragging a divider to resize its primary pane.
    Resizing {
        divider: Divider,
        /// The primary pane's fixed near edge (xoff for vertical, yoff for horizontal), cells.
        near: i32,
        /// Last absolute size (cells) we issued a resize for.
        last_sent: u32,
        /// Whether any `resize-pane` was actually sent (i.e. the pointer
        /// dragged). A press that never drags is a click: on release it falls
        /// back to focusing the pane under the cursor, because the grab zone
        /// overlaps the adjacent pane bodies.
        resized: bool,
    },
}

/// Tracks the current left-button gesture over a tmux pane.
#[derive(Resource, Default)]
pub(crate) struct TmuxMouseGesture {
    state: GestureState,
    click: ClickTracker,
    /// The in-flight inline-webview press: the child a left press inside an
    /// interactive inline rect was forwarded to, so the matching release's
    /// click-up routes to the SAME child even if the pointer drifted off-rect.
    inline_press: Option<Entity>,
}

/// Tracks consecutive-click count using a timeout + positional drift gate.
#[derive(Default)]
struct ClickTracker {
    last: Option<(Duration, Vec2, u8)>,
}

impl ClickTracker {
    /// Registers a press at `now` / `pos` and returns the resulting click count
    /// (1 = single, 2 = double, 3 = triple, capped at 3). `cfg` is
    /// `(double_click_timeout, click_drift_px)` in logical units.
    fn register(&mut self, now: Duration, pos: Vec2, cfg: (Duration, f32)) -> u8 {
        let (timeout, drift) = cfg;
        let count = match self.last {
            Some((t, p, c)) if now.saturating_sub(t) <= timeout && p.distance(pos) <= drift => {
                (c + 1).min(3)
            }
            _ => 1,
        };
        self.last = Some((now, pos, count));
        count
    }
}

/// Returns the `(Entity, PaneId)` of the first `TmuxPane` whose `ComputedNode`
/// contains `cursor_phys` (physical px), or `None` when no pane covers the point.
fn pane_under_cursor(
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    cursor_phys: Vec2,
) -> Option<(Entity, PaneId)> {
    panes
        .iter()
        .find(|(_, _, node, transform)| node.contains_point(**transform, cursor_phys))
        .map(|(entity, pane, _, _)| (entity, pane.id))
}

/// Returns the divider whose grab zone contains `cursor_phys` (physical px),
/// given physical cell metrics and a tolerance in physical px. The grab zone is
/// the divider's reserved gap cell (`[pos, pos+1)` on the major axis) expanded by
/// `tol_phys` on each side, intersected with the divider's span on the
/// perpendicular axis. This matches the visible handle bar (which fills the gap
/// cell) so the handle, the resize grab, and the hover cursor coincide.
pub(crate) fn divider_at(
    dividers: &[Divider],
    cursor_phys: Vec2,
    cell_w: f32,
    cell_h: f32,
    tol_phys: f32,
) -> Option<Divider> {
    dividers.iter().copied().find(|d| match d.axis {
        DividerAxis::Vertical => {
            let gap0 = d.pos as f32 * cell_w;
            let gap1 = (d.pos + 1) as f32 * cell_w;
            let span0 = d.span_start as f32 * cell_h;
            let span1 = d.span_end as f32 * cell_h;
            cursor_phys.x >= gap0 - tol_phys
                && cursor_phys.x <= gap1 + tol_phys
                && cursor_phys.y >= span0
                && cursor_phys.y < span1
        }
        DividerAxis::Horizontal => {
            let gap0 = d.pos as f32 * cell_h;
            let gap1 = (d.pos + 1) as f32 * cell_h;
            let span0 = d.span_start as f32 * cell_w;
            let span1 = d.span_end as f32 * cell_w;
            cursor_phys.y >= gap0 - tol_phys
                && cursor_phys.y <= gap1 + tol_phys
                && cursor_phys.x >= span0
                && cursor_phys.x < span1
        }
    })
}

/// New absolute size (cells) for a divider's primary pane given the pointer's
/// cell coordinate on the major axis. The pane's near edge stays fixed; its far
/// edge follows the pointer. Clamped to at least 1.
fn resize_target_size(near: i32, pointer_cell: i32) -> u32 {
    (pointer_cell - near).max(1) as u32
}

/// Interprets raw left-button messages into tmux `select-pane`, `resize-pane`,
/// or copy-mode selection commands.
///
/// On each `Pressed` event the cursor's physical position is hit-tested: a
/// press within a divider's grab zone (whose primary pane has geometry) enters
/// `Resizing`; otherwise the pane under the cursor is focused (`select-pane`)
/// and the state becomes `Pressed`; a press over nothing leaves `Idle`. While
/// `Pressed`, a pointer that drags past `drag_threshold_px` auto-enters tmux
/// copy mode on the pressed pane and transitions to `Selecting`, which positions
/// the copy cursor to the press cell, begins a selection, and extends it as the
/// pointer moves (all pane-targeted via `send-keys -X -t %id`). Each frame while
/// `Resizing` the pointer's major-axis cell coordinate is mapped to an absolute
/// target size and sent as `resize-pane -x/-y` whenever the target changes; the
/// send is pointer-driven (not a reaction to `%layout-change`) so there is no
/// resize feedback loop. On `Released` from `Selecting` a begun selection is
/// copied and bridged to the clipboard; a `Resizing` release that never dragged
/// is treated as a click and focuses the pane under the cursor; any other
/// release returns the state to `Idle`. When the primary window is not focused,
/// or a modal (picker / copy-search prompt) owns input, queued events are
/// drained and the state is reset.
///
/// Each left press/release is first offered to the inline-webview layer
/// (`route_tmux_inline_left_click`): a press inside an interactive inline rect
/// focuses + forwards to the child's CEF browser and never reaches the tmux
/// gesture pipeline; a press outside every rect drops inline focus and falls
/// through to the normal pane gesture.
fn arbiter(
    mut gesture: ResMut<TmuxMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut commands: Commands,
    mut queries: ResMut<CopyModeQueries>,
    mut inline_route: TmuxInlineRouteParams,
    connection: NonSend<TmuxConnection>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    hyperlink: HyperlinkGate,
    dividers_q: Query<&TmuxDividers, With<ActiveWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    configs: Option<Res<OzmuxConfigsResource>>,
    modals: ModalGate,
    copy_modes: Query<(), With<CopyModeState>>,
    snapshots: Query<&CopyModeSnapshot>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let ModalGate {
        picker,
        copy_prompt,
    } = &modals;
    let Ok(window) = windows.single() else {
        buttons.clear();
        gesture.state = GestureState::Idle;
        return;
    };
    if !window.focused {
        buttons.clear();
        gesture.state = GestureState::Idle;
        return;
    }
    // NOTE: a gesture behind a picker / copy-search prompt must not mutate
    // tmux. The focused-webview case is NOT drained here — the inline click
    // pre-step below owns focus (in-rect press keeps it, off-rect press
    // releases it and drives tmux).
    if picker.open || copy_prompt.open.is_some() {
        buttons.clear();
        gesture.state = GestureState::Idle;
        return;
    }

    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);

    let (grab_tol_logical, drag_threshold_logical, dbl_click_ms, click_drift) = configs
        .as_deref()
        .map(|c| {
            (
                c.mouse.divider_grab_tolerance_px,
                c.mouse.drag_threshold_px,
                c.mouse.double_click_timeout_ms,
                c.mouse.click_drift_px,
            )
        })
        .unwrap_or((4.0, 4.0, 400, 8.0));
    let tol_phys = grab_tol_logical * scale;
    let drag_threshold_phys = drag_threshold_logical * scale;

    let dividers: &[Divider] = dividers_q.single().map(|d| d.0.as_slice()).unwrap_or(&[]);

    for ev in buttons.read() {
        if ev.button != bevy::input::mouse::MouseButton::Left {
            continue;
        }
        if let Some(cursor_phys) = window.cursor_position().map(|c| c * scale)
            && let Some((terminal, _pane_id, local_phys)) = tmux_pane_at_phys(&panes, cursor_phys)
            && route_tmux_inline_left_click(
                &mut gesture,
                &mut inline_route,
                &panes,
                terminal,
                local_phys,
                cursor_phys,
                ev.state,
                cell_w,
                cell_h,
                scale,
            )
        {
            continue;
        }
        match ev.state {
            ButtonState::Pressed => {
                let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
                    continue;
                };
                let pane_under = pane_under_cursor(&panes, cursor_phys);
                // NOTE: this open-check must run (and `continue`) before the
                // divider / select-pane logic below — otherwise a
                // modifier-click on a link would also focus/select the pane in
                // the same press.
                if let Some((pane_e, _pane_id)) = pane_under {
                    let mods = current_modifiers(&hyperlink.keys);
                    if link_modifier_held(&mods)
                        && let Some((_, _, node, transform)) =
                            panes.iter().find(|(e, _, _, _)| *e == pane_e)
                        && let Some(local) = phys_to_pane_local(node, transform, cursor_phys)
                        && let Ok(grid) = hyperlink.grids.get(pane_e)
                    {
                        let (col, row, _) =
                            cell_at_local(local, cell_w, cell_h, grid.cols, grid.rows);
                        if let Some(uri) = should_open_at(
                            grid,
                            row.saturating_sub(1) as u16,
                            col.saturating_sub(1) as u16,
                            ozma_tty_engine::MouseButtonKind::Left,
                            ozma_tty_engine::ButtonEventKind::Press,
                            true,
                        ) {
                            try_open_uri(uri.as_str());
                            continue;
                        }
                    }
                }
                // Resolve a divider grab to its primary pane's near edge + size.
                // A divider whose primary pane has no projected geometry yet
                // cannot be resized, so it falls through to a pane focus rather
                // than entering Resizing with a bogus (0) baseline.
                let resize =
                    divider_at(dividers, cursor_phys, cell_w, cell_h, tol_phys).and_then(|d| {
                        panes
                            .iter()
                            .find(|(_, p, _, _)| p.id == d.primary)
                            .map(|(_, p, _, _)| match d.axis {
                                DividerAxis::Vertical => (d, p.dims.xoff, p.dims.width),
                                DividerAxis::Horizontal => (d, p.dims.yoff, p.dims.height),
                            })
                    });
                if let Some((divider, near, last_sent)) = resize {
                    gesture.state = GestureState::Resizing {
                        divider,
                        near,
                        last_sent,
                        resized: false,
                    };
                } else if let Some((pane, pane_id)) = pane_under {
                    if let Some(client) = connection.client() {
                        let cmd = select_pane_command(pane_id);
                        if let Err(e) = client.handle().send(&cmd) {
                            tracing::warn!(?e, pane = pane_id.0, "select-pane send failed");
                        }
                    }
                    let now = time.elapsed();
                    let cursor_logical = cursor_phys / scale;
                    let click_cfg = (Duration::from_millis(dbl_click_ms as u64), click_drift);
                    let count = gesture.click.register(now, cursor_logical, click_cfg);
                    gesture.state = GestureState::Pressed {
                        pane,
                        pane_id,
                        origin_phys: cursor_phys,
                        click_count: count,
                    };
                }
            }
            ButtonState::Released => {
                let prior = std::mem::replace(&mut gesture.state, GestureState::Idle);
                match prior {
                    // Only copy when a selection was actually begun. A drag that
                    // released before the copy-mode snapshot arrived never sent
                    // `begin-selection`; copying then would clobber the system
                    // clipboard with the stale paste buffer.
                    GestureState::Selecting { pane_id, begun, .. } if begun => {
                        if let Some(client) = connection.client() {
                            let handle = client.handle();
                            let copy = target_copy_cmd(pane_id, "send-keys -X copy-selection");
                            if let Err(e) = handle.send(&copy) {
                                tracing::warn!(
                                    ?e,
                                    pane = pane_id.0,
                                    "drag-select copy-selection send failed"
                                );
                            } else {
                                match handle.send(&show_buffer_command()) {
                                    Ok(id) => queries.register(id, pane_id, CopyQueryKind::Buffer),
                                    Err(e) => {
                                        tracing::warn!(
                                            ?e,
                                            pane = pane_id.0,
                                            "drag-select show-buffer send failed"
                                        )
                                    }
                                }
                            }
                        }
                    }
                    GestureState::Pressed {
                        pane,
                        pane_id,
                        origin_phys,
                        click_count,
                    } if click_count >= 2 => {
                        let Some(client) = connection.client() else {
                            break;
                        };
                        // Resolve the click cell BEFORE entering copy mode, so a
                        // failed lookup cannot leave the pane stuck in copy mode
                        // with no PendingMultiSelect to drive (and exit) it.
                        let Ok((_, p, node, transform)) = panes.get(pane) else {
                            break;
                        };
                        let cols = p.dims.width as u16;
                        let rows = p.dims.height as u16;
                        let Some(cell) =
                            cell_at_pane(node, transform, origin_phys, cell_w, cell_h, cols, rows)
                        else {
                            break;
                        };
                        if copy_modes.get(pane).is_err() {
                            if let Err(e) = client
                                .handle()
                                .send(&format!("copy-mode -t %{}", pane_id.0))
                            {
                                tracing::warn!(
                                    ?e,
                                    pane = pane_id.0,
                                    "multi-click copy-mode entry send failed"
                                );
                                break;
                            }
                            commands.entity(pane).insert(CopyModeState);
                        }
                        let kind = if click_count == 2 {
                            MultiSelectKind::Word
                        } else {
                            MultiSelectKind::Line
                        };
                        gesture.state = GestureState::PendingMultiSelect {
                            pane,
                            pane_id,
                            cell,
                            kind,
                        };
                    }
                    // A divider press that never dragged is a click: the grab
                    // zone overlaps the adjacent pane bodies, so focus the pane
                    // under the cursor instead of silently doing nothing.
                    GestureState::Resizing { resized, .. } if !resized => {
                        if let Some(cursor_phys) = window.cursor_position().map(|c| c * scale)
                            && let Some((_, pane_id)) = pane_under_cursor(&panes, cursor_phys)
                            && let Some(client) = connection.client()
                            && let Err(e) = client.handle().send(&select_pane_command(pane_id))
                        {
                            tracing::warn!(
                                ?e,
                                pane = pane_id.0,
                                "divider-click select-pane failed"
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if let GestureState::Pressed {
        pane,
        pane_id,
        origin_phys,
        ..
    } = gesture.state
    {
        let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
            return;
        };
        if cursor_phys.distance(origin_phys) <= drag_threshold_phys {
            return;
        }
        let Ok((_, p, node, transform)) = panes.get(pane) else {
            gesture.state = GestureState::Idle;
            return;
        };
        let cols = p.dims.width as u16;
        let rows = p.dims.height as u16;
        if copy_modes.get(pane).is_err() {
            if let Some(client) = connection.client() {
                if let Err(e) = client
                    .handle()
                    .send(&format!("copy-mode -t %{}", pane_id.0))
                {
                    tracing::warn!(?e, pane = pane_id.0, "copy-mode entry send failed");
                    return;
                }
            } else {
                return;
            }
            commands.entity(pane).insert(CopyModeState);
        }
        let Some(anchor) = cell_at_pane(node, transform, origin_phys, cell_w, cell_h, cols, rows)
        else {
            return;
        };
        gesture.state = GestureState::Selecting {
            pane,
            pane_id,
            anchor,
            begun: false,
            last_target: None,
        };
        return;
    }

    if let GestureState::Selecting {
        pane,
        pane_id,
        anchor,
        begun,
        last_target,
    } = &mut gesture.state
    {
        let Ok((_, p, node, transform)) = panes.get(*pane) else {
            gesture.state = GestureState::Idle;
            return;
        };
        // NOTE: the snapshot is the copy cursor the relative cursor_deltas are
        // computed from. copy-mode was just entered, so the first state refresh
        // round-trips over a frame; without a snapshot, defer to a later frame
        // rather than computing deltas off a stale/absent cursor.
        let Ok(snapshot_cursor) = snapshots.get(*pane).map(|s| (s.0.cursor_x, s.0.cursor_y)) else {
            return;
        };
        let Some(client) = connection.client() else {
            return;
        };
        let handle = client.handle();
        let cols = p.dims.width as u16;
        let rows = p.dims.height as u16;
        let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
            return;
        };
        let Some(cell) = cell_at_pane(node, transform, cursor_phys, cell_w, cell_h, cols, rows)
        else {
            return;
        };

        if !*begun {
            for cmd in cursor_deltas(snapshot_cursor, *anchor) {
                if let Err(e) = handle.send(&target_copy_cmd(*pane_id, &cmd)) {
                    tracing::warn!(?e, pane = pane_id.0, "drag-select anchor delta send failed");
                }
            }
            if let Err(e) = handle.send(&target_copy_cmd(*pane_id, "send-keys -X begin-selection"))
            {
                tracing::warn!(
                    ?e,
                    pane = pane_id.0,
                    "drag-select begin-selection send failed"
                );
                return;
            }
            *begun = true;
            *last_target = Some(*anchor);
        } else if Some(cell) != *last_target {
            for cmd in cursor_deltas(snapshot_cursor, cell) {
                if let Err(e) = handle.send(&target_copy_cmd(*pane_id, &cmd)) {
                    tracing::warn!(?e, pane = pane_id.0, "drag-select extend delta send failed");
                }
            }
            *last_target = Some(cell);
        }
        return;
    }

    if let GestureState::PendingMultiSelect {
        pane,
        pane_id,
        cell,
        kind,
    } = gesture.state
    {
        if panes.get(pane).is_err() {
            gesture.state = GestureState::Idle;
            return;
        }
        let Ok(snapshot_cursor) = snapshots.get(pane).map(|s| (s.0.cursor_x, s.0.cursor_y)) else {
            return;
        };
        let Some(client) = connection.client() else {
            return;
        };
        let handle = client.handle();
        for cmd in multi_select_commands(kind, snapshot_cursor, cell, pane_id) {
            if let Err(e) = handle.send(&cmd) {
                tracing::warn!(?e, pane = pane_id.0, "multi-select cmd send failed");
            }
        }
        match handle.send(&show_buffer_command()) {
            Ok(id) => queries.register(id, pane_id, CopyQueryKind::Buffer),
            Err(e) => {
                tracing::warn!(?e, pane = pane_id.0, "multi-select show-buffer send failed")
            }
        }
        gesture.state = GestureState::Idle;
        return;
    }

    if let GestureState::Resizing {
        divider,
        near,
        last_sent,
        resized,
    } = &mut gesture.state
    {
        let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
            return;
        };

        let pointer_cell = match divider.axis {
            DividerAxis::Vertical => (cursor_phys.x / cell_w).floor() as i32,
            DividerAxis::Horizontal => (cursor_phys.y / cell_h).floor() as i32,
        };

        let target = resize_target_size(*near, pointer_cell);

        // The pointer drives the send (not `%layout-change`), so there is no
        // resize feedback loop; emitting only on a new target cell yields at
        // most one absolute (idempotent) resize per frame. We do NOT gate on
        // the confirmed pane size catching up to `last_sent` — when tmux clamps
        // a resize the size never reaches the request, and such a gate would
        // wedge the drag for the rest of the gesture.
        if target == *last_sent {
            return;
        }

        let Some(client) = connection.client() else {
            return;
        };

        let cmd = match divider.axis {
            DividerAxis::Vertical => resize_pane_x_command(divider.primary, target),
            DividerAxis::Horizontal => resize_pane_y_command(divider.primary, target),
        };

        if let Err(e) = client.handle().send(&cmd) {
            tracing::warn!(?e, pane = divider.primary.0, "resize-pane send failed");
            return;
        }

        *last_sent = target;
        *resized = true;
    }
}

/// Inline-routing params for the arbiter, bundled to stay within Bevy's
/// system-parameter limit. `focused_webview` / `browsers` are optional so
/// CEF-less tests construct the system (state effects still apply).
#[derive(SystemParam)]
struct TmuxInlineRouteParams<'w, 's> {
    focused_webview: Option<ResMut<'w, FocusedWebview>>,
    children: Query<'w, 's, &'static Children>,
    inline: Query<'w, 's, (&'static InlineWebview, Has<NonInteractive>)>,
    inline_parents: Query<'w, 's, &'static ChildOf, With<InlineWebview>>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    browsers: Option<NonSend<'w, Browsers>>,
}

/// Routes a left press/release through the inline-webview layer, returning
/// `true` when the event was consumed and must NOT reach the tmux gesture
/// pipeline. A press inside an
/// interactive rect sets `FocusedWebview`, issues the UNGATED `set_focus`
/// BEFORE the gated `send_mouse_click` (CEF drops clicks to a browser with no
/// `focused_frame()`, so the first click would otherwise be swallowed),
/// forwards the press in DIP, and records the in-flight press; a press outside
/// every rect clears an inline `FocusedWebview` and returns `false`. Release
/// forwards the click-up to the recorded child (drift-tolerant) and clears.
#[expect(
    clippy::too_many_arguments,
    reason = "inline routing needs the gesture state, route params, and pointer geometry"
)]
fn route_tmux_inline_left_click(
    gesture: &mut TmuxMouseGesture,
    route: &mut TmuxInlineRouteParams,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    terminal: Entity,
    local_phys: Vec2,
    cursor_phys: Vec2,
    button_state: ButtonState,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> bool {
    match button_state {
        ButtonState::Pressed => {
            gesture.inline_press = None;
            let hit = route.overlay_rects.get(terminal).ok().and_then(|overlays| {
                inline_hit_at(
                    &route.children,
                    &route.inline,
                    overlays,
                    terminal,
                    local_phys,
                    cell_w_phys,
                    cell_h_phys,
                    scale,
                )
            });
            let Some(hit) = hit else {
                if let Some(focused) = route.focused_webview.as_deref_mut()
                    && focused
                        .0
                        .is_some_and(|current| route.inline_parents.contains(current))
                {
                    focused.0 = None;
                }
                return false;
            };
            if let Some(focused) = route.focused_webview.as_deref_mut()
                && focused.0 != Some(hit.child)
            {
                focused.0 = Some(hit.child);
            }
            if let Some(browsers) = route.browsers.as_deref() {
                browsers.set_focus(&hit.child, true);
                browsers.send_mouse_click(&hit.child, hit.local_dip, PointerButton::Primary, false);
            }
            gesture.inline_press = Some(hit.child);
            true
        }
        ButtonState::Released => {
            let Some(child) = gesture.inline_press.take() else {
                return false;
            };
            if let Some(browsers) = route.browsers.as_deref()
                && let Some(dip) = tmux_inline_release_dip(
                    route,
                    panes,
                    child,
                    cursor_phys,
                    cell_w_phys,
                    cell_h_phys,
                    scale,
                )
            {
                browsers.send_mouse_click(&child, dip, PointerButton::Primary, true);
            }
            true
        }
    }
}

/// Webview-local DIP for a release on `child`, WITHOUT containment (a pointer
/// that drifted off the rect still produces a release position). `None` when
/// the child/terminal/rect chain is gone.
fn tmux_inline_release_dip(
    route: &TmuxInlineRouteParams,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    child: Entity,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> Option<Vec2> {
    let terminal = route.inline_parents.get(child).ok()?.parent();
    let (_, _, node, transform) = panes.get(terminal).ok()?;
    let local_phys = phys_to_pane_local(node, transform, cursor_phys)?;
    let (view, _) = route.inline.get(child).ok()?;
    inline_local_dip(
        route.overlay_rects.get(terminal).ok()?,
        view.slot,
        local_phys,
        cell_w_phys,
        cell_h_phys,
        scale,
    )
}

/// Forwards pointer motion over an interactive inline rect of a tmux pane to
/// the child's CEF browser (`send_mouse_move`, webview-local DIP), forwarding
/// whatever mouse buttons are held so the one system serves both hover and an
/// in-rect drag. `CursorMoved`-driven (one forward per frame, latest position), and
/// focus-gated inside `bevy_cef` so motion over an unfocused browser is
/// dropped browser-side. `Browsers` is optional so CEF-less tests construct it.
fn forward_tmux_inline_mouse_moves(
    mut cursor_msg: MessageReader<CursorMoved>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: Query<&Children>,
    inline: Query<(&InlineWebview, Has<NonInteractive>)>,
    overlay_rects: Query<&TerminalOverlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    picker: Res<SessionPicker>,
    copy_prompt: Res<CopyPrompt>,
    browsers: Option<NonSend<Browsers>>,
) {
    let Some(moved) = cursor_msg.read().last() else {
        return;
    };
    if picker.open || copy_prompt.open.is_some() {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = window.scale_factor();
    let cursor_phys = moved.position * scale;
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some((terminal, _pane_id, local_phys)) = tmux_pane_at_phys(&panes, cursor_phys) else {
        return;
    };
    let Ok(overlays) = overlay_rects.get(terminal) else {
        return;
    };
    let Some(hit) = inline_hit_at(
        &children, &inline, overlays, terminal, local_phys, cell_w, cell_h, scale,
    ) else {
        return;
    };
    if let Some(browsers) = browsers.as_ref() {
        browsers.send_mouse_move(
            &hit.child,
            mouse_buttons.get_pressed(),
            hit.local_dip,
            false,
        );
    }
}

/// Inserts `-t %<id>` into a `send-keys -X ...` copy-mode command so it targets
/// a specific pane instead of the client's active pane. Non-`send-keys -X`
/// commands are returned unchanged.
fn target_copy_cmd(pane: PaneId, cmd: &str) -> String {
    match cmd.strip_prefix("send-keys -X") {
        Some(rest) => format!("send-keys -X -t %{}{}", pane.0, rest),
        None => cmd.to_string(),
    }
}

/// Pane-targeted copy-mode commands to position the copy cursor at `cell`
/// (relative to the snapshot cursor) and select a word/line. Does NOT include
/// `show-buffer` — the caller sends that separately to register the reply.
fn multi_select_commands(
    kind: MultiSelectKind,
    snapshot_cursor: (u16, u16),
    cell: (u16, u16),
    pane: PaneId,
) -> Vec<String> {
    let mut out: Vec<String> = cursor_deltas(snapshot_cursor, cell)
        .iter()
        .map(|c| target_copy_cmd(pane, c))
        .collect();
    let select = match kind {
        MultiSelectKind::Word => "send-keys -X select-word",
        MultiSelectKind::Line => "send-keys -X select-line",
    };
    out.push(target_copy_cmd(pane, select));
    out.push(target_copy_cmd(pane, "send-keys -X copy-selection"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::mouse::MouseButtonInput;
    use ozma_tty_renderer::CellMetrics;

    #[test]
    fn gesture_state_default_is_idle() {
        assert_eq!(GestureState::default(), GestureState::Idle);
    }

    fn vdiv(primary: u32, pos: i32, s: i32, e: i32) -> Divider {
        Divider {
            axis: DividerAxis::Vertical,
            primary: PaneId(primary),
            pos,
            span_start: s,
            span_end: e,
        }
    }

    #[test]
    fn hit_test_grabs_vertical_divider_within_tolerance() {
        let ds = [vdiv(1, 40, 0, 24)];
        let hit = divider_at(&ds, Vec2::new(322.0, 100.0), 8.0, 16.0, 4.0);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().primary, PaneId(1));
    }

    #[test]
    fn hit_test_misses_outside_tolerance() {
        let ds = [vdiv(1, 40, 0, 24)];
        assert!(divider_at(&ds, Vec2::new(360.0, 100.0), 8.0, 16.0, 4.0).is_none());
    }

    #[test]
    fn hit_test_misses_outside_span() {
        let ds = [vdiv(1, 40, 0, 12)];
        assert!(divider_at(&ds, Vec2::new(320.0, 208.0), 8.0, 16.0, 4.0).is_none());
    }

    #[test]
    fn hit_test_grabs_far_side_of_gap() {
        // Gap cell is column 40 = px [320, 328); the far side (x=327) is on the
        // visible handle and must grab even though it is >tol from the gap's
        // leading edge (320) — the zone spans the whole gap cell, not ±tol.
        let ds = [vdiv(1, 40, 0, 24)];
        let hit = divider_at(&ds, Vec2::new(327.0, 100.0), 8.0, 16.0, 4.0);
        assert_eq!(hit.map(|d| d.primary), Some(PaneId(1)));
    }

    #[test]
    fn click_count_increments_within_timeout_and_drift() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(
            t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg),
            1
        );
        assert_eq!(
            t.register(Duration::from_millis(200), Vec2::new(11.0, 11.0), cfg),
            2
        );
        assert_eq!(
            t.register(Duration::from_millis(350), Vec2::new(12.0, 10.0), cfg),
            3
        );
    }

    #[test]
    fn click_count_resets_after_timeout() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(
            t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg),
            1
        );
        assert_eq!(
            t.register(Duration::from_millis(500), Vec2::new(10.0, 10.0), cfg),
            1
        );
    }

    #[test]
    fn click_count_resets_after_drift() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(
            t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg),
            1
        );
        assert_eq!(
            t.register(Duration::from_millis(100), Vec2::new(40.0, 40.0), cfg),
            1
        );
    }

    #[test]
    fn resize_target_size_follows_pointer() {
        assert_eq!(resize_target_size(0, 50), 50);
        assert_eq!(resize_target_size(10, 25), 15);
        assert_eq!(resize_target_size(0, 0), 1);
    }

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
    fn left_press_without_cursor_stays_idle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.init_resource::<TmuxMouseGesture>();
        app.init_resource::<CopyModeQueries>();
        app.init_resource::<SessionPicker>();
        app.init_resource::<CopyPrompt>();
        app.init_resource::<FocusedWebview>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(test_metrics());
        app.add_systems(Update, arbiter);
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: bevy::input::mouse::MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle
        );
    }

    #[test]
    fn multi_select_word_commands() {
        let cmds = multi_select_commands(MultiSelectKind::Word, (0, 0), (3, 0), PaneId(2));
        assert_eq!(
            cmds,
            vec![
                "send-keys -X -t %2 -N 3 cursor-right".to_string(),
                "send-keys -X -t %2 select-word".to_string(),
                "send-keys -X -t %2 copy-selection".to_string(),
            ]
        );
    }

    #[test]
    fn target_copy_cmd_inserts_pane_target_after_send_keys_x() {
        assert_eq!(
            target_copy_cmd(PaneId(2), "send-keys -X begin-selection"),
            "send-keys -X -t %2 begin-selection",
        );
    }

    #[test]
    fn target_copy_cmd_preserves_flags_after_send_keys_x() {
        assert_eq!(
            target_copy_cmd(PaneId(2), "send-keys -X -N 3 cursor-right"),
            "send-keys -X -t %2 -N 3 cursor-right",
        );
    }

    #[test]
    fn target_copy_cmd_passes_non_matching_through() {
        assert_eq!(
            target_copy_cmd(PaneId(2), "copy-mode -t %2"),
            "copy-mode -t %2",
        );
    }

    fn make_arbiter_inline_app() -> (App, Entity, Entity) {
        use bevy::window::WindowResolution;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.init_resource::<TmuxMouseGesture>();
        app.init_resource::<CopyModeQueries>();
        app.init_resource::<SessionPicker>();
        app.init_resource::<CopyPrompt>();
        app.init_resource::<FocusedWebview>();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.insert_resource(test_metrics());
        app.add_systems(Update, arbiter);

        // Pane host node at window center (400, 300), size 800x600 → top-left
        // at (0, 0). Rect rows 2..12, cols 3..43 → phys y 32..192, x 24..344 at
        // 8x16 px.
        let mut overlays = TerminalOverlays::default();
        overlays.rects[0] = IVec4::new(2, 3, 10, 40);
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 100,
                        height: 37,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                ComputedNode {
                    size: Vec2::new(800.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(400.0, 300.0),
                overlays,
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                InlineWebview {
                    view_id: "inline".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();

        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        (app, pane, child)
    }

    fn set_cursor(app: &mut App, phys: Vec2) {
        use bevy::math::DVec2;

        let win = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Window>(win)
            .unwrap()
            .set_physical_cursor_position(Some(DVec2::new(phys.x as f64, phys.y as f64)));
    }

    fn write_button(app: &mut App, button: bevy::input::mouse::MouseButton, state: ButtonState) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button,
                state,
                window: Entity::PLACEHOLDER,
            });
    }

    #[test]
    fn inline_press_focuses_child_and_consumes() {
        let (mut app, _pane, child) = make_arbiter_inline_app();
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "a press inside an interactive inline rect must focus that child"
        );
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "a consumed inline press must NOT arm a Pressed/Selecting gesture"
        );
    }

    #[test]
    fn move_resolves_inline_child_over_rect() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut app, _pane, child) = make_arbiter_inline_app();
        let hit = app
            .world_mut()
            .run_system_once(
                move |panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
                      children: Query<&Children>,
                      inline: Query<(&InlineWebview, Has<NonInteractive>)>,
                      overlays: Query<&TerminalOverlays>| {
                    let (terminal, _pane_id, local) =
                        tmux_pane_at_phys(&panes, Vec2::new(40.0, 48.0)).unwrap();
                    inline_hit_at(
                        &children,
                        &inline,
                        overlays.get(terminal).unwrap(),
                        terminal,
                        local,
                        8.0,
                        16.0,
                        1.0,
                    )
                    .map(|h| h.child)
                },
            )
            .unwrap();
        assert_eq!(
            hit,
            Some(child),
            "pointer over the rect must resolve the inline child"
        );
    }

    #[test]
    fn move_resolves_nothing_off_rect() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut app, _pane, _child) = make_arbiter_inline_app();
        let hit = app
            .world_mut()
            .run_system_once(
                |panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
                 children: Query<&Children>,
                 inline: Query<(&InlineWebview, Has<NonInteractive>)>,
                 overlays: Query<&TerminalOverlays>| {
                    let (terminal, _pane_id, local) =
                        tmux_pane_at_phys(&panes, Vec2::new(400.0, 400.0)).unwrap();
                    inline_hit_at(
                        &children,
                        &inline,
                        overlays.get(terminal).unwrap(),
                        terminal,
                        local,
                        8.0,
                        16.0,
                        1.0,
                    )
                    .map(|h| h.child)
                },
            )
            .unwrap();
        assert_eq!(
            hit, None,
            "pointer over terminal text must resolve no inline child"
        );
    }

    #[test]
    fn inline_off_rect_press_releases_focus_and_falls_through() {
        let (mut app, pane, child) = make_arbiter_inline_app();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "an off-rect press must release inline focus"
        );
        assert!(
            matches!(
                app.world().resource::<TmuxMouseGesture>().state,
                GestureState::Pressed { pane: p, .. } if p == pane
            ),
            "an off-rect press must fall through to the normal pane gesture"
        );
    }
}
