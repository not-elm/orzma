//! tmux feature plugin: aggregates all tmux runtime sub-plugins.

mod copy_mode;
mod dialog;
mod divider_handle;
mod input;
mod mouse;
mod pane_focus;
pub(crate) mod pane_hit;
mod render;
mod window_bar;
mod window_bar_input;

use bevy::prelude::*;
use copy_mode::CopyModePlugin;
use dialog::DialogPlugin;
use divider_handle::DividerHandlePlugin;
use input::InputPlugin;
use mouse::MousePlugin;
use ozmux_tmux::TmuxSessionPlugin;
use pane_focus::PaneFocusPlugin;
use render::RenderPlugin;
use window_bar::WindowBarPlugin;

/// Bevy plugin aggregating all tmux runtime sub-plugins.
pub struct OzmuxTmuxPlugin;

impl Plugin for OzmuxTmuxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            TmuxSessionPlugin,
            RenderPlugin,
            InputPlugin,
            MousePlugin,
            CopyModePlugin,
            WindowBarPlugin,
            DialogPlugin,
            DividerHandlePlugin,
            PaneFocusPlugin,
        ));
    }
}
