//! Mouse gesture arbiter for the tmux backend.
//!
//! Owns a single left-button state machine (`TmuxMouseGesture`) that reads raw
//! `MouseButtonInput` messages and issues `select-pane` on a focused press.
//! This is the sole authority over pane-body left-button gestures in the tmux
//! backend; later phases add divider-resize and drag-select to the same
//! state machine.

use crate::input::InputPhase;
use bevy::input::ButtonState;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use ozmux_tmux::{PaneId, TmuxConnection, TmuxPane, select_pane_command};

/// Bevy plugin that registers the tmux mouse gesture arbiter.
pub(crate) struct OzmuxTmuxMousePlugin;

impl Plugin for OzmuxTmuxMousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxMouseGesture>();
        app.add_systems(Update, arbiter.in_set(InputPhase::Dispatch));
    }
}

/// The current phase of a left-button gesture over a tmux pane.
#[derive(Default, Debug, PartialEq)]
enum GestureState {
    /// No button is held; the arbiter is waiting for the next press.
    #[default]
    Idle,
    /// Left button is held; `pane_id` is the pane that received the press and
    /// `origin_phys` is the physical-pixel cursor position at press time.
    Pressed {
        pane_id: PaneId,
        origin_phys: Vec2,
    },
}

/// Tracks the current left-button gesture over a tmux pane.
#[derive(Resource, Default)]
pub(crate) struct TmuxMouseGesture {
    state: GestureState,
}

/// Returns the `PaneId` of the first `TmuxPane` whose `ComputedNode` contains
/// `cursor_phys` (physical px), or `None` when no pane covers the point.
fn pane_under_cursor(
    panes: &Query<(&TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    cursor_phys: Vec2,
) -> Option<PaneId> {
    panes
        .iter()
        .find(|(_, node, transform)| node.contains_point(**transform, cursor_phys))
        .map(|(pane, _, _)| pane.id)
}

/// Interprets raw left-button messages into tmux `select-pane` commands.
///
/// On each `Pressed` event the cursor's physical position is resolved to a
/// pane; if one is found a `select-pane` command is sent and the state
/// transitions to `Pressed`. On `Released` the state returns to `Idle`. When
/// the primary window is not focused any queued events are drained and the
/// state is reset.
fn arbiter(
    mut gesture: ResMut<TmuxMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    connection: NonSend<TmuxConnection>,
    panes: Query<(&TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        buttons.clear();
        gesture.state = GestureState::Idle;
        return;
    };
    if !window.focused {
        buttons.clear();
        gesture.state = GestureState::Idle;
        return;
    }
    let scale = window.scale_factor();
    for ev in buttons.read() {
        if ev.button != bevy::input::mouse::MouseButton::Left {
            continue;
        }
        match ev.state {
            ButtonState::Pressed => {
                let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
                    continue;
                };
                let Some(pane_id) = pane_under_cursor(&panes, cursor_phys) else {
                    continue;
                };
                if let Some(client) = connection.client() {
                    let cmd = select_pane_command(pane_id);
                    if let Err(e) = client.handle().send(&cmd) {
                        tracing::warn!(?e, pane = pane_id.0, "select-pane send failed");
                    }
                }
                gesture.state = GestureState::Pressed {
                    pane_id,
                    origin_phys: cursor_phys,
                };
            }
            ButtonState::Released => {
                gesture.state = GestureState::Idle;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::mouse::MouseButtonInput;

    #[test]
    fn gesture_state_default_is_idle() {
        assert_eq!(GestureState::default(), GestureState::Idle);
    }

    #[test]
    fn left_press_without_cursor_stays_idle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.init_resource::<TmuxMouseGesture>();
        app.add_systems(Update, arbiter);
        app.world_mut()
            .spawn((Window { focused: true, ..default() }, PrimaryWindow));
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: bevy::input::mouse::MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        assert_eq!(app.world().resource::<TmuxMouseGesture>().state, GestureState::Idle);
    }
}
