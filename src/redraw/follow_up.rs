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
    if !redraws.is_empty() {
        redraws.clear();
    }
    if woke || input {
        redraws.write(RequestRedraw);
    }
}

/// Whether a window event counts as outside input: every event except the
/// device-level `MouseMotion`.
fn counts_as_input(event: &WindowEvent) -> bool {
    !matches!(event, WindowEvent::MouseMotion(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::mouse::MouseMotion;

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

    /// Asserts that every window event except `MouseMotion` counts as
    /// outside input.
    ///
    /// Case: the user presses a key over the window, and separately moves
    /// the mouse over another monitor while orzma has focus.
    #[test]
    fn every_window_event_but_mouse_motion_counts_as_input() {
        assert!(counts_as_input(&key_press()));
        assert!(!counts_as_input(&mouse_motion()));
    }

    /// Asserts that an armed gate yields exactly one follow-up request and
    /// is taken.
    ///
    /// Case: the backend sent a frame while orzma was idle, and its update
    /// must be followed by one more.
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

    /// Asserts that the previous update's request is cleared, so each
    /// trigger yields exactly one follow-up frame.
    ///
    /// Case: one PTY frame arrives, and the follow-up update that runs next
    /// has nothing new to take in.
    #[test]
    fn the_previous_request_is_cleared_so_one_trigger_yields_one_frame() {
        let gate = WakeGate::detached();
        let mut app = app(&gate);
        assert!(gate.arm());
        app.world_mut().write_message(key_press());
        app.update();
        assert_eq!(redraws(&app), 1);
        app.update();
        assert_eq!(redraws(&app), 0);
    }
}
