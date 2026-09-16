//! Mouse-wheel dispatch for every `OrzmaTerminal` surface: sub-notch
//! accumulation and a dominant-axis lock, then one `RequestTtyWheel`
//! handing the notches to the terminal under the cursor.

use super::{
    TerminalSurfaces, cell_context_for, cell_dims, hit_candidates, on_any_mouse_message,
    protocol_mods,
};
use crate::input::InputPhase;
use crate::input::bindings::{FineModifier, OrzmaMouseConfig};
use crate::input::keyboard::current_terminal_modifiers;
use crate::input::mouse::gesture::{
    WheelAccumulator, accumulate_notches, lock_dominant_axis, wheel_delta_cells,
};
use crate::surface::geometry::topmost_surface_at;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_orzma_tty_renderer::TerminalCellMetricsResource;
use bevy_orzmux::prelude::RequestTtyWheel;
use orzma_tty::prelude::{TerminalModifiers, WheelInput, WheelModifiers};

/// Adds mouse-wheel dispatch and its notch-accumulator resource.
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

/// Whether the OS delivers a discrete Shift+wheel as horizontal travel,
/// so a Shift-held frame's line-unit horizontal travel is folded onto the
/// vertical axis.
const SHIFT_WHEEL_ARRIVES_HORIZONTAL: bool = cfg!(target_os = "macos");

/// A resolved wheel target for one frame: the surface entity, the cursor
/// that hit it, and the cell pitch.
struct WheelTarget {
    target: Entity,
    cursor_phys: Vec2,
    cell_w: f32,
    cell_h: f32,
}

/// Hands this frame's wheel notches to the terminal under the cursor as
/// one `RequestTtyWheel`, after normalizing the gesture: sub-notch
/// accumulation, the dominant-axis lock, the macOS Shift fold, the fine
/// modifier, and the cell under the cursor.
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
    if wheel.is_empty() {
        return;
    }
    let held = current_terminal_modifiers(&keys);
    let fold_shift = SHIFT_WHEEL_ARRIVES_HORIZONTAL && held.shift;
    let (up, right) = accumulate_wheel(&mut gesture_acc, wheel.read(), wt.cell_h, fold_shift, &cfg);
    if up == 0 && right == 0 {
        return;
    }
    let mods = wheel_modifiers(&held, cfg.fine_modifier, SHIFT_WHEEL_ARRIVES_HORIZONTAL);
    let cell = cell_context_for(&terminals, wt.target, wt.cell_w, wt.cell_h)
        .and_then(|ctx| ctx.hit(wt.cursor_phys))
        .map(|(cell, _)| cell);
    commands.trigger(RequestTtyWheel {
        terminal: wt.target,
        input: WheelInput {
            up,
            right,
            mods,
            cell,
            report_mods: protocol_mods(&held),
        },
    });
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
    let (cell_w, cell_h) = cell_dims(metrics);
    let cursor_phys = window
        .cursor_position()
        .map(|c| c * window.scale_factor())?;
    let target = topmost_surface_at(cursor_phys, hit_candidates(terminals))?;
    Some(WheelTarget {
        target,
        cursor_phys,
        cell_w,
        cell_h,
    })
}

/// Sums this frame's wheel travel in cells, applies the dominant-axis
/// lock, and accumulates whole notches per axis. Returns `(up, right)`
/// notches.
///
/// When `fold_shift` is set, the horizontal residual is cleared and each
/// line-unit event's horizontal travel is added onto the vertical axis as
/// winit orients it, so a wheel-up the OS delivered as horizontal travel
/// counts as up; pixel-unit travel keeps its axes.
fn accumulate_wheel<'a>(
    gesture_acc: &mut WheelAccumulator,
    wheel: impl IntoIterator<Item = &'a MouseWheel>,
    cell_h: f32,
    fold_shift: bool,
    cfg: &OrzmaMouseConfig,
) -> (i32, i32) {
    if fold_shift {
        gesture_acc.residual_cells_h = 0.0;
    }
    let (delta_up, delta_x) = wheel.into_iter().fold((0.0f32, 0.0f32), |(v, h), ev| {
        // NOTE: BOTH axes divide by cell_h (line height), not cell_w, so a given
        // finger distance yields the same notch rate horizontally and vertically.
        // Using the narrower cell_w (advance_phys, ~half of line_height_phys) made
        // horizontal ~2x too sensitive — do not "correct" ev.x to cell_w.
        let cells_up = wheel_delta_cells(ev.unit, ev.y, cell_h);
        let cells_x = wheel_delta_cells(ev.unit, ev.x, cell_h);
        if fold_shift && matches!(ev.unit, MouseScrollUnit::Line) {
            (v + cells_up + cells_x, h)
        } else {
            (v + cells_up, h + cells_x)
        }
    });
    // NOTE: do NOT also clear the suppressed axis's residual here. The lock
    // zeros the off-axis delta before accumulation, so it adds 0 and cannot leak
    // a notch; clearing would instead wipe genuine sub-notch progress on a
    // deliberate horizontal swipe whose slow frames dip below the lock ratio.
    let (delta_up, delta_right) =
        lock_dominant_axis(delta_up, rightward(delta_x), cfg.axis_lock_ratio);
    let up = accumulate_notches(
        &mut gesture_acc.residual_cells,
        delta_up,
        cfg.cells_per_notch,
    );
    let right = accumulate_notches(
        &mut gesture_acc.residual_cells_h,
        delta_right,
        cfg.cells_per_notch,
    );
    (up, right)
}

/// Orients a frame's horizontal wheel travel so that positive points
/// right. winit reports a positive `MouseWheel.x` as the content moving
/// right, which scrolls toward the left, on every platform.
fn rightward(delta_x: f32) -> f32 {
    -delta_x
}

/// The wheel-routing modifiers for the held keys. Where Shift+wheel
/// arrives horizontal, Shift only falls through and never selects fine
/// scrolling.
fn wheel_modifiers(
    held: &TerminalModifiers,
    fine_modifier: FineModifier,
    shift_wheel_arrives_horizontal: bool,
) -> WheelModifiers {
    let fine = match fine_modifier {
        FineModifier::Shift => held.shift && !shift_wheel_arrives_horizontal,
        FineModifier::Ctrl => held.ctrl,
        FineModifier::Alt => held.alt,
        FineModifier::None => true,
    };
    WheelModifiers {
        shift: held.shift,
        fine,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::mouse::test_support::{set_phys_cursor, test_metrics};
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::message::Messages;
    use bevy::input::mouse::MouseScrollUnit;
    use bevy::input::touch::TouchPhase;
    use bevy::ui::{ComputedNode, UiGlobalTransform};
    use bevy_orzma_tty_renderer::schema::TerminalView;
    use orzma_tty::prelude::CellCoord;

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
            TerminalView {
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
        capture_wheel(&mut app);
        app
    }

    fn wheel_event(unit: MouseScrollUnit, x: f32, y: f32) -> MouseWheel {
        MouseWheel {
            unit,
            x,
            y,
            window: Entity::PLACEHOLDER,
            phase: TouchPhase::Moved,
        }
    }

    fn write_wheel(app: &mut App, x: f32, y: f32) {
        app.world_mut()
            .resource_mut::<Messages<MouseWheel>>()
            .write(wheel_event(MouseScrollUnit::Line, x, y));
    }

    fn disable_axis_lock(app: &mut App) {
        app.insert_resource(OrzmaMouseConfig {
            axis_lock_ratio: 0.0,
            ..default()
        });
    }

    fn press_shift(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
    }

    #[derive(Resource, Default)]
    struct CapturedWheel(Vec<WheelInput>);

    fn capture_wheel(app: &mut App) {
        app.init_resource::<CapturedWheel>().add_observer(
            |ev: On<RequestTtyWheel>, mut cap: ResMut<CapturedWheel>| {
                cap.0.push(ev.input);
            },
        );
    }

    /// Runs one update over a frame carrying `(x, y)` line-unit wheel
    /// travel with the cursor over cell (6, 4), and returns the requests
    /// it produced.
    fn dispatch(app: &mut App, x: f32, y: f32) -> Vec<WheelInput> {
        set_phys_cursor(app, Vec2::new(40.0, 48.0));
        write_wheel(app, x, y);
        app.update();
        app.world().resource::<CapturedWheel>().0.clone()
    }

    /// Asserts that one line of wheel-up travel sends a request of two
    /// positive vertical notches and no horizontal notch.
    ///
    /// Case: the user spins a discrete mouse wheel up one click over a
    /// terminal.
    #[test]
    fn a_wheel_up_line_sends_two_vertical_notches() {
        let mut app = make_wheel_app();
        let sent = dispatch(&mut app, 0.0, 1.0);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].up, 2);
        assert_eq!(sent[0].right, 0);
    }

    /// Asserts that wheel-up and wheel-down send vertical notches of
    /// opposite sign.
    ///
    /// Case: the user reverses the wheel direction mid-gesture.
    #[test]
    fn wheel_direction_flips_the_sign_of_the_vertical_notches() {
        let mut up_app = make_wheel_app();
        let up = dispatch(&mut up_app, 0.0, 1.0);
        let mut down_app = make_wheel_app();
        let down = dispatch(&mut down_app, 0.0, -1.0);
        assert!(up[0].up > 0);
        assert!(down[0].up < 0);
        assert_eq!(up[0].up, -down[0].up);
    }

    /// Asserts that a positive horizontal delta, which winit defines as
    /// the content moving right, sends a leftward notch and no vertical
    /// notch.
    ///
    /// Case: the user scrolls sideways toward the start of a long line.
    #[test]
    fn a_positive_horizontal_delta_sends_a_leftward_notch_only() {
        let mut app = make_wheel_app();
        let sent = dispatch(&mut app, 0.5, 0.0);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].up, 0);
        assert_eq!(sent[0].right, -1);
    }

    /// Asserts that the dominant-axis lock zeroes the vertical component
    /// of a horizontal-dominant gesture whose vertical travel alone would
    /// exceed the notch threshold.
    ///
    /// Case: an imprecise trackpad swipe intended as a horizontal gesture
    /// carries a small vertical component.
    #[test]
    fn a_horizontal_dominant_gesture_sends_no_vertical_notch() {
        let mut app = make_wheel_app();
        let sent = dispatch(&mut app, -2.0, 0.6);
        assert!(sent.iter().all(|input| input.up == 0));
        assert!(sent.iter().any(|input| input.right != 0));
    }

    /// Asserts that a vertical-dominant gesture keeps its vertical
    /// notches and drops the horizontal jitter.
    ///
    /// Case: a vertical wheel gesture on a trackpad carries a small
    /// horizontal jitter component.
    #[test]
    fn a_vertical_dominant_gesture_keeps_its_vertical_notches() {
        let mut app = make_wheel_app();
        let sent = dispatch(&mut app, 0.6, -2.0);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].up != 0);
        assert_eq!(sent[0].right, 0);
    }

    /// Asserts that a diagonal gesture with the axis lock disabled sends
    /// both axes' notches in one request.
    ///
    /// Case: a config with `axis_lock_ratio = 0.0` receives a diagonal
    /// trackpad swipe.
    #[test]
    fn a_diagonal_gesture_with_the_lock_disabled_sends_both_axes() {
        let mut app = make_wheel_app();
        disable_axis_lock(&mut app);
        let sent = dispatch(&mut app, 0.5, -0.5);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].up, -1);
        assert_eq!(sent[0].right, -1);
    }

    /// Asserts that a request carries the cell under the cursor.
    ///
    /// Case: the user spins the wheel with the cursor over the seventh
    /// column of the fourth row.
    #[test]
    fn a_request_carries_the_cell_under_the_cursor() {
        let mut app = make_wheel_app();
        let sent = dispatch(&mut app, 0.0, 1.0);
        assert_eq!(sent[0].cell, Some(CellCoord { col: 6, row: 4 }));
    }

    /// Asserts that a held Alt reaches the request both as the report's
    /// meta bit and as the fine-scroll modifier.
    ///
    /// Case: the user holds Option, the default fine-scroll modifier,
    /// while spinning the wheel.
    #[test]
    fn a_held_alt_reaches_the_report_bits_and_the_fine_modifier() {
        let mut app = make_wheel_app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::AltLeft);
        let sent = dispatch(&mut app, 0.0, 1.0);
        assert!(sent[0].report_mods.alt);
        assert!(sent[0].mods.fine);
    }

    /// Asserts that a Shift-held frame's horizontal travel folds onto the
    /// vertical axis on macOS and stays horizontal elsewhere, with Shift
    /// reported in the routing modifiers either way.
    ///
    /// Case: the user holds Shift and spins a discrete mouse wheel up,
    /// which macOS delivers as horizontal travel.
    #[test]
    fn shift_held_horizontal_travel_folds_onto_the_vertical_axis_on_macos() {
        let mut app = make_wheel_app();
        press_shift(&mut app);
        let sent = dispatch(&mut app, 1.0, 0.0);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].mods.shift);
        if cfg!(target_os = "macos") {
            assert_eq!(sent[0].up, 2);
            assert_eq!(sent[0].right, 0);
        } else {
            assert_eq!(sent[0].up, 0);
            assert_eq!(sent[0].right, -2);
        }
    }

    /// Asserts that folding adds a line-unit event's horizontal travel onto
    /// the vertical axis as winit orients it, while an unfolded frame keeps
    /// the travel horizontal.
    ///
    /// Case: the user holds Shift and spins a discrete wheel up, which macOS
    /// delivers as horizontal travel.
    #[test]
    fn folding_turns_line_unit_horizontal_travel_into_vertical_notches() {
        let cfg = OrzmaMouseConfig::default();
        let frame = [wheel_event(MouseScrollUnit::Line, 1.0, 0.0)];
        let mut folded = WheelAccumulator::default();
        assert_eq!(
            accumulate_wheel(&mut folded, &frame, 16.0, true, &cfg),
            (2, 0)
        );
        let mut unfolded = WheelAccumulator::default();
        assert_eq!(
            accumulate_wheel(&mut unfolded, &frame, 16.0, false, &cfg),
            (0, -2)
        );
    }

    /// Asserts that folding leaves pixel-unit travel on its own axes.
    ///
    /// Case: on macOS the user holds Shift while swiping a trackpad
    /// sideways, which the OS does not turn into vertical travel.
    #[test]
    fn folding_leaves_pixel_unit_travel_on_its_axes() {
        let cfg = OrzmaMouseConfig::default();
        let frame = [wheel_event(MouseScrollUnit::Pixel, -40.0, 0.0)];
        let mut acc = WheelAccumulator::default();
        assert_eq!(accumulate_wheel(&mut acc, &frame, 16.0, true, &cfg), (0, 5));
    }

    /// Asserts that a folded frame clears the horizontal sub-notch residual
    /// and an unfolded frame leaves it in place.
    ///
    /// Case: the user drifts a trackpad sideways short of a notch, then
    /// holds Shift and spins the wheel.
    #[test]
    fn a_folded_frame_clears_the_horizontal_residual() {
        let cfg = OrzmaMouseConfig::default();
        let drift = [wheel_event(MouseScrollUnit::Line, 0.25, 0.0)];
        let spin = [wheel_event(MouseScrollUnit::Line, 0.0, 1.0)];
        let mut folded = WheelAccumulator::default();
        let mut unfolded = WheelAccumulator::default();
        accumulate_wheel(&mut folded, &drift, 16.0, false, &cfg);
        accumulate_wheel(&mut unfolded, &drift, 16.0, false, &cfg);
        accumulate_wheel(&mut folded, &spin, 16.0, true, &cfg);
        accumulate_wheel(&mut unfolded, &spin, 16.0, false, &cfg);
        assert_eq!(folded.residual_cells_h, 0.0);
        assert_eq!(unfolded.residual_cells_h, -0.25);
    }

    /// Asserts that Shift configured as the fine modifier never selects
    /// fine scrolling where Shift+wheel arrives horizontal, still does
    /// elsewhere, and that another fine modifier is unaffected.
    ///
    /// Case: a user whose config sets `fine_modifier = "shift"` holds
    /// Shift while scrolling, once on macOS and once on Windows, and
    /// another user holds the default Alt.
    #[test]
    fn shift_as_the_fine_modifier_only_falls_through_where_shift_wheel_arrives_horizontal() {
        let shift = TerminalModifiers {
            shift: true,
            ..TerminalModifiers::default()
        };
        assert_eq!(
            wheel_modifiers(&shift, FineModifier::Shift, true),
            WheelModifiers {
                shift: true,
                fine: false
            }
        );
        assert_eq!(
            wheel_modifiers(&shift, FineModifier::Shift, false),
            WheelModifiers {
                shift: true,
                fine: true
            }
        );
        let alt = TerminalModifiers {
            alt: true,
            ..TerminalModifiers::default()
        };
        assert_eq!(
            wheel_modifiers(&alt, FineModifier::Alt, true),
            WheelModifiers {
                shift: false,
                fine: true
            }
        );
    }
}
