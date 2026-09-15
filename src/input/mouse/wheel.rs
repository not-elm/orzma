//! Mouse-wheel dispatch for every `OrzmaTerminal` surface: sub-notch
//! accumulation and a dominant-axis lock, then routing by the terminal's
//! modes to mouse reports, cursor keys, or the scrollback.

use super::{
    TerminalSurfaces, cell_context_for, cell_dims, hit_candidates, on_any_mouse_message,
    protocol_mods,
};
use crate::action::terminal::TerminalViewportScroll;
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
use bevy_orzmux::prelude::{RequestTtyKeyInput, RequestTtyMouseInput, TtyModes};
use orzma_tty::prelude::{
    CellCoord, MouseReport, MouseReportKind, ProtocolModifiers, TerminalModifiers, WheelDecision,
    WheelModifiers,
};

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

/// Routes this frame's wheel notches to the terminal under the cursor.
///
/// Vertical notches become mouse reports while the application tracks the
/// mouse and Shift is not held, cursor keys while alternate scroll is in
/// effect, and a viewport scroll otherwise. Horizontal notches become
/// mouse reports only.
fn dispatch_mouse_wheel(
    mut commands: Commands,
    mut gesture_acc: ResMut<WheelAccumulator>,
    mut wheel: MessageReader<MouseWheel>,
    terminals: TerminalSurfaces,
    pane_modes: Query<&TtyModes>,
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
    let modes = pane_modes.get(wt.target).copied().unwrap_or_default().0;
    let mods = wheel_modifiers(&held, cfg.fine_modifier, SHIFT_WHEEL_ARRIVES_HORIZONTAL);
    let cell = cell_context_for(&terminals, wt.target, wt.cell_w, wt.cell_h)
        .and_then(|ctx| ctx.hit(wt.cursor_phys))
        .map(|(cell, _)| cell);
    let report_mods = protocol_mods(&held);
    for decision in [
        WheelDecision::route(modes, up, mods, &cfg.wheel),
        WheelDecision::route_horizontal(modes, right, mods, &cfg.wheel),
    ] {
        trigger_wheel_decision(&mut commands, wt.target, decision, cell, report_mods);
    }
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

/// Fans one routing decision out to the request it names on `target`. A
/// report needs the cell under the cursor and is dropped without one.
fn trigger_wheel_decision(
    commands: &mut Commands,
    target: Entity,
    decision: WheelDecision,
    cell: Option<CellCoord>,
    mods: ProtocolModifiers,
) {
    match decision {
        WheelDecision::Report { button, count } => {
            let Some(cell) = cell else {
                return;
            };
            let mouse = MouseReport {
                button,
                kind: MouseReportKind::Press,
                cell,
                mods,
            };
            for _ in 0..count {
                commands.trigger(RequestTtyMouseInput {
                    terminal: target,
                    mouse,
                });
            }
        }
        WheelDecision::CursorKeys { key, count } => {
            for _ in 0..count {
                commands.trigger(RequestTtyKeyInput {
                    terminal: target,
                    key: key.clone(),
                    modifiers: TerminalModifiers::default(),
                });
            }
        }
        WheelDecision::ScrollViewport(lines) => commands.trigger(TerminalViewportScroll {
            entity: target,
            lines,
        }),
        WheelDecision::Noop => {}
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
    use bevy_orzma_tty_renderer::schema::TerminalView;
    use orzma_tty::prelude::{MouseButton, TerminalKey};
    use orzma_vt::prelude::{MouseTracking, ScreenKind, VtModes};

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

    #[derive(Resource, Default)]
    struct CapturedReports(Vec<MouseReport>);

    #[derive(Resource, Default)]
    struct CapturedKeys(Vec<TerminalKey>);

    fn capture_forwarded_input(app: &mut App) {
        app.init_resource::<CapturedReports>()
            .init_resource::<CapturedKeys>()
            .add_observer(
                |ev: On<RequestTtyMouseInput>, mut cap: ResMut<CapturedReports>| {
                    cap.0.push(ev.mouse);
                },
            )
            .add_observer(
                |ev: On<RequestTtyKeyInput>, mut cap: ResMut<CapturedKeys>| {
                    cap.0.push(ev.key.clone());
                },
            );
    }

    fn fixture_terminal(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<OrzmaTerminal>>()
            .single(app.world())
            .expect("the fixture spawns one terminal")
    }

    fn set_modes(app: &mut App, modes: VtModes) {
        let terminal = fixture_terminal(app);
        app.world_mut().entity_mut(terminal).insert(TtyModes(modes));
    }

    fn press_shift(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ShiftLeft);
    }

    fn tracking_modes() -> VtModes {
        VtModes {
            mouse_tracking: MouseTracking::Drag,
            ..VtModes::default()
        }
    }

    fn alternate_screen_modes() -> VtModes {
        VtModes {
            active_screen: ScreenKind::Alternate,
            ..VtModes::default()
        }
    }

    fn tracking_alternate_screen_modes() -> VtModes {
        VtModes {
            mouse_tracking: MouseTracking::Drag,
            active_screen: ScreenKind::Alternate,
            ..VtModes::default()
        }
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

    /// Asserts that a purely horizontal wheel gesture over a terminal that
    /// is not tracking the mouse never scrolls the viewport.
    ///
    /// Case: the user swipes a trackpad left or right over a shell prompt.
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

    /// Asserts that a diagonal gesture with the axis lock disabled still
    /// scrolls the viewport by its vertical component over a terminal that
    /// is not tracking the mouse.
    ///
    /// Case: a config with `axis_lock_ratio: 0.0` (lock disabled) receives a
    /// diagonal wheel gesture at a shell prompt.
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

    /// Asserts that a wheel-up gesture over a mouse-tracking terminal sends
    /// one wheel-up press report per notch at the cell under the cursor,
    /// and scrolls nothing.
    ///
    /// Case: nvim runs with `mouse=nvi`, so button-event tracking is on,
    /// and the user spins the wheel up over its buffer.
    #[test]
    fn a_tracking_terminal_receives_a_wheel_report_per_notch() {
        let mut app = make_wheel_app();
        set_modes(&mut app, tracking_modes());
        capture_viewport_scrolls(&mut app);
        capture_forwarded_input(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.0, 1.0);
        app.update();
        let expected = MouseReport {
            button: MouseButton::WheelUp,
            kind: MouseReportKind::Press,
            cell: CellCoord { col: 6, row: 4 },
            mods: ProtocolModifiers::default(),
        };
        assert_eq!(
            app.world().resource::<CapturedReports>().0,
            vec![expected; 2]
        );
        assert!(app.world().resource::<CapturedScrolls>().0.is_empty());
    }

    /// Asserts that a wheel report over a mouse-tracking terminal carries
    /// a held Alt as its meta bit.
    ///
    /// Case: the user holds Option, the default fine-scroll modifier,
    /// while spinning the wheel over nvim.
    #[test]
    fn a_wheel_report_carries_a_held_alt() {
        let mut app = make_wheel_app();
        set_modes(&mut app, tracking_modes());
        capture_forwarded_input(&mut app);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::AltLeft);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.0, 1.0);
        app.update();
        let reports = &app.world().resource::<CapturedReports>().0;
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|report| report.mods.alt));
    }

    /// Asserts that a wheel-up gesture over the alternate screen without
    /// mouse tracking sends the notches' lines as cursor-up keys and
    /// scrolls nothing.
    ///
    /// Case: `less` shows a long file on the alternate screen without
    /// tracking the mouse, and the user spins the wheel up.
    #[test]
    fn the_alternate_screen_without_tracking_receives_cursor_keys() {
        let mut app = make_wheel_app();
        set_modes(&mut app, alternate_screen_modes());
        capture_viewport_scrolls(&mut app);
        capture_forwarded_input(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.0, 1.0);
        app.update();
        assert_eq!(
            app.world().resource::<CapturedKeys>().0,
            vec![TerminalKey::ArrowUp; 6]
        );
        assert!(app.world().resource::<CapturedReports>().0.is_empty());
        assert!(app.world().resource::<CapturedScrolls>().0.is_empty());
    }

    /// Asserts that Shift over a mouse-tracking alternate screen skips the
    /// reports and sends cursor keys instead.
    ///
    /// Case: the user holds Shift and scrolls a trackpad over nvim, whose
    /// mouse tracking would otherwise take the wheel.
    #[test]
    fn shift_over_a_tracking_alternate_screen_sends_cursor_keys() {
        let mut app = make_wheel_app();
        set_modes(&mut app, tracking_alternate_screen_modes());
        capture_forwarded_input(&mut app);
        press_shift(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.0, 1.0);
        app.update();
        assert_eq!(
            app.world().resource::<CapturedKeys>().0,
            vec![TerminalKey::ArrowUp; 6]
        );
        assert!(app.world().resource::<CapturedReports>().0.is_empty());
    }

    /// Asserts that a positive horizontal wheel delta, which winit defines
    /// as the content moving right, reports the wheel-left button over a
    /// mouse-tracking terminal on every platform.
    ///
    /// Case: the user scrolls sideways toward the start of a long line
    /// over a tracking application.
    #[test]
    fn a_positive_horizontal_delta_over_a_tracking_terminal_reports_wheel_left() {
        let mut app = make_wheel_app();
        set_modes(&mut app, tracking_modes());
        capture_forwarded_input(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 0.5, 0.0);
        app.update();
        let buttons: Vec<MouseButton> = app
            .world()
            .resource::<CapturedReports>()
            .0
            .iter()
            .map(|report| report.button)
            .collect();
        assert_eq!(buttons, vec![MouseButton::WheelLeft]);
    }

    /// Asserts that a Shift-held frame's horizontal travel folds onto the
    /// vertical axis on macOS, reaching the cursor-key route there, and
    /// stays horizontal elsewhere, where Shift then leaves it unrouted.
    ///
    /// Case: on macOS the user holds Shift and spins a discrete mouse
    /// wheel up over nvim, and the OS delivers that as horizontal travel.
    #[test]
    fn shift_held_horizontal_travel_folds_onto_the_vertical_axis_on_macos() {
        let mut app = make_wheel_app();
        set_modes(&mut app, tracking_alternate_screen_modes());
        capture_forwarded_input(&mut app);
        press_shift(&mut app);
        set_phys_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_wheel(&mut app, 1.0, 0.0);
        app.update();
        let keys = &app.world().resource::<CapturedKeys>().0;
        if cfg!(target_os = "macos") {
            assert_eq!(*keys, vec![TerminalKey::ArrowUp; 6]);
        } else {
            assert!(keys.is_empty());
        }
        assert!(app.world().resource::<CapturedReports>().0.is_empty());
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
