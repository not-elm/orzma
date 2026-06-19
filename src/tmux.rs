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

use crate::ozma::AppMode;
use bevy::prelude::*;
use copy_mode::CopyModePlugin;
use dialog::DialogPlugin;
use divider_handle::DividerHandlePlugin;
use input::InputPlugin;
use mouse::MousePlugin;
use ozmux_tmux::{
    TmuxConnection, TmuxConnectionClosed, TmuxConnectionReset, TmuxPresence, TmuxSessionPlugin,
};
use pane_focus::PaneFocusPlugin;
use render::RenderPlugin;
use window_bar::WindowBarPlugin;

/// SystemSet applied to every tmux Update system. Runs only in `AppMode::Ozmux`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct OzmuxActiveSet;

/// Bevy plugin aggregating all tmux runtime sub-plugins.
pub struct OzmuxTmuxPlugin;

impl Plugin for OzmuxTmuxPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, OzmuxActiveSet.run_if(in_state(AppMode::Ozmux)))
            .add_systems(OnEnter(AppMode::Ozmux), on_enter_ozmux)
            .add_systems(OnExit(AppMode::Ozmux), on_exit_ozmux)
            .add_observer(on_tmux_connection_closed)
            .add_plugins((
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

fn on_tmux_connection_closed(
    _ev: On<TmuxConnectionClosed>,
    mut next_mode: ResMut<NextState<AppMode>>,
) {
    next_mode.set(AppMode::Ozma);
}

fn on_enter_ozmux(mut commands: Commands) {
    commands.insert_resource(TmuxPresence);
}

fn on_exit_ozmux(mut commands: Commands, mut connection: NonSendMut<TmuxConnection>) {
    if let Some(client) = connection.client() {
        let _ = client.handle().send("detach-client");
    }
    connection.take();
    commands.remove_resource::<TmuxPresence>();
    commands.trigger(TmuxConnectionReset);
}
