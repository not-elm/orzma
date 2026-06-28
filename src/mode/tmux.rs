//! tmux feature plugin: aggregates all tmux runtime sub-plugins.

mod adopt;
mod copy_mode;
mod divider_handle;
mod forward;
mod gate;
mod input;
mod locale;
mod mode_ui;
mod mouse;
mod paint_rescue;
mod pane_focus;
pub(crate) mod pane_hit;
mod render;
mod webview_tokens;
mod window_bar;
mod window_bar_input;

use crate::app_mode::AppMode;
use adopt::AdoptPlugin;
use bevy::prelude::*;
use copy_mode::CopyModePlugin;
use divider_handle::DividerHandlePlugin;
use forward::ForwardPlugin;
use gate::GatePlugin;
use input::InputPlugin;
use locale::TmuxLocalePlugin;
use mode_ui::TmuxModeUiPlugin;
use mouse::MousePlugin;
use ozmux_tmux::{TmuxClient, TmuxConnectionClosed, TmuxSessionPlugin};
use paint_rescue::PaintRescuePlugin;
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
            .add_observer(on_tmux_connection_closed)
            .add_plugins((
                TmuxSessionPlugin,
                AdoptPlugin,
                RenderPlugin,
                PaintRescuePlugin,
                InputPlugin,
                MousePlugin,
                ForwardPlugin,
                CopyModePlugin,
                WindowBarPlugin,
                DividerHandlePlugin,
                PaneFocusPlugin,
                GatePlugin,
                WebviewTokensPlugin,
                TmuxLocalePlugin,
                TmuxModeUiPlugin,
            ));
    }
}

/// Sends `detach-client` over the live connection, if any.
///
/// The `%exit` notification tmux emits in response drives the teardown path
/// (see `crate::mode::tmux::adopt`), which closes the connection and returns to
/// `AppMode::Default`. Callers must NOT also set `NextState(Default)` directly:
/// the connection stays live until tmux acknowledges the detach, and the
/// teardown owns the mode transition.
pub(crate) fn request_detach(client: &mut TmuxClient) {
    if let Err(error) = client.send_raw("detach-client") {
        tracing::warn!(?error, "detach-client send failed");
    }
}

fn on_tmux_connection_closed(
    _ev: On<TmuxConnectionClosed>,
    mut next_mode: ResMut<NextState<AppMode>>,
) {
    next_mode.set(AppMode::Default);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;

    #[test]
    fn request_detach_sends_detach_client() {
        let mut client = TmuxClient::new_adopted();
        request_detach(&mut client);
        assert_eq!(client.take_outgoing(), b"detach-client\n");
    }

    #[test]
    fn connection_closed_returns_to_default() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Tmux);
        app.add_observer(on_tmux_connection_closed);
        app.world_mut().trigger(TmuxConnectionClosed);
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Default
        );
    }
}
