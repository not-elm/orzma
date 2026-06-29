//! Mouse-wheel dispatch for every `OzmaTerminal` surface: scrollback and
//! app-forward wheel reporting with sub-notch accumulation and dominant-axis
//! lock. Hit-tests the cursor to a cell, drives the engine's pure `WheelAction`
//! router, and fans the decided effects out via the shared
//! `trigger_mouse_effects`. Registered by `MouseWheelInputPlugin`; skips
//! `MouseDisabled` surfaces.

use super::{CellContext, MouseEffect, trigger_mouse_effects};
use crate::input::InputPhase;
use crate::input::bindings::{FineModifier, OzmaMouseConfig};
use crate::input::focus::MouseDisabled;
use crate::input::gesture::{
    WheelAccumulator, accumulate_notches, lock_dominant_axis, wheel_delta_cells,
};
use crate::input::keyboard::current_terminal_modifiers;
use crate::webview_pointer::topmost_surface_at;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::{
    CellCoord, TermMode, TerminalHandle, TerminalModifiers, WheelAction, WheelConfig,
    WheelModifiers,
};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::schema::TerminalGrid;

/// Registers the mouse-wheel dispatcher and its accumulator resource. Runs in
/// `InputPhase::Dispatch`, gated to frames carrying a wheel message.
pub(super) struct MouseWheelInputPlugin;

impl Plugin for MouseWheelInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WheelAccumulator>().add_systems(
            Update,
            dispatch_mouse_wheel
                .in_set(InputPhase::Dispatch)
                .run_if(on_message::<MouseWheel>),
        );
    }
}

/// The shared wheel dispatcher: routes to the topmost terminal under the cursor,
/// resets the accumulator on a target change, accumulates notches, drives
/// `decide_wheel`, and fans the decided effects out to per-operation
/// `EntityEvent`s via `trigger_mouse_effects`. Skips `MouseDisabled`
/// terminals; an empty candidate set drains the wheel events.
pub(super) fn dispatch_mouse_wheel(
    mut commands: Commands,
    mut gesture_acc: ResMut<WheelAccumulator>,
    mut wheel: MessageReader<MouseWheel>,
    terminals: Query<
        (
            Entity,
            &TerminalHandle,
            &ComputedNode,
            &UiGlobalTransform,
            &TerminalGrid,
        ),
        (With<OzmaTerminal>, Without<MouseDisabled>),
    >,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        wheel.clear();
        return;
    };
    if !window.focused || terminals.is_empty() {
        wheel.clear();
        return;
    }
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some(cursor_phys) = window.cursor_position().map(|c| c * window.scale_factor()) else {
        wheel.clear();
        return;
    };
    let Some(target) = topmost_surface_at(
        cursor_phys,
        terminals
            .iter()
            .map(|(e, _, node, transform, _)| (e, node, transform)),
    ) else {
        wheel.clear();
        return;
    };
    let Ok((_, handle, node, transform, grid)) = terminals.get(target) else {
        wheel.clear();
        return;
    };
    let ctx = CellContext {
        node,
        transform,
        grid,
        cell_w,
        cell_h,
    };

    gesture_acc.retarget(target);
    let (delta_v, delta_h) = wheel.read().fold((0.0f32, 0.0f32), |(v, h), ev| {
        // NOTE: BOTH axes divide by cell_h (line height), not cell_w, so a given
        // finger distance yields the same notch rate horizontally and vertically.
        // Using the narrower cell_w (advance_phys, ~half of line_height_phys) made
        // horizontal ~2x too sensitive — do not "correct" ev.x to ctx.cell_w.
        (
            v + wheel_delta_cells(ev.unit, ev.y, ctx.cell_h),
            h + wheel_delta_cells(ev.unit, ev.x, ctx.cell_h),
        )
    });
    // NOTE: do NOT also clear the suppressed axis's residual here. The lock
    // zeros the off-axis delta before accumulation, so it adds 0 and cannot leak
    // a notch; clearing would instead wipe genuine sub-notch progress on a
    // deliberate horizontal swipe whose slow frames dip below the lock ratio.
    let (delta_v, delta_h) = lock_dominant_axis(delta_v, delta_h, cfg.axis_lock_ratio);
    let raw_v = accumulate_notches(
        &mut gesture_acc.residual_cells,
        delta_v,
        cfg.cells_per_notch,
    );
    let raw_h = accumulate_notches(
        &mut gesture_acc.residual_cells_h,
        delta_h,
        cfg.cells_per_notch,
    );
    if raw_v == 0 && raw_h == 0 {
        return;
    }
    let cell = ctx
        .hit(cursor_phys)
        .map(|(cell, _)| cell)
        .unwrap_or(CellCoord { col: 1, row: 1 });
    let modes = handle.current_modes();
    let mut effects = Vec::new();
    if raw_v != 0 {
        // NOTE: Bevy +y (up/older) → engine convention (negative = up/older).
        let mods = build_wheel_modifiers(&keys, &cfg);
        effects.extend(decide_wheel(modes, -raw_v, cell, mods, &cfg.wheel));
    }
    if raw_h != 0 {
        // NOTE: macOS/winit reports a physical-right trackpad scroll as a
        // negative MouseWheel.x (opposite X11/Wayland), so negate ONLY on
        // macOS to map physical-right → Right (cb 67). Other platforms already
        // match the engine's positive=right convention; gating mirrors the
        // macOS-only handling in `build_wheel_modifiers_horizontal`.
        let signed_h = if cfg!(target_os = "macos") {
            -raw_h
        } else {
            raw_h
        };
        let mods = build_wheel_modifiers_horizontal(&keys, &cfg);
        effects.extend(effects_from_wheel_action(WheelAction::route_horizontal(
            modes, signed_h, cell, mods, &cfg.wheel,
        )));
    }
    trigger_mouse_effects(&mut commands, target, effects);
}

/// Pure wheel decision. `notches` is in the engine convention (negative =
/// up/older); callers negate the Bevy-derived up-positive value before calling.
pub(super) fn decide_wheel(
    modes: TermMode,
    notches: i32,
    cell: CellCoord,
    mods: WheelModifiers,
    cfg: &WheelConfig,
) -> Vec<MouseEffect> {
    effects_from_wheel_action(WheelAction::route(modes, notches, cell, mods, cfg))
}

/// Maps a routed `WheelAction` to host effects. Shared by the vertical
/// (`route`) and horizontal (`route_horizontal`) wheel paths.
pub(super) fn effects_from_wheel_action(action: WheelAction) -> Vec<MouseEffect> {
    match action {
        WheelAction::Noop => Vec::new(),
        WheelAction::WriteToPty(b) => vec![MouseEffect::Write(b)],
        WheelAction::ScrollViewport(lines) => vec![MouseEffect::Scroll(lines)],
    }
}

/// Horizontal-wheel modifiers. On macOS the OS converts Shift+wheel into a
/// horizontal scroll while Shift stays physically held; stripping the Shift bit
/// keeps the report a plain `<ScrollWheelLeft/Right>` rather than the shifted
/// (and by default unmapped) variant. Other platforms pass modifiers through.
pub(super) fn build_wheel_modifiers_horizontal(
    keys: &ButtonInput<KeyCode>,
    cfg: &OzmaMouseConfig,
) -> WheelModifiers {
    let mut mods = build_wheel_modifiers(keys, cfg);
    if cfg!(target_os = "macos") {
        mods.shift = false;
    }
    mods
}

/// Builds `WheelModifiers` from the held keys + the fine-scroll config.
fn build_wheel_modifiers(keys: &ButtonInput<KeyCode>, cfg: &OzmaMouseConfig) -> WheelModifiers {
    let m = current_terminal_modifiers(keys);
    WheelModifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        fine: fine_held(cfg.fine_modifier, &m),
    }
}

fn fine_held(modifier: FineModifier, m: &TerminalModifiers) -> bool {
    match modifier {
        FineModifier::Shift => m.shift,
        FineModifier::Ctrl => m.ctrl,
        FineModifier::Alt => m.alt,
        FineModifier::None => true,
    }
}
