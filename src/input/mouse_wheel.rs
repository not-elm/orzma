//! Bevy system that translates mouse-wheel input into either host
//! scrollback adjustments or PTY-bound mouse / arrow-key bytes via the
//! pure router in `bevy_terminal::wheel`.
//!
//! Per-frame flow:
//!
//! 1. Read `MessageReader<MouseWheel>`. For `Line` units, accumulate
//!    `y` into `residual_y`. For `Pixel` units, divide by the cell
//!    height and accumulate.
//! 2. Reset the residual when the sign flips or the focused entity
//!    changes — both signals indicate the previous accumulation is
//!    stale.
//! 3. Truncate the residual to an integer `notches` count; the
//!    fractional remainder carries to the next frame.
//! 4. Resolve the active session's focused pane → entity (mirrors
//!    `dispatch_focused_key`). If copy mode is active, skip — the copy
//!    mode handler owns input semantics there.
//! 5. Resolve the cursor cell within the focused pane (or fall back
//!    to `(1, 1)`).
//! 6. Build `WheelModifiers` from `ButtonInput<KeyCode>` using the
//!    config's `fine_modifier` to set `mods.fine`.
//! 7. Call `bevy_terminal::route_wheel` once; dispatch the returned
//!    `WheelAction` to the focused entity's `TerminalHandle`.

use bevy::input::ButtonInput;
use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_terminal::{
    CellCoord, Coalescer, PtyHandle, TerminalHandle, WheelAction, WheelConfig, WheelModifiers,
    route_wheel,
};
use ozmux_configs::mouse::FineModifier;

use crate::ui::terminal::{CELL_H_LOGICAL_PX, CELL_W_LOGICAL_PX};

/// Per-frame accumulator that carries fractional Pixel deltas across
/// frames and tracks the entity the residual was earned on (so a focus
/// change clears stale momentum).
#[derive(Resource, Default)]
pub(crate) struct WheelAccumulator {
    residual_y: f32,
    last_entity: Option<Entity>,
}

/// Bevy Plugin that registers `WheelAccumulator` and the
/// `mouse_wheel_system` against the `Update` schedule.
pub(crate) struct MouseWheelInputPlugin;

impl Plugin for MouseWheelInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WheelAccumulator>()
            .add_systems(Update, mouse_wheel_system);
    }
}

#[allow(clippy::too_many_arguments)]
fn mouse_wheel_system(
    mut wheel_msgs: MessageReader<MouseWheel>,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<crate::configs::OzmuxConfigsResource>,
    registry: Res<crate::ui::registry::ActivityEntityRegistry>,
    mux: Res<crate::multiplexer::Multiplexer>,
    copy_mode_q: Query<(), With<crate::ui::copy_mode::CopyModeState>>,
    mut accumulator: ResMut<WheelAccumulator>,
    mut handles: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer)>,
    sessions: Query<&crate::multiplexer::AttachedSession>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let mut delta_y = 0.0f32;
    let mut had_input = false;
    for ev in wheel_msgs.read() {
        had_input = true;
        let cells = match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y / CELL_H_LOGICAL_PX,
        };
        delta_y += cells;
    }
    if !had_input {
        return;
    }

    let Some(attached) = sessions.iter().next() else {
        return;
    };
    let Ok((wid, pid)) = mux.active_pane_of_session(&attached.0) else {
        return;
    };
    let Some(window_state) = mux.windows.get(&wid) else {
        return;
    };
    let Ok(pane) = window_state.pane(&pid) else {
        return;
    };
    let Some(entity) = registry.get(&pane.active_activity) else {
        return;
    };

    if copy_mode_q.get(entity).is_ok() {
        return;
    }

    if accumulator.last_entity != Some(entity) {
        accumulator.residual_y = 0.0;
        accumulator.last_entity = Some(entity);
    } else if accumulator.residual_y.signum() != delta_y.signum() && accumulator.residual_y != 0.0
    {
        accumulator.residual_y = 0.0;
    }

    accumulator.residual_y += delta_y;
    let notches = accumulator.residual_y.trunc() as i32;
    if notches == 0 {
        return;
    }
    accumulator.residual_y -= notches as f32;

    let mouse_cfg = &configs.mouse;
    let fine_pressed = match mouse_cfg.fine_modifier {
        FineModifier::Shift => {
            keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight)
        }
        FineModifier::Ctrl => {
            keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight)
        }
        FineModifier::Alt => keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        FineModifier::None => false,
    };
    let mods = WheelModifiers {
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        fine: fine_pressed,
    };

    let cursor_cell = windows
        .iter()
        .next()
        .and_then(|w| w.cursor_position())
        .map(|pos| CellCoord {
            col: ((pos.x / CELL_W_LOGICAL_PX) as u32).saturating_add(1).max(1),
            row: ((pos.y / CELL_H_LOGICAL_PX) as u32).saturating_add(1).max(1),
        })
        .unwrap_or(CellCoord { col: 1, row: 1 });

    let wheel_cfg = WheelConfig {
        lines_per_notch: mouse_cfg.lines_per_notch,
        fine_lines: mouse_cfg.fine_lines,
        max_protocol_events_per_frame: mouse_cfg.max_protocol_events_per_frame,
    };

    let Ok((mut handle, mut pty, mut coalescer)) = handles.get_mut(entity) else {
        return;
    };

    let action = route_wheel(handle.current_modes(), notches, cursor_cell, mods, &wheel_cfg);

    match action {
        WheelAction::Noop => {}
        WheelAction::ScrollViewport(delta) => {
            handle.scroll(&mut coalescer, delta);
        }
        WheelAction::WriteToPty(bytes) => {
            if !handle.is_at_bottom() {
                handle.scroll_to_bottom(&mut coalescer);
            }
            if let Err(e) = handle.write(&mut pty, &bytes) {
                tracing::warn!(?e, ?entity, "mouse wheel write failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_accumulator_default_is_zero() {
        let acc = WheelAccumulator::default();
        assert_eq!(acc.residual_y, 0.0);
        assert!(acc.last_entity.is_none());
    }
}
