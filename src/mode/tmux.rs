//! tmux feature plugin: aggregates all tmux runtime sub-plugins.

use crate::input::tmux::forward::ForwardPlugin;
use crate::input::tmux::gate::GatePlugin;
use crate::input::tmux::input::InputPlugin;
use crate::input::tmux::mouse::MousePlugin;
use crate::input::tmux::window_bar_input::WindowBarInputPlugin;
use bevy::prelude::*;

/// Bevy plugin aggregating all tmux runtime sub-plugins.
pub struct OzmuxTmuxPlugin;

impl Plugin for OzmuxTmuxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            InputPlugin,
            MousePlugin,
            ForwardPlugin,
            WindowBarInputPlugin,
            GatePlugin,
        ));
    }
}
