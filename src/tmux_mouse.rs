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

use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use bevy::input::ButtonState;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozmux_tmux::{
    ActiveWindow, PaneId, TmuxConnection, TmuxDividers, TmuxPane, resize_pane_x_command,
    resize_pane_y_command, select_pane_command,
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

/// The current phase of a left-button gesture over a tmux pane.
#[derive(Default, Debug, PartialEq)]
enum GestureState {
    /// No button is held; the arbiter is waiting for the next press.
    #[default]
    Idle,
    /// Left button is held; `pane_id` is the pane that received the press and
    /// `origin_phys` is the physical-pixel cursor position at press time.
    Pressed {
        pane_id: PaneId,
        origin_phys: Vec2,
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
}

/// What a left-press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    Divider(Divider),
    Pane(PaneId),
    None,
}

/// Returns the `PaneId` of the first `TmuxPane` whose `ComputedNode` contains
/// `cursor_phys` (physical px), or `None` when no pane covers the point.
fn pane_under_cursor(
    panes: &Query<(&TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    cursor_phys: Vec2,
) -> Option<PaneId> {
    panes
        .iter()
        .find(|(_, node, transform)| node.contains_point(**transform, cursor_phys))
        .map(|(pane, _, _)| pane.id)
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
fn position_commands(cur: (u16, u16), target: (u16, u16), history: u32, scroll: u32) -> Vec<String> {
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

/// Interprets raw left-button messages into tmux `select-pane` or
/// `resize-pane` commands.
///
/// On each `Pressed` event the cursor's physical position is classified via
/// `classify_press`: a divider hit enters `Resizing` state; a pane hit sends
/// `select-pane` and enters `Pressed`; a miss leaves the state `Idle`. Each
/// frame while `Resizing` the pointer's major-axis cell coordinate is mapped to
/// an absolute target size and sent as `resize-pane -x/-y` when the target
/// differs from the last-sent size and the prior resize has been confirmed
/// (one-in-flight throttle). On `Released` the state returns to `Idle`. When
/// the primary window is not focused any queued events are drained and the
/// state is reset.
fn arbiter(
    mut gesture: ResMut<TmuxMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    connection: NonSend<TmuxConnection>,
    panes: Query<(&TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    dividers_q: Query<&TmuxDividers, With<ActiveWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    configs: Option<Res<OzmuxConfigsResource>>,
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

    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);

    let (grab_tol_logical, max_resize_per_frame) = configs
        .as_deref()
        .map(|c| {
            (
                c.mouse.divider_grab_tolerance_px,
                c.mouse.max_resize_commands_per_frame,
            )
        })
        .unwrap_or((4.0, 4));
    let tol_phys = grab_tol_logical * scale;

    let dividers: &[Divider] = dividers_q
        .single()
        .map(|d| d.0.as_slice())
        .unwrap_or(&[]);

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
                match classify_press(dividers, pane_under, cursor_phys, cell_w, cell_h, tol_phys) {
                    Press::Divider(d) => {
                        let (near, last_sent) = panes
                            .iter()
                            .find(|(p, _, _)| p.id == d.primary)
                            .map(|(p, _, _)| match d.axis {
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
                        gesture.state = GestureState::Pressed {
                            pane_id,
                            origin_phys: cursor_phys,
                        };
                    }
                    Press::None => {}
                }
            }
            ButtonState::Released => {
                gesture.state = GestureState::Idle;
            }
        }
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
            .find(|(p, _, _)| p.id == divider.primary)
            .map(|(p, _, _)| match divider.axis {
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
        let result = classify_press(&[], Option::<PaneId>::None, Vec2::new(0.0, 0.0), 8.0, 16.0, 4.0);
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
        app.insert_resource(test_metrics());
        app.add_systems(Update, arbiter);
        app.world_mut()
            .spawn((Window { focused: true, ..default() }, PrimaryWindow));
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: bevy::input::mouse::MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        assert_eq!(app.world().resource::<TmuxMouseGesture>().state, GestureState::Idle);
    }
}
