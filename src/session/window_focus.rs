//! Window focus: forwards the primary window's focus changes to the
//! multiplexer backend.

use crate::system_set::OrzmaSystems;
use bevy::app::{App, Plugin, Update};
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::With;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::schedule::common_conditions::{on_message, resource_exists};
use bevy::ecs::system::{Query, Res};
use bevy::window::{PrimaryWindow, WindowFocused};
use bevy_orzmux::prelude::OrzmuxConnection;
use orzmux::prelude::OrzmuxCommand;

/// Adds the window-focus sender.
pub(super) struct WindowFocusPlugin;

impl Plugin for WindowFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<WindowFocused>().add_systems(
            Update,
            send_window_focus
                // NOTE: this must stay after the input systems so that a click
                // selecting a pane in the frame where the window regains focus
                // reaches the backend first; otherwise the previously active
                // pane receives a spurious `CSI I` followed by `CSI O`.
                .after(OrzmaSystems::Input)
                .run_if(resource_exists::<OrzmuxConnection>)
                .run_if(on_message::<WindowFocused>),
        );
    }
}

fn send_window_focus(
    mut focus_changes: MessageReader<WindowFocused>,
    connection: Res<OrzmuxConnection>,
    primary_windows: Query<(), With<PrimaryWindow>>,
) {
    for change in focus_changes.read() {
        if primary_windows.contains(change.window) {
            connection.0.send(OrzmuxCommand::WindowFocus {
                focused: change.focused,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::MinimalPlugins;
    use bevy::window::Window;
    use orzmux::prelude::OrzmuxClient;

    /// Asserts that every focus change of the primary window is forwarded
    /// in order, and that another window's changes are not.
    ///
    /// Case: the user switches away from orzma and back within one frame
    /// while a second window exists.
    #[test]
    fn primary_window_focus_changes_are_forwarded_in_order() {
        let (client, _events, commands) = OrzmuxClient::detached();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(WindowFocusPlugin)
            .insert_resource(OrzmuxConnection(client));
        let primary = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        let other = app.world_mut().spawn(Window::default()).id();
        app.world_mut().write_message(WindowFocused {
            window: primary,
            focused: false,
        });
        app.world_mut().write_message(WindowFocused {
            window: other,
            focused: true,
        });
        app.world_mut().write_message(WindowFocused {
            window: primary,
            focused: true,
        });
        app.update();
        let sent: Vec<OrzmuxCommand> = commands.try_iter().map(|(_, c)| c).collect();
        assert!(matches!(
            sent.as_slice(),
            [
                OrzmuxCommand::WindowFocus { focused: false },
                OrzmuxCommand::WindowFocus { focused: true }
            ]
        ));
    }
}
