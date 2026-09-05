//! Session lifecycle of the mux-backed shell surface: pane spawn
//! requests, window geometry, and exit-on-session-end. Active-pane focus
//! is mirrored by `crate::input::focus`, not here.

pub(crate) mod spawn;

mod exit;
mod layout;

use bevy::prelude::*;

/// Bevy plugin for the shell session lifecycle (spawn / layout / exit).
pub(crate) struct SessionPlugin;

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((spawn::SpawnPlugin, exit::ExitPlugin, layout::LayoutPlugin));
    }
}
