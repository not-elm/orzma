//! Requests exactly one follow-up frame after an update that took in
//! outside input, so work an update hands to the next one reaches the
//! screen.

use crate::redraw::wake::WakeGate;
use bevy::ecs::message::Messages;
use bevy::prelude::*;
use bevy::window::{RequestRedraw, WindowEvent};

/// Requests one follow-up frame after input wakes and window input.
pub(super) struct FollowUpPlugin {
    gate: WakeGate,
}

impl FollowUpPlugin {
    /// Builds the plugin around `gate`, the pending-wake flag the app's
    /// input waker sets.
    pub fn new(gate: WakeGate) -> Self {
        Self { gate }
    }
}

impl Plugin for FollowUpPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: the gate must be taken before the `Update` systems drain the
        // queues its wakes announce. A wake armed after a drain but before a
        // later take sends nothing, stranding its item until an unrelated
        // wake; running in `First` keeps the take ahead of every drain.
        app.insert_resource(self.gate.clone())
            .add_message::<RequestRedraw>()
            .add_message::<WindowEvent>()
            .add_systems(First, request_follow_up);
    }
}

/// Clears the previous update's redraw request, then requests exactly one
/// follow-up frame when an input wake or a window input arrived.
fn request_follow_up(
    mut redraws: ResMut<Messages<RequestRedraw>>,
    mut events: MessageReader<WindowEvent>,
    gate: Res<WakeGate>,
) {
    let woke = gate.take();
    let mut input = false;
    for event in events.read() {
        input |= counts_as_input(event);
    }
    // NOTE: bevy_winit re-reads every `RequestRedraw` in `Messages` after
    // each update with a fresh cursor, so a request left in the buffer
    // would run another update, and the clear below keeps each trigger
    // to exactly one follow-up. The clear also drops any `RequestRedraw`
    // that another system wrote earlier in the same update, so no other
    // system may rely on writing `RequestRedraw` before this one runs in
    // `First`.
    if !redraws.is_empty() {
        redraws.clear();
    }
    if woke || input {
        redraws.write(RequestRedraw);
    }
}

/// Whether a window event counts as outside input: every event except the
/// device-level `MouseMotion` and the app's own `RequestRedraw`.
fn counts_as_input(event: &WindowEvent) -> bool {
    !matches!(
        event,
        WindowEvent::MouseMotion(_) | WindowEvent::RequestRedraw(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::mouse::MouseMotion;
    use bevy::window::WindowFocused;

    fn app(gate: &WakeGate) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(FollowUpPlugin::new(gate.clone()));
        app
    }

    fn redraws(app: &App) -> usize {
        app.world().resource::<Messages<RequestRedraw>>().len()
    }

    fn key_press() -> WindowEvent {
        WindowEvent::KeyboardInput(KeyboardInput {
            key_code: KeyCode::KeyA,
            logical_key: Key::Character("a".into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        })
    }

    fn mouse_motion() -> WindowEvent {
        WindowEvent::MouseMotion(MouseMotion { delta: Vec2::ONE })
    }

    /// A test resource recording the return of `WakeGate::arm` from an
    /// `Update` system.
    #[derive(Resource, Default)]
    struct ArmResult(Option<bool>);

    /// Arms the gate again and records whether this call newly armed it.
    fn record_arm(mut result: ResMut<ArmResult>, gate: Res<WakeGate>) {
        result.0 = Some(gate.arm());
    }

    /// Asserts that a keyboard press and a window-focus change count as
    /// outside input, while mouse motion and the app's own redraw request
    /// do not.
    ///
    /// Case: the user presses a key and brings the window to focus, and
    /// separately moves the mouse over another monitor while the window
    /// system also reports its own pending redraw.
    #[test]
    fn window_input_counts_but_mouse_motion_and_redraw_requests_do_not() {
        assert!(counts_as_input(&key_press()));
        assert!(counts_as_input(&WindowEvent::WindowFocused(
            WindowFocused {
                window: Entity::PLACEHOLDER,
                focused: true,
            }
        )));
        assert!(!counts_as_input(&mouse_motion()));
        assert!(!counts_as_input(&WindowEvent::RequestRedraw(RequestRedraw)));
    }

    /// Asserts that an armed gate yields exactly one follow-up request and
    /// is taken.
    ///
    /// Case: the backend sends a frame while orzma is idle.
    #[test]
    fn an_armed_gate_requests_one_follow_up_and_is_taken() {
        let gate = WakeGate::detached();
        let mut app = app(&gate);
        assert!(gate.arm());
        app.update();
        assert_eq!(redraws(&app), 1);
        assert!(!gate.take());
    }

    /// Asserts that a keyboard window event requests a follow-up while
    /// mouse motion alone does not.
    ///
    /// Case: the user zooms with a shortcut, and separately moves the mouse
    /// over another monitor while orzma has focus.
    #[test]
    fn window_input_requests_a_follow_up_but_mouse_motion_does_not() {
        let gate = WakeGate::detached();
        let mut typed = app(&gate);
        typed.world_mut().write_message(key_press());
        typed.update();
        assert_eq!(redraws(&typed), 1);

        let mut moved = app(&gate);
        moved.world_mut().write_message(mouse_motion());
        moved.update();
        assert_eq!(redraws(&moved), 0);
    }

    /// Asserts that two triggers arriving in the same update still yield
    /// exactly one follow-up request, which the next update clears.
    ///
    /// Case: the backend sends a frame and the user presses a key within
    /// the same update.
    #[test]
    fn two_triggers_in_one_update_yield_one_request_that_the_next_update_clears() {
        let gate = WakeGate::detached();
        let mut app = app(&gate);
        assert!(gate.arm());
        app.world_mut().write_message(key_press());
        app.update();
        assert_eq!(redraws(&app), 1);
        app.update();
        assert_eq!(redraws(&app), 0);
    }

    /// Asserts that `First` has already taken the gate by the time
    /// `Update` runs.
    ///
    /// Case: the backend arms the gate for a wake, and orzma's next
    /// update runs.
    #[test]
    fn the_gate_is_taken_in_first_before_update_runs() {
        let gate = WakeGate::detached();
        let mut app = app(&gate);
        app.init_resource::<ArmResult>()
            .add_systems(Update, record_arm);
        assert!(gate.arm());
        app.update();
        assert_eq!(app.world().resource::<ArmResult>().0, Some(true));
    }

    /// Asserts that a follow-up request drains every window event queued
    /// in the update, not just the first one.
    ///
    /// Case: two keys arrive in the same update.
    #[test]
    fn every_queued_window_event_is_read_in_one_update() {
        let gate = WakeGate::detached();
        let mut app = app(&gate);
        app.world_mut().write_message(key_press());
        app.world_mut().write_message(key_press());
        app.update();
        assert_eq!(redraws(&app), 1);
        app.update();
        assert_eq!(redraws(&app), 0);
    }
}
