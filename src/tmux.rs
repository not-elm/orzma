//! tmux feature plugin: aggregates all tmux runtime sub-plugins.

mod copy_mode;
mod dialog;
mod divider_handle;
mod forward;
mod gate;
mod input;
mod mode_ui;
mod mouse;
mod pane_focus;
pub(crate) mod pane_hit;
mod render;
mod webview_tokens;
mod window_bar;
mod window_bar_input;

use crate::app_mode::AppMode;
use bevy::prelude::*;
use copy_mode::CopyModePlugin;
use dialog::DialogPlugin;
use divider_handle::DividerHandlePlugin;
use forward::ForwardPlugin;
use gate::GatePlugin;
use input::InputPlugin;
use mode_ui::TmuxModeUiPlugin;
use mouse::MousePlugin;
use ozmux_tmux::{
    TmuxConnection, TmuxConnectionClosed, TmuxConnectionReset, TmuxPresence, TmuxSessionPlugin,
};
use pane_focus::PaneFocusPlugin;
use render::RenderPlugin;
use webview_tokens::WebviewTokensPlugin;
use window_bar::WindowBarPlugin;

/// SystemSet applied to every tmux Update system. Runs only in `AppMode::Tmux`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TmuxActiveSet;

/// Bevy plugin aggregating all tmux runtime sub-plugins.
pub struct OzmuxTmuxPlugin;

impl Plugin for OzmuxTmuxPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, TmuxActiveSet.run_if(in_state(AppMode::Tmux)))
            .add_systems(OnEnter(AppMode::Tmux), on_enter_tmux)
            .add_systems(OnExit(AppMode::Tmux), on_exit_tmux)
            .add_observer(on_tmux_connection_closed)
            .add_plugins((
                TmuxSessionPlugin,
                RenderPlugin,
                InputPlugin,
                MousePlugin,
                ForwardPlugin,
                CopyModePlugin,
                WindowBarPlugin,
                DialogPlugin,
                DividerHandlePlugin,
                PaneFocusPlugin,
                GatePlugin,
                WebviewTokensPlugin,
                TmuxModeUiPlugin,
            ));
    }
}

fn on_tmux_connection_closed(
    _ev: On<TmuxConnectionClosed>,
    mut next_mode: ResMut<NextState<AppMode>>,
) {
    next_mode.set(AppMode::Default);
}

fn on_enter_tmux(mut commands: Commands) {
    commands.insert_resource(TmuxPresence);
}

fn on_exit_tmux(mut commands: Commands, mut connection: NonSendMut<TmuxConnection>) {
    if let Some(handle) = connection.handle() {
        let _ = handle.send_raw("detach-client");
    }
    connection.close();
    commands.remove_resource::<TmuxPresence>();
    commands.trigger(TmuxConnectionReset);
}
