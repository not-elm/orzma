//! Mouse-wheel dispatch for every `OrzmaTerminal` surface: scrollback with
//! sub-notch accumulation and dominant-axis lock. Vertical wheel motion
//! always scrolls the terminal's viewport (`TerminalViewportScroll`);
//! horizontal wheel motion is ignored — app-forward wheel reporting is
//! out of scope until mouse routing is reintroduced against `orzma_tty`
//! (D17 of the engine-swap design). Registered by `MouseWheelInputPlugin`;
//! skips `MouseDisabled` surfaces.

use super::{TerminalSurfaces, cell_dims, hit_candidates, on_any_mouse_message};
use crate::action::terminal::TerminalViewportScroll;
use crate::input::InputPhase;
use crate::input::bindings::{FineModifier, OrzmaMouseConfig, WheelConfig};
use crate::input::keyboard::current_terminal_modifiers;
use crate::input::mouse::gesture::{
    WheelAccumulator, accumulate_notches, lock_dominant_axis, wheel_delta_cells,
};
use crate::surface::geometry::topmost_surface_at;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use orzma_tty::prelude::TerminalModifiers;
use orzma_tty_renderer::TerminalCellMetricsResource;

/// Registers the mouse-wheel dispatcher and its accumulator resource. Runs in
/// `InputPhase::Dispatch`, gated to frames carrying any mouse message — a
/// cursor-only frame must still run `WheelAccumulator::retarget` so a
/// terminal's sub-notch residual is cleared when the cursor moves to another
/// terminal.
pub(super) struct MouseWheelInputPlugin;

impl Plugin for MouseWheelInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WheelAccumulator>().add_systems(
            Update,
            dispatch_mouse_wheel
                .in_set(InputPhase::Dispatch)
                .run_if(on_any_mouse_message()),
        );
    }
}

/// A resolved wheel target for one frame: the surface entity and the cell
/// height (for delta scaling).
struct WheelTarget {
    target: Entity,
    cell_h: f32,
}

/// Routes this frame's wheel messages to the terminal under the cursor.
///
/// The horizontal axis is still accumulated so the dominant-axis lock can
/// stop a horizontal-dominant gesture from leaking a vertical scroll, but
/// it is never routed anywhere: D17a of the engine-swap design drops
/// horizontal wheel reporting entirely.
fn dispatch_mouse_wheel(
    mut commands: Commands,
    mut gesture_acc: ResMut<WheelAccumulator>,
    mut wheel: MessageReader<MouseWheel>,
    terminals: TerminalSurfaces,
    cfg: Res<OrzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(wt) = resolve_wheel_target(&terminals, &windows, &metrics) else {
        wheel.clear();
        return;
    };
    gesture_acc.retarget(wt.target);
    let (raw_v, _raw_h) = accumulate_wheel(&mut gesture_acc, &mut wheel, wt.cell_h, &cfg);
    apply_vertical_scroll(&mut commands, wt.target, raw_v, &keys, &cfg);
}

/// Resolves the focused window's cursor to the topmost terminal surface
/// under it, or `None` on any miss (the caller drains the wheel reader).
fn resolve_wheel_target(
    terminals: &TerminalSurfaces<'_, '_>,
    windows: &Query<&Window, With<PrimaryWindow>>,
    metrics: &TerminalCellMetricsResource,
) -> Option<WheelTarget> {
    let window = windows.single().ok()?;
    if !window.focused || terminals.is_empty() {
        return None;
    }
    let (_, cell_h) = cell_dims(metrics);
    let cursor_phys = window
        .cursor_position()
        .map(|c| c * window.scale_factor())?;
    let target = topmost_surface_at(cursor_phys, hit_candidates(terminals))?;
    Some(WheelTarget { target, cell_h })
}

/// Folds this frame's wheel deltas, applies the dominant-axis lock, and
/// accumulates whole notches per axis. Returns `(raw_v, raw_h)`.
fn accumulate_wheel(
    gesture_acc: &mut WheelAccumulator,
    wheel: &mut MessageReader<MouseWheel>,
    cell_h: f32,
    cfg: &OrzmaMouseConfig,
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

/// Applies the vertical axis of one wheel dispatch: a non-zero notch count
/// always scrolls the target's viewport by `scroll_lines`. Horizontal notches
/// are never routed (D17a) — the caller already discards `raw_h`.
fn apply_vertical_scroll(
    commands: &mut Commands,
    target: Entity,
    raw_v: i32,
    keys: &ButtonInput<KeyCode>,
    cfg: &OrzmaMouseConfig,
) {
    if raw_v == 0 {
        return;
    }
    let fine = fine_held(cfg.fine_modifier, &current_terminal_modifiers(keys));
    let lines = scroll_lines(raw_v, fine, &cfg.wheel);
    commands.trigger(TerminalViewportScroll {
        entity: target,
        lines,
    });
}

/// Converts a signed notch count into a viewport-scroll line count, honoring
/// the fine-scroll modifier. Ports the scrollback branch of the removed
/// engine's `WheelAction::route`, which received `-raw_v` and returned
/// `-(-raw_v) * lines_per`. `raw_v` carries
/// Bevy's wheel sign (positive = wheel up = toward older output), which is
/// also the positive direction of `Scroll::Delta`, so no negation is applied:
/// `TerminalViewportScroll.lines` positive = deeper into scrollback.
fn scroll_lines(raw_v: i32, fine: bool, cfg: &WheelConfig) -> i32 {
    let lines_per = if fine {
        cfg.fine_lines
    } else {
        cfg.lines_per_notch
    } as i32;
    raw_v * lines_per
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
    use crate::action::terminal::TerminalViewportScroll;
    use crate::input::mouse::test_support::{set_phys_cursor, test_metrics};
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::message::Messages;
    use bevy::input::mouse::MouseScrollUnit;
    use bevy::input::touch::TouchPhase;
    use bevy::ui::{ComputedNode, UiGlobalTransform};
    use orzma_tty_renderer::schema::TerminalGrid;

    fn make_wheel_app() -> App {
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseWheel>()
            .init_resource::<OrzmaMouseConfig>()
            .init_resource::<WheelAccumulator>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(test_metrics())
            .add_systems(Update, dispatch_mouse_wheel);

        app.world_mut().spawn((
            OrzmaTerminal,
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
            .resource_mut::<Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: MouseScrollUnit::Line,
                x,
                y,
                window: Entity::PLACEHOLDER,
                phase: TouchPhase::Moved,
            });
    }

    fn disable_axis_lock(app: &mut App) {
        app.insert_resource(OrzmaMouseConfig {
            axis_lock_ratio: 0.0,
            ..default()
        });
    }

    #[derive(Resource, Default)]
    struct CapturedScrolls(Vec<i32>);

    fn capture_viewport_scrolls(app: &mut App) {
        app.init_resource::<CapturedScrolls>();
        app.add_observer(
            |ev: On<TerminalViewportScroll>, mut cap: ResMut<CapturedScrolls>| {
                cap.0.push(ev.lines);
            },
        );
    }

    /// Asserts a vertical wheel-up notch scrolls the target's viewport
    /// toward older output by a positive, non-zero multiple of the
    /// configured `lines_per_notch` (3 by default), unscaled.
    ///
    /// Case: the user spins the wheel while the cursor sits over a terminal
    /// with no app mouse mode active.
    #[test]
    fn dispatch_vertical_wheel_scrolls_viewport() {
        let mut app = make_wheel_app();
        capture_viewport_scrolls(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.0, 1.0);
        app.update();
        let scrolls = app.world().resource::<CapturedScrolls>();
        let total: i32 = scrolls.0.iter().sum();
        assert!(
            total > 0 && total.rem_euclid(3) == 0,
            "wheel up must pass lines_per_notch (3) through unscaled as a positive count, got {total} from {:?}",
            scrolls.0
        );
    }

    /// Asserts wheel-up and wheel-down scroll the viewport in opposite
    /// directions.
    ///
    /// Case: the user reverses the wheel direction mid-gesture.
    #[test]
    fn dispatch_vertical_scrollback_direction_flips_with_wheel() {
        let up = {
            let mut app = make_wheel_app();
            capture_viewport_scrolls(&mut app);
            set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
            write_wheel(&mut app, 0.0, 1.0);
            app.update();
            app.world()
                .resource::<CapturedScrolls>()
                .0
                .iter()
                .sum::<i32>()
        };
        let down = {
            let mut app = make_wheel_app();
            capture_viewport_scrolls(&mut app);
            set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
            write_wheel(&mut app, 0.0, -1.0);
            app.update();
            app.world()
                .resource::<CapturedScrolls>()
                .0
                .iter()
                .sum::<i32>()
        };
        assert!(up > 0, "wheel up must scroll toward older output");
        assert!(
            up != 0 && down != 0 && up.signum() != down.signum(),
            "wheel up and down must scroll the viewport in opposite directions, got up={up} down={down}"
        );
    }

    /// Asserts that a purely horizontal wheel gesture is ignored rather than
    /// routed anywhere, so the viewport never scrolls.
    ///
    /// Case: the user swipes a trackpad left or right over a terminal.
    #[test]
    fn dispatch_horizontal_wheel_never_scrolls_the_viewport() {
        let mut app = make_wheel_app();
        capture_viewport_scrolls(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5, 0.0);
        app.update();
        assert!(
            app.world().resource::<CapturedScrolls>().0.is_empty(),
            "a purely horizontal wheel gesture must not scroll the viewport"
        );
    }

    /// Asserts that the dominant-axis lock absorbs a horizontal-dominant
    /// diagonal gesture whose vertical component alone would exceed the
    /// notch threshold, so the viewport does not scroll.
    ///
    /// Case: an imprecise trackpad swipe intended as a horizontal gesture
    /// carries a small vertical component.
    #[test]
    fn horizontal_dominant_gesture_never_scrolls_the_viewport() {
        let mut app = make_wheel_app();
        capture_viewport_scrolls(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, -2.0, 0.6);
        app.update();
        assert!(
            app.world().resource::<CapturedScrolls>().0.is_empty(),
            "a horizontal-dominant gesture must not leak a vertical scroll"
        );
    }

    /// Asserts the vertical scroll count is unaffected by a horizontal
    /// jitter component once the axis lock zeros it out.
    ///
    /// Case: a vertical-dominant wheel gesture on a trackpad carries a small
    /// horizontal jitter component alongside the intended vertical motion.
    #[test]
    fn vertical_dominant_gesture_still_scrolls_despite_horizontal_jitter() {
        let mut app = make_wheel_app();
        capture_viewport_scrolls(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.6, -2.0);
        app.update();
        let total: i32 = app.world().resource::<CapturedScrolls>().0.iter().sum();
        assert!(
            total != 0,
            "off-axis horizontal jitter must not suppress the intended vertical scroll"
        );
    }

    /// Asserts `scroll_lines` uses `lines_per_notch` by default.
    ///
    /// Case: the user scrolls with no fine-scroll modifier held.
    #[test]
    fn scroll_lines_uses_lines_per_notch_by_default() {
        assert_eq!(scroll_lines(1, false, &WheelConfig::default()), 3);
    }

    /// Asserts `scroll_lines` uses `fine_lines` when the fine modifier is
    /// held and keeps the wheel's sign.
    ///
    /// Case: the user holds the configured fine-scroll modifier (Alt by
    /// default) while spinning the wheel to slow-scroll.
    #[test]
    fn scroll_lines_fine_modifier_uses_fine_lines() {
        assert_eq!(scroll_lines(-2, true, &WheelConfig::default()), -2);
    }

    /// Asserts a diagonal gesture with the axis lock disabled still routes
    /// only its vertical component to the viewport (horizontal stays
    /// unrouted regardless of the lock).
    ///
    /// Case: a config with `axis_lock_ratio: 0.0` (lock disabled) receives a
    /// diagonal wheel gesture.
    #[test]
    fn dispatch_diagonal_with_lock_disabled_still_only_scrolls_vertically() {
        let mut app = make_wheel_app();
        disable_axis_lock(&mut app);
        capture_viewport_scrolls(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5, -0.5);
        app.update();
        let total: i32 = app.world().resource::<CapturedScrolls>().0.iter().sum();
        assert!(total != 0, "the vertical component must still scroll");
    }
}
