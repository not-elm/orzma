//! Bevy system that translates mouse-wheel input into either host
//! scrollback adjustments or PTY-bound mouse / arrow-key bytes via the
//! pure router in `bevy_terminal::wheel::WheelAction::route`.
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
//! 7. Call `WheelAction::route` once; dispatch the returned
//!    `WheelAction` to the focused entity's `TerminalHandle`.

use bevy::input::ButtonInput;
use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_terminal::{
    CellCoord, Coalescer, PtyHandle, TerminalHandle, WheelAction, WheelConfig, WheelModifiers,
};
use bevy_terminal_renderer::TerminalCellMetricsResource;
use ozmux_configs::mouse::FineModifier;

/// Per-frame accumulator that carries fractional Pixel deltas across
/// frames and tracks the entity the residual was earned on (so a focus
/// change clears stale momentum).
#[derive(Resource, Default)]
pub(crate) struct WheelAccumulator {
    residual_y: f32,
    last_entity: Option<Entity>,
}

/// Bevy Plugin that registers `WheelAccumulator` and the
/// `dispatch_mouse_wheel` system against the `Update` schedule.
pub(crate) struct MouseWheelInputPlugin;

impl Plugin for MouseWheelInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WheelAccumulator>().add_systems(
            Update,
            dispatch_mouse_wheel.in_set(crate::system_set::OzmuxSystems::Input),
        );
    }
}

fn dispatch_mouse_wheel(
    mut wheel_msgs: MessageReader<MouseWheel>,
    mut accumulator: ResMut<WheelAccumulator>,
    mut handles: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer)>,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<crate::configs::OzmuxConfigsResource>,
    registry: Res<crate::ui::registry::ActivityEntityRegistry>,
    mux: ozmux_multiplexer::MultiplexerCommands,
    copy_mode_q: Query<(), With<crate::ui::copy_mode::CopyModeState>>,
    attached_q: Query<
        bevy::prelude::Entity,
        (
            With<ozmux_multiplexer::SessionMarker>,
            With<ozmux_multiplexer::AttachedSession>,
        ),
    >,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
) {
    // Cell pitch comes from the font-derived TerminalCellMetricsResource
    // (physical px, DPR-adjusted by `update_terminal_material`). MouseWheel
    // `Pixel` events and `cursor_position()` both report logical px, so we
    // divide the phys metrics by DPR to compare apples-to-apples here.
    let dpr = windows
        .iter()
        .next()
        .map(|w| w.scale_factor())
        .unwrap_or(1.0);
    let cell_w_logical = (metrics.metrics.advance_phys.floor() / dpr).max(1.0);
    let cell_h_logical = (metrics.metrics.line_height_phys.floor() / dpr).max(1.0);

    let Some(delta_y) = aggregate_wheel_delta(&mut wheel_msgs, cell_h_logical) else {
        return;
    };
    let Some(entity) = super::resolve_focused_terminal(&mux, &attached_q, &registry) else {
        return;
    };
    if copy_mode_q.get(entity).is_ok() {
        return;
    }
    let mouse_cfg = &configs.mouse;
    let Some(notches) =
        consume_notches(&mut accumulator, entity, delta_y, mouse_cfg.cells_per_notch)
    else {
        return;
    };
    let mods = build_wheel_modifiers(&keys, mouse_cfg.fine_modifier);
    let cursor = cursor_cell(&windows, cell_w_logical, cell_h_logical);
    let Ok((mut handle, mut pty, mut coalescer)) = handles.get_mut(entity) else {
        return;
    };
    let action = WheelAction::route(
        handle.current_modes(),
        notches,
        cursor,
        mods,
        &wheel_config(mouse_cfg),
    );
    apply_wheel_action(action, &mut handle, &mut pty, &mut coalescer, entity);
}

/// Aggregates a frame's `MouseWheel` events into a single signed
/// cell-delta. Returns `None` when no events arrived.
///
/// winit reports positive `y` when natural scrolling moves the
/// viewport content downward (revealing older lines above); our
/// router uses the opposite convention (`notches < 0` = up / older),
/// so the sign is flipped here.
fn aggregate_wheel_delta(
    events: &mut MessageReader<MouseWheel>,
    cell_h_logical: f32,
) -> Option<f32> {
    let mut delta_y = 0.0f32;
    let mut had_input = false;
    for ev in events.read() {
        had_input = true;
        let cells = match ev.unit {
            MouseScrollUnit::Line => -ev.y,
            MouseScrollUnit::Pixel => -ev.y / cell_h_logical,
        };
        delta_y += cells;
    }
    had_input.then_some(delta_y)
}

/// Updates the per-frame accumulator and returns the integer notch
/// count to dispatch, or `None` when the residual hasn't crossed the
/// notch threshold yet.
///
/// Resets the residual on focus change or sign flip — both signal
/// that any prior momentum is stale.
fn consume_notches(
    accumulator: &mut WheelAccumulator,
    entity: Entity,
    delta_y: f32,
    cells_per_notch: f32,
) -> Option<i32> {
    if accumulator.last_entity != Some(entity) {
        accumulator.residual_y = 0.0;
        accumulator.last_entity = Some(entity);
    } else if accumulator.residual_y.signum() != delta_y.signum() && accumulator.residual_y != 0.0 {
        accumulator.residual_y = 0.0;
    }
    let threshold = cells_per_notch.max(f32::EPSILON);
    accumulator.residual_y += delta_y;
    let notches = (accumulator.residual_y / threshold).trunc() as i32;
    if notches == 0 {
        return None;
    }
    accumulator.residual_y -= notches as f32 * threshold;
    Some(notches)
}

/// Captures the current keyboard modifier state, resolving
/// `mods.fine` against the configured `fine_modifier`.
fn build_wheel_modifiers(
    keys: &ButtonInput<KeyCode>,
    fine_modifier: FineModifier,
) -> WheelModifiers {
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let fine = match fine_modifier {
        FineModifier::Shift => shift,
        FineModifier::Ctrl => ctrl,
        FineModifier::Alt => alt,
        FineModifier::None => false,
    };
    WheelModifiers {
        shift,
        ctrl,
        alt,
        fine,
    }
}

/// Translates the window cursor position into a 1-indexed cell
/// coordinate. Falls back to `(1, 1)` when no cursor position is
/// available (cursor outside the window, no primary window matched).
fn cursor_cell(
    windows: &Query<&Window, With<PrimaryWindow>>,
    cell_w_logical: f32,
    cell_h_logical: f32,
) -> CellCoord {
    windows
        .iter()
        .next()
        .and_then(|w| w.cursor_position())
        .map(|pos| CellCoord {
            col: ((pos.x / cell_w_logical) as u32).saturating_add(1).max(1),
            row: ((pos.y / cell_h_logical) as u32).saturating_add(1).max(1),
        })
        .unwrap_or(CellCoord { col: 1, row: 1 })
}

/// Projects the runtime `MouseConfig` onto the router's `WheelConfig`
/// (the per-call subset the pure router needs).
fn wheel_config(cfg: &ozmux_configs::mouse::MouseConfig) -> WheelConfig {
    WheelConfig {
        lines_per_notch: cfg.lines_per_notch,
        fine_lines: cfg.fine_lines,
        max_protocol_events_per_frame: cfg.max_protocol_events_per_frame,
    }
}

/// Applies a router-decided `WheelAction` to the focused terminal —
/// either scrolls the viewport or writes pre-encoded bytes to the
/// PTY (snapping to live tail first for the write path).
fn apply_wheel_action(
    action: WheelAction,
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
    entity: Entity,
) {
    match action {
        WheelAction::Noop => {}
        WheelAction::ScrollViewport(delta) => {
            handle.scroll(coalescer, delta);
        }
        WheelAction::WriteToPty(bytes) => {
            if !handle.is_at_bottom() {
                handle.scroll_to_bottom(coalescer);
            }
            if let Err(e) = handle.write(pty, &bytes) {
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
