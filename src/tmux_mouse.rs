//! Mouse gesture arbiter for the tmux backend.
//!
//! Owns a single left-button state machine (`TmuxMouseGesture`) that reads raw
//! `MouseButtonInput` messages and issues `select-pane` on a focused press.
//! Divider-drag-to-resize is handled in the same state machine: a press within
//! `divider_grab_tolerance_px` of a divider line enters `Resizing` state; each
//! frame while held the pointer's major-axis cell coordinate is converted to an
//! absolute target size and an `resize-pane -x/-y` command is sent when the
//! target differs from the last-sent size and the prior resize has been
//! confirmed by `%layout-change` (one-in-flight throttle).
//!
//! A press on a pane that drags past `drag_threshold_px` enters `Selecting`
//! state: copy mode is auto-entered on the pane under the press, then the copy
//! cursor is positioned to the press cell and a selection begun, extending as
//! the pointer moves and copying to the clipboard (via the tmux paste buffer)
//! on release. All copy-mode commands are pane-targeted (`send-keys -X -t %id`)
//! so they act on the pressed pane regardless of the client's active pane.

use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::tmux_copy_mode::{CopyModeSnapshot, cell_at_pane, cursor_deltas};
use crate::tmux_picker::SessionPicker;
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::CopyPrompt;
use bevy::input::ButtonState;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use ozma_tty_renderer::TerminalCellMetricsResource;
use std::time::Duration;
use ozmux_tmux::{
    ActiveWindow, CopyModeQueries, CopyQueryKind, PaneId, TmuxConnection, TmuxDividers, TmuxPane,
    resize_pane_x_command, resize_pane_y_command, select_pane_command, show_buffer_command,
};
use tmux_control_parser::{Divider, DividerAxis};

/// Bevy plugin that registers the tmux mouse gesture arbiter.
pub(crate) struct OzmuxTmuxMousePlugin;

impl Plugin for OzmuxTmuxMousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxMouseGesture>();
        app.add_systems(Update, arbiter.in_set(InputPhase::Dispatch));
    }
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
        /// Resize commands emitted in the current frame (per-frame cap).
        commands_this_frame: u32,
    },
}

/// Tracks the current left-button gesture over a tmux pane.
#[derive(Resource, Default)]
pub(crate) struct TmuxMouseGesture {
    state: GestureState,
    click: ClickTracker,
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

/// What a left-press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    Divider(Divider),
    Pane(PaneId),
    None,
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
/// given physical cell metrics and a half-tolerance in physical px. The pointer
/// must be within `tol_phys` of the divider line on the major axis and inside
/// its span on the perpendicular axis.
fn divider_at(
    dividers: &[Divider],
    cursor_phys: Vec2,
    cell_w: f32,
    cell_h: f32,
    tol_phys: f32,
) -> Option<Divider> {
    dividers.iter().copied().find(|d| match d.axis {
        DividerAxis::Vertical => {
            let line = d.pos as f32 * cell_w;
            let span0 = d.span_start as f32 * cell_h;
            let span1 = d.span_end as f32 * cell_h;
            (cursor_phys.x - line).abs() <= tol_phys
                && cursor_phys.y >= span0
                && cursor_phys.y < span1
        }
        DividerAxis::Horizontal => {
            let line = d.pos as f32 * cell_h;
            let span0 = d.span_start as f32 * cell_w;
            let span1 = d.span_end as f32 * cell_w;
            (cursor_phys.y - line).abs() <= tol_phys
                && cursor_phys.x >= span0
                && cursor_phys.x < span1
        }
    })
}

/// Classifies a left-press into what it landed on. Dividers take precedence
/// over panes: if `cursor_phys` is within the grab zone of a divider, returns
/// `Press::Divider`; else returns `Press::Pane` if `pane_under` is `Some`,
/// else `Press::None`.
fn classify_press(
    dividers: &[Divider],
    pane_under: Option<PaneId>,
    cursor_phys: Vec2,
    cell_w: f32,
    cell_h: f32,
    tol_phys: f32,
) -> Press {
    if let Some(d) = divider_at(dividers, cursor_phys, cell_w, cell_h, tol_phys) {
        return Press::Divider(d);
    }
    pane_under.map(Press::Pane).unwrap_or(Press::None)
}

/// New absolute size (cells) for a divider's primary pane given the pointer's
/// cell coordinate on the major axis. The pane's near edge stays fixed; its far
/// edge follows the pointer. Clamped to at least 1.
fn resize_target_size(near: i32, pointer_cell: i32) -> u32 {
    (pointer_cell - near).max(1) as u32
}

/// Commands to move the tmux copy cursor from `cur` (visible col,row) to
/// `target` (visible col,row). The row uses absolute `goto-line` (idempotent,
/// drift-free); the column uses relative cursor motion. `history` and `scroll`
/// are `CopyState.history_size` / `scroll_position` for the absolute mapping
/// (`absolute_line = (history - scroll) + visible_row`).
// NOTE: kept for the real-tmux drag-select validation (plan Task 13); the
// arbiter currently uses relative cursor_deltas. dead_code is allowed (not
// expected) because the test build references it, so the lint only fires for
// the non-test build.
#[allow(dead_code)]
fn position_commands(
    cur: (u16, u16),
    target: (u16, u16),
    history: u32,
    scroll: u32,
) -> Vec<String> {
    let mut out = Vec::new();
    let top = history as i32 - scroll as i32;
    let target_line = (top + target.1 as i32).max(0);
    out.push(format!("send-keys -X goto-line {target_line}"));
    let dx = target.0 as i32 - cur.0 as i32;
    if dx > 0 {
        out.push(format!("send-keys -X -N {dx} cursor-right"));
    } else if dx < 0 {
        out.push(format!("send-keys -X -N {} cursor-left", -dx));
    }
    out
}

/// Interprets raw left-button messages into tmux `select-pane`, `resize-pane`,
/// or copy-mode selection commands.
///
/// On each `Pressed` event the cursor's physical position is classified via
/// `classify_press`: a divider hit enters `Resizing` state; a pane hit sends
/// `select-pane` and enters `Pressed`; a miss leaves the state `Idle`. While
/// `Pressed`, a pointer that drags past `drag_threshold_px` auto-enters tmux
/// copy mode on the pressed pane and transitions to `Selecting`, which positions
/// the copy cursor to the press cell, begins a selection, and extends it as the
/// pointer moves (all pane-targeted via `send-keys -X -t %id`). Each frame while
/// `Resizing` the pointer's major-axis cell coordinate is mapped to an absolute
/// target size and sent as `resize-pane -x/-y` when the target differs from the
/// last-sent size and the prior resize has been confirmed (one-in-flight
/// throttle). On `Released` from `Selecting` the selection is copied and bridged
/// to the clipboard; any other release returns the state to `Idle`. When the
/// primary window is not focused, or a modal (picker / copy-search prompt /
/// webview) owns input, queued events are drained and the state is reset.
fn arbiter(
    mut gesture: ResMut<TmuxMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut commands: Commands,
    mut queries: ResMut<CopyModeQueries>,
    connection: NonSend<TmuxConnection>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    dividers_q: Query<&TmuxDividers, With<ActiveWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    configs: Option<Res<OzmuxConfigsResource>>,
    picker: Res<SessionPicker>,
    copy_prompt: Res<CopyPrompt>,
    focused_webview: Res<FocusedWebview>,
    copy_modes: Query<(), With<CopyModeState>>,
    snapshots: Query<&CopyModeSnapshot>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
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
    // NOTE: a gesture behind a modal must not mutate tmux; mirror the keyboard
    // path. While a picker / copy-search prompt or a webview owns input, drain
    // the events so they do not replay later, and reset the gesture.
    if picker.open || copy_prompt.open.is_some() || focused_webview.0.is_some() {
        buttons.clear();
        gesture.state = GestureState::Idle;
        return;
    }

    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);

    let (grab_tol_logical, max_resize_per_frame, drag_threshold_logical, dbl_click_ms, click_drift) =
        configs
            .as_deref()
            .map(|c| {
                (
                    c.mouse.divider_grab_tolerance_px,
                    c.mouse.max_resize_commands_per_frame,
                    c.mouse.drag_threshold_px,
                    c.mouse.double_click_timeout_ms,
                    c.mouse.click_drift_px,
                )
            })
            .unwrap_or((4.0, 4, 4.0, 400, 8.0));
    let tol_phys = grab_tol_logical * scale;
    let drag_threshold_phys = drag_threshold_logical * scale;

    let dividers: &[Divider] = dividers_q.single().map(|d| d.0.as_slice()).unwrap_or(&[]);

    for ev in buttons.read() {
        if ev.button != bevy::input::mouse::MouseButton::Left {
            continue;
        }
        match ev.state {
            ButtonState::Pressed => {
                let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
                    continue;
                };
                let pane_under = pane_under_cursor(&panes, cursor_phys);
                match classify_press(
                    dividers,
                    pane_under.map(|(_, id)| id),
                    cursor_phys,
                    cell_w,
                    cell_h,
                    tol_phys,
                ) {
                    Press::Divider(d) => {
                        let (near, last_sent) = panes
                            .iter()
                            .find(|(_, p, _, _)| p.id == d.primary)
                            .map(|(_, p, _, _)| match d.axis {
                                DividerAxis::Vertical => (p.dims.xoff, p.dims.width),
                                DividerAxis::Horizontal => (p.dims.yoff, p.dims.height),
                            })
                            .unwrap_or_else(|| match d.axis {
                                DividerAxis::Vertical => (0, 0),
                                DividerAxis::Horizontal => (0, 0),
                            });
                        gesture.state = GestureState::Resizing {
                            divider: d,
                            near,
                            last_sent,
                            commands_this_frame: 0,
                        };
                    }
                    Press::Pane(pane_id) => {
                        if let Some(client) = connection.client() {
                            let cmd = select_pane_command(pane_id);
                            if let Err(e) = client.handle().send(&cmd) {
                                tracing::warn!(?e, pane = pane_id.0, "select-pane send failed");
                            }
                        }
                        let pane = pane_under.map(|(e, _)| e).unwrap_or(Entity::PLACEHOLDER);
                        let now = time.elapsed();
                        let cursor_logical = cursor_phys / scale;
                        let click_cfg = (
                            Duration::from_millis(dbl_click_ms as u64),
                            click_drift,
                        );
                        let count = gesture.click.register(now, cursor_logical, click_cfg);
                        gesture.state = GestureState::Pressed {
                            pane,
                            pane_id,
                            origin_phys: cursor_phys,
                            click_count: count,
                        };
                    }
                    Press::None => {}
                }
            }
            ButtonState::Released => {
                let prior = std::mem::replace(&mut gesture.state, GestureState::Idle);
                match prior {
                    GestureState::Selecting { pane_id, .. } => {
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
                                    Ok(id) => {
                                        queries.register(id, pane_id, CopyQueryKind::Buffer)
                                    }
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
                        let Some((_, p, node, transform)) =
                            panes.iter().find(|(e, _, _, _)| *e == pane)
                        else {
                            break;
                        };
                        let cols = p.dims.width as u16;
                        let rows = p.dims.height as u16;
                        let Some(cell) = cell_at_pane(
                            node,
                            transform,
                            origin_phys,
                            cell_w,
                            cell_h,
                            cols,
                            rows,
                        ) else {
                            break;
                        };
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
        let Some((_, p, node, transform)) = panes.iter().find(|(e, _, _, _)| *e == pane) else {
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
        let Some((_, p, node, transform)) = panes.iter().find(|(e, _, _, _)| *e == *pane) else {
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
        let Some((_, _, _, _)) = panes.iter().find(|(e, _, _, _)| *e == pane) else {
            gesture.state = GestureState::Idle;
            return;
        };
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
        commands_this_frame,
    } = &mut gesture.state
    {
        *commands_this_frame = 0;

        let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
            return;
        };

        let pointer_cell = match divider.axis {
            DividerAxis::Vertical => (cursor_phys.x / cell_w).floor() as i32,
            DividerAxis::Horizontal => (cursor_phys.y / cell_h).floor() as i32,
        };

        let target = resize_target_size(*near, pointer_cell);

        if target == *last_sent {
            return;
        }

        let current_size = panes
            .iter()
            .find(|(_, p, _, _)| p.id == divider.primary)
            .map(|(_, p, _, _)| match divider.axis {
                DividerAxis::Vertical => p.dims.width,
                DividerAxis::Horizontal => p.dims.height,
            });

        let Some(current_size) = current_size else {
            return;
        };

        if current_size != *last_sent {
            return;
        }

        if *commands_this_frame >= max_resize_per_frame {
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
        *commands_this_frame += 1;
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

    fn hdiv(primary: u32, pos: i32, s: i32, e: i32) -> Divider {
        Divider {
            axis: DividerAxis::Horizontal,
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
    fn click_count_increments_within_timeout_and_drift() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg), 1);
        assert_eq!(t.register(Duration::from_millis(200), Vec2::new(11.0, 11.0), cfg), 2);
        assert_eq!(t.register(Duration::from_millis(350), Vec2::new(12.0, 10.0), cfg), 3);
    }

    #[test]
    fn click_count_resets_after_timeout() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg), 1);
        assert_eq!(t.register(Duration::from_millis(500), Vec2::new(10.0, 10.0), cfg), 1);
    }

    #[test]
    fn click_count_resets_after_drift() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg), 1);
        assert_eq!(t.register(Duration::from_millis(100), Vec2::new(40.0, 40.0), cfg), 1);
    }

    #[test]
    fn position_commands_use_goto_line_for_row_and_relative_for_column() {
        let cmds = position_commands((2, 3), (5, 7), 100, 0);
        assert_eq!(
            cmds,
            vec![
                "send-keys -X goto-line 107".to_string(),
                "send-keys -X -N 3 cursor-right".to_string(),
            ]
        );
    }

    #[test]
    fn resize_target_size_follows_pointer() {
        assert_eq!(resize_target_size(0, 50), 50);
        assert_eq!(resize_target_size(10, 25), 15);
        assert_eq!(resize_target_size(0, 0), 1);
    }

    #[test]
    fn classify_press_divider_takes_precedence_over_pane() {
        let ds = [vdiv(1, 40, 0, 24)];
        let result = classify_press(
            &ds,
            Some(PaneId(2)),
            Vec2::new(322.0, 100.0),
            8.0,
            16.0,
            4.0,
        );
        assert_eq!(result, Press::Divider(vdiv(1, 40, 0, 24)));
    }

    #[test]
    fn classify_press_pane_when_no_divider_hit() {
        let ds = [vdiv(1, 40, 0, 24)];
        let result = classify_press(
            &ds,
            Some(PaneId(3)),
            Vec2::new(100.0, 100.0),
            8.0,
            16.0,
            4.0,
        );
        assert_eq!(result, Press::Pane(PaneId(3)));
    }

    #[test]
    fn classify_press_none_when_both_miss() {
        let result = classify_press(
            &[],
            Option::<PaneId>::None,
            Vec2::new(0.0, 0.0),
            8.0,
            16.0,
            4.0,
        );
        assert_eq!(result, Press::None);
    }

    #[test]
    fn classify_press_horizontal_divider() {
        let ds = [hdiv(5, 12, 0, 80)];
        let result = classify_press(
            &ds,
            Some(PaneId(7)),
            Vec2::new(200.0, 194.0),
            8.0,
            16.0,
            4.0,
        );
        assert_eq!(result, Press::Divider(hdiv(5, 12, 0, 80)));
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
}
