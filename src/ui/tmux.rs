//! tmux-mode UI: the mode container, window bar, prompts, divider handles,
//! and pane focus styling.

pub(crate) mod confirm_prompt;
pub(crate) mod mode_ui;
pub(crate) mod rename_prompt;
pub(crate) mod window_bar;

mod divider_handle;
mod pane_focus;

use bevy::prelude::*;
use confirm_prompt::ConfirmPromptPlugin;
use divider_handle::DividerHandlePlugin;
use mode_ui::TmuxModeUiPlugin;
use pane_focus::PaneFocusPlugin;
use rename_prompt::RenamePromptPlugin;
use window_bar::WindowBarPlugin;

/// Bevy plugin aggregating the tmux-mode UI sub-plugins.
pub(crate) struct TmuxUiPlugin;

impl Plugin for TmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            TmuxModeUiPlugin,
            WindowBarPlugin,
            ConfirmPromptPlugin,
            RenamePromptPlugin,
            DividerHandlePlugin,
            PaneFocusPlugin,
        ));
    }
}
