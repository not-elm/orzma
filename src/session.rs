//! Session lifecycle of the mux-backed shell surface: pane spawn
//! requests, window geometry, active-pane focus mirroring, and
//! exit-on-session-end.

pub(crate) mod spawn;

mod exit;
mod focus;
mod layout;

use bevy::prelude::*;

/// Bevy plugin for the shell session lifecycle (spawn / layout / focus /
/// exit).
pub(crate) struct SessionPlugin;

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            spawn::SpawnPlugin,
            exit::ExitPlugin,
            focus::FocusPlugin,
            layout::LayoutPlugin,
        ));
    }
}
