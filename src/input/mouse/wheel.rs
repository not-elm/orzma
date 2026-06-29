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
use bevy::input::mouse::{MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::{
    CellCoord, TermMode, TerminalHandle, TerminalModifiers, WheelAction, WheelConfig,
    WheelModifiers,
};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::schema::TerminalGrid;

/// Registers the mouse-wheel dispatcher and its accumulator resource. Runs in
/// `InputPhase::Dispatch`, gated to frames carrying any mouse message — a
/// cursor-only frame must still run `WheelAccumulator::retarget` so a pane's
/// sub-notch residual is cleared when the cursor moves to another terminal.
pub(super) struct MouseWheelInputPlugin;

impl Plugin for MouseWheelInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WheelAccumulator>().add_systems(
            Update,
            dispatch_mouse_wheel.in_set(InputPhase::Dispatch).run_if(
                on_message::<MouseButtonInput>
                    .or(on_message::<CursorMoved>)
                    .or(on_message::<MouseWheel>),
            ),
        );
    }
}

/// The shared wheel dispatcher: routes to the topmost terminal under the cursor,
/// resets the accumulator on a target change, accumulates notches, drives
/// `decide_wheel`, and fans the decided effects out to per-operation
/// `EntityEvent`s via `trigger_mouse_effects`. Skips `MouseDisabled`
/// terminals; an empty candidate set drains the wheel events.
fn dispatch_mouse_wheel(
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
fn decide_wheel(
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
fn effects_from_wheel_action(action: WheelAction) -> Vec<MouseEffect> {
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
fn build_wheel_modifiers_horizontal(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::mouse::test_support::{
        CapturedEffects, add_effect_capture_observers, set_phys_cursor, test_metrics,
    };
    use bevy::input::mouse::MouseScrollUnit;
    use ozma_terminal::Clipboard;

    fn make_wheel_app(enable_modes: &[u8]) -> App {
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseWheel>()
            .init_resource::<OzmaMouseConfig>()
            .init_resource::<WheelAccumulator>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Clipboard>()
            .init_resource::<CapturedEffects>()
            .insert_resource(test_metrics())
            .add_systems(Update, dispatch_mouse_wheel);
        add_effect_capture_observers(&mut app);

        let mut handle = TerminalHandle::detached(100, 37);
        handle.advance(enable_modes);
        app.world_mut().spawn((
            OzmaTerminal,
            handle,
            ComputedNode {
                size: Vec2::new(800.0, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(400.0, 300.0),
            TerminalGrid {
                cols: 100,
                rows: 37,
                ..default()
            },
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    fn write_wheel(app: &mut App, x: f32, y: f32) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: MouseScrollUnit::Line,
                x,
                y,
                window: Entity::PLACEHOLDER,
            });
    }

    /// Sign of `MouseWheel.x` for a physical-right trackpad scroll on this
    /// platform: negative on macOS (winit's PixelDelta is opposite X11/Wayland),
    /// positive elsewhere. Lets the direction tests assert the same SGR button
    /// on every target instead of being cfg-gated to macOS.
    fn phys_right_sign() -> f32 {
        if cfg!(target_os = "macos") { -1.0 } else { 1.0 }
    }

    fn disable_axis_lock(app: &mut App) {
        app.insert_resource(OzmaMouseConfig {
            axis_lock_ratio: 0.0,
            ..default()
        });
    }

    #[test]
    fn scrollback_up_returns_positive_viewport_scroll() {
        // Bevy +y (wheel up) → caller negates → engine notches negative → into history.
        let fx = decide_wheel(
            TermMode::empty(),
            -1,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &WheelConfig::default(),
        );
        assert_eq!(fx, vec![MouseEffect::Scroll(3)]);
    }

    #[test]
    fn app_capture_wheel_forwards_bytes() {
        let modes = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let fx = decide_wheel(
            modes,
            -1,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &WheelConfig::default(),
        );
        assert!(matches!(fx.as_slice(), [MouseEffect::Write(b)] if !b.is_empty()));
    }

    #[test]
    fn effects_from_wheel_action_maps_each_variant() {
        assert_eq!(effects_from_wheel_action(WheelAction::Noop), vec![]);
        assert_eq!(
            effects_from_wheel_action(WheelAction::WriteToPty(b"x".to_vec())),
            vec![MouseEffect::Write(b"x".to_vec())]
        );
        assert_eq!(
            effects_from_wheel_action(WheelAction::ScrollViewport(3)),
            vec![MouseEffect::Scroll(3)]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn horizontal_modifiers_strip_shift_on_macos() {
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ShiftLeft);
        let cfg = OzmaMouseConfig::default();
        let mods = build_wheel_modifiers_horizontal(&keys, &cfg);
        assert!(
            !mods.shift,
            "macOS converts Shift+wheel to horizontal at the OS level; the report must not carry the Shift bit"
        );
    }

    #[test]
    fn dispatch_pure_horizontal_right_emits_sgr_67() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5 * phys_right_sign(), 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))),
            "a physical-right wheel in mouse mode must emit an SGR wheel-right (cb 67) report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_horizontal_left_emits_sgr_66() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, -0.5 * phys_right_sign(), 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<66;"))),
            "a physical-left wheel in mouse mode must emit an SGR wheel-left (cb 66) report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_diagonal_emits_both_axes() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        // Disable the dominant-axis lock so a diagonal keeps both axes; this
        // test guards the batching (both axes in ONE trigger), not the lock.
        disable_axis_lock(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5 * phys_right_sign(), -0.5);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<65;"))),
            "vertical (down, cb 65) report missing: {:?}",
            cap.0
        );
        assert!(
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(b"\x1b[<67;"))),
            "horizontal (right, cb 67) report missing: {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_axis_lock_drops_jitter_during_vertical_scroll() {
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        // Vertical-dominant swipe whose horizontal component (0.6 cells) is on
        // its own past cells_per_notch (0.5) and WOULD emit a notch unlocked;
        // |x|/hypot = 0.29 < 0.9, so the default lock must drop it (no cb 66/67).
        // A smaller jitter would not discriminate — it makes no notch either way.
        write_wheel(&mut app, 0.6, -2.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        let has = |needle: &[u8]| {
            cap.0
                .iter()
                .any(|e| matches!(e, MouseEffect::Write(b) if b.starts_with(needle)))
        };
        assert!(has(b"\x1b[<65;"), "vertical (down, cb 65) report missing");
        assert!(
            !has(b"\x1b[<66;") && !has(b"\x1b[<67;"),
            "off-axis jitter must NOT emit a horizontal report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn dispatch_horizontal_without_mouse_mode_emits_no_report() {
        let mut app = make_wheel_app(b"");
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5, 0.0);
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        assert!(
            cap.0.iter().all(|e| !matches!(e, MouseEffect::Write(_))),
            "horizontal wheel outside a mouse mode must not emit a report, got {:?}",
            cap.0
        );
    }

    #[test]
    fn pixel_horizontal_sensitivity_matches_vertical() {
        // Equal Pixel-unit deltas on both axes must emit equal report counts.
        // Regression: horizontal divided ev.x by cell_w (~half of cell_h), so a
        // given finger distance fired ~2x the notches and scrolled too far.
        let mut app = make_wheel_app(b"\x1b[?1000;1006h");
        // Disable the dominant-axis lock; this test compares per-axis
        // sensitivity, which needs both axes to survive an equal-delta gesture.
        disable_axis_lock(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: MouseScrollUnit::Pixel,
                x: 16.0 * phys_right_sign(),
                y: 16.0,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        let cap = app.world().resource::<CapturedEffects>();
        let count = |needle: &[u8]| -> usize {
            cap.0
                .iter()
                .filter_map(|e| match e {
                    MouseEffect::Write(b) => Some(b),
                    _ => None,
                })
                .map(|b| b.windows(needle.len()).filter(|w| *w == needle).count())
                .sum()
        };
        // test_metrics: cell_w = 8, cell_h = 16. y=16 → up reports (cb 64);
        // a physical-right x → right reports (cb 67).
        let vertical = count(b"\x1b[<64;");
        let horizontal = count(b"\x1b[<67;");
        assert!(
            vertical > 0 && horizontal > 0,
            "both axes must emit reports, got v={vertical} h={horizontal}"
        );
        assert_eq!(
            horizontal, vertical,
            "equal Pixel deltas must emit equal report counts (horizontal sensitivity \
             must match vertical), got v={vertical} h={horizontal}"
        );
    }
}
