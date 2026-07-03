//! tmux feature plugin: aggregates all tmux runtime sub-plugins.

pub(crate) mod confirm_prompt;
pub(crate) mod copy_mode;
mod divider_handle;
mod mode_ui;
mod paint_rescue;
mod pane_focus;
pub(crate) mod rename_prompt;
pub(crate) mod render;
pub(crate) mod window_bar;

use crate::input::tmux::forward::ForwardPlugin;
use crate::input::tmux::gate::GatePlugin;
use crate::input::tmux::input::InputPlugin;
use crate::input::tmux::mouse::MousePlugin;
use crate::input::tmux::window_bar_input::WindowBarInputPlugin;
use bevy::prelude::*;
use confirm_prompt::ConfirmPromptPlugin;
use copy_mode::CopyModePlugin;
use divider_handle::DividerHandlePlugin;
use mode_ui::TmuxModeUiPlugin;
use paint_rescue::PaintRescuePlugin;
use pane_focus::PaneFocusPlugin;
use rename_prompt::RenamePromptPlugin;
use render::RenderPlugin;
use window_bar::WindowBarPlugin;

/// Bevy plugin aggregating all tmux runtime sub-plugins.
pub struct OzmuxTmuxPlugin;

impl Plugin for OzmuxTmuxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            RenderPlugin,
            PaintRescuePlugin,
            InputPlugin,
            MousePlugin,
            ForwardPlugin,
            CopyModePlugin,
            WindowBarPlugin,
            WindowBarInputPlugin,
            DividerHandlePlugin,
            PaneFocusPlugin,
            GatePlugin,
        ))
        .add_plugins((TmuxModeUiPlugin, ConfirmPromptPlugin, RenamePromptPlugin));
    }
}
