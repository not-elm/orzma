//! Mouse-wheel dispatch for every `OzmaTerminal` surface: scrollback and
//! app-forward wheel reporting with sub-notch accumulation and dominant-axis
//! lock. Hit-tests the cursor to a cell, drives the engine's pure `WheelAction`
//! router, and triggers the matched per-operation `EntityEvent` directly via
//! `apply_wheel_action`. Registered by `MouseWheelInputPlugin`; skips
//! `MouseDisabled` surfaces.

use super::{CellContext, TerminalSurfaces, hit_candidates};
use crate::input::InputPhase;
use crate::input::bindings::{FineModifier, OzmaMouseConfig};
use crate::input::gesture::{
    WheelAccumulator, accumulate_notches, lock_dominant_axis, wheel_delta_cells,
};
use crate::input::keyboard::current_terminal_modifiers;
use crate::webview_pointer::topmost_surface_at;
use bevy::input::mouse::{MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorMoved, PrimaryWindow};
use ozma_terminal::{TerminalMouseWrite, TerminalViewportScroll};
use ozma_tty_engine::{CellCoord, TermMode, TerminalModifiers, WheelAction, WheelModifiers};
use ozma_tty_renderer::TerminalCellMetricsResource;

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

/// A resolved wheel target for one frame: the surface entity, the cell under the
/// cursor, the cell height (for delta scaling), and the terminal modes. A plain
/// `Copy` value — the cell is resolved during `resolve_wheel_target`, so nothing
/// downstream needs the borrowed `CellContext`.
struct WheelTarget {
    target: Entity,
    cell: CellCoord,
    cell_h: f32,
    modes: TermMode,
}

/// The shared wheel dispatcher: resolves the topmost terminal under the cursor
/// (`resolve_wheel_target`), resets the accumulator on a target change,
/// accumulates notches (`accumulate_wheel`), and routes each non-zero axis to a
/// `WheelAction` applied as a per-operation `EntityEvent` (`apply_wheel`). Skips
/// `MouseDisabled` terminals; an empty candidate set drains the wheel events.
fn dispatch_mouse_wheel(
    mut commands: Commands,
    mut gesture_acc: ResMut<WheelAccumulator>,
    mut wheel: MessageReader<MouseWheel>,
    terminals: TerminalSurfaces,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(wt) = resolve_wheel_target(&terminals, &windows, &metrics) else {
        wheel.clear();
        return;
    };
    gesture_acc.retarget(wt.target);
    let (raw_v, raw_h) = accumulate_wheel(&mut gesture_acc, &mut wheel, wt.cell_h, &cfg);
    if raw_v == 0 && raw_h == 0 {
        return;
    }
    apply_wheel(
        &mut commands,
        wt.target,
        wt.cell,
        wt.modes,
        raw_v,
        raw_h,
        &keys,
        &cfg,
    );
}

/// Resolves the window/focus/empty-set guard + cursor → topmost surface for this
/// frame to a `WheelTarget`, or `None` on any miss (the caller drains the wheel
/// reader). The cell is resolved eagerly via a local `CellContext` (one cheap
/// projection even on a frame that accumulates no notch) so the result is a
/// lifetime-free value.
fn resolve_wheel_target(
    terminals: &TerminalSurfaces<'_, '_>,
    windows: &Query<&Window, With<PrimaryWindow>>,
    metrics: &TerminalCellMetricsResource,
) -> Option<WheelTarget> {
    let window = windows.single().ok()?;
    if !window.focused || terminals.is_empty() {
        return None;
    }
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_phys = window
        .cursor_position()
        .map(|c| c * window.scale_factor())?;
    let target = topmost_surface_at(cursor_phys, hit_candidates(terminals))?;
    let (_, handle, node, transform, grid) = terminals.get(target).ok()?;
    let ctx = CellContext {
        node,
        transform,
        grid,
        cell_w,
        cell_h,
    };
    let cell = ctx
        .hit(cursor_phys)
        .map(|(cell, _)| cell)
        .unwrap_or(CellCoord { col: 1, row: 1 });
    Some(WheelTarget {
        target,
        cell,
        cell_h,
        modes: handle.current_modes(),
    })
}

/// Folds this frame's wheel deltas, applies the dominant-axis lock, and
/// accumulates whole notches per axis. Returns `(raw_v, raw_h)`.
fn accumulate_wheel(
    gesture_acc: &mut WheelAccumulator,
    wheel: &mut MessageReader<MouseWheel>,
    cell_h: f32,
    cfg: &OzmaMouseConfig,
) -> (i32, i32) {
    let (delta_v, delta_h) = wheel.read().fold((0.0f32, 0.0f32), |(v, h), ev| {
        // NOTE: BOTH axes divide by cell_h (line height), not cell_w, so a given
        // finger distance yields the same notch rate horizontally and vertically.
        // Using the narrower cell_w (advance_phys, ~half of line_height_phys) made
        // horizontal ~2x too sensitive — do not "correct" ev.x to cell_w.
        (
            v + wheel_delta_cells(ev.unit, ev.y, cell_h),
            h + wheel_delta_cells(ev.unit, ev.x, cell_h),
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
    (raw_v, raw_h)
}

/// Routes each non-zero axis to a `WheelAction` (vertical negated to the engine
/// convention; horizontal macOS sign flip) and applies it via `apply_wheel_action`.
fn apply_wheel(
    commands: &mut Commands,
    target: Entity,
    cell: CellCoord,
    modes: TermMode,
    raw_v: i32,
    raw_h: i32,
    keys: &ButtonInput<KeyCode>,
    cfg: &OzmaMouseConfig,
) {
    if raw_v != 0 {
        // NOTE: Bevy +y (up/older) → engine convention (negative = up/older).
        let mods = build_wheel_modifiers(keys, cfg);
        apply_wheel_action(
            commands,
            target,
            WheelAction::route(modes, -raw_v, cell, mods, &cfg.wheel),
        );
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
        let mods = build_wheel_modifiers_horizontal(keys, cfg);
        apply_wheel_action(
            commands,
            target,
            WheelAction::route_horizontal(modes, signed_h, cell, mods, &cfg.wheel),
        );
    }
}

/// Applies one routed `WheelAction` as the matching per-operation `EntityEvent`.
fn apply_wheel_action(commands: &mut Commands, entity: Entity, action: WheelAction) {
    match action {
        WheelAction::WriteToPty(bytes) => {
            commands.trigger(TerminalMouseWrite { entity, bytes });
        }
        WheelAction::ScrollViewport(lines) => {
            commands.trigger(TerminalViewportScroll { entity, lines });
        }
        WheelAction::Noop => {}
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
    use crate::input::mouse::MouseEffect;
    use crate::input::mouse::test_support::{
        CapturedEffects, add_effect_capture_observers, set_phys_cursor, test_metrics,
    };
    use bevy::input::mouse::MouseScrollUnit;
    use bevy::ui::{ComputedNode, UiGlobalTransform};
    use ozma_terminal::{Clipboard, OzmaTerminal};
    use ozma_tty_engine::TerminalHandle;
    use ozma_tty_renderer::schema::TerminalGrid;

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
        // test guards that both axes are emitted from one dispatch run, not the lock.
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
