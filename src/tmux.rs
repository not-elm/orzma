//! tmux feature plugin: aggregates all tmux runtime sub-plugins.

mod adopt;
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
use adopt::AdoptPlugin;
use bevy::prelude::*;
use copy_mode::CopyModePlugin;
use dialog::DialogPlugin;
use divider_handle::DividerHandlePlugin;
use forward::ForwardPlugin;
use gate::GatePlugin;
use input::InputPlugin;
use mode_ui::TmuxModeUiPlugin;
use mouse::MousePlugin;
use ozmux_tmux::{TmuxConnection, TmuxConnectionClosed, TmuxSessionPlugin};
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

/// Sends `detach-client` over the live connection, if any.
///
/// The `%exit` notification tmux emits in response drives the teardown path
/// (see `crate::tmux::adopt`), which closes the connection and returns to
/// `AppMode::Default`. Callers must NOT also set `NextState(Default)` directly:
/// the connection stays live until tmux acknowledges the detach, and the
/// teardown owns the mode transition.
pub(crate) fn request_detach(connection: &TmuxConnection) {
    if let Some(handle) = connection.handle()
        && let Err(error) = handle.send_raw("detach-client")
    {
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
    use ozma_tty_engine::AdoptedControlMode;
    use ozmux_tmux::TmuxPresence;

    #[test]
    fn exit_tmux_view_does_not_close_connection() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Tmux);
        app.insert_non_send_resource(TmuxConnection::default());
        app.insert_resource(TmuxPresence);

        let gateway = app.world_mut().spawn(AdoptedControlMode::default()).id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Default);
        app.update();

        assert!(
            app.world()
                .non_send_resource::<TmuxConnection>()
                .is_connected(),
            "view-hide (Tmux -> Default) must not close the connection"
        );
        assert!(
            app.world().get_resource::<TmuxPresence>().is_some(),
            "TmuxPresence must persist across a view-hide"
        );
    }

    #[test]
    fn request_detach_sends_detach_client() {
        let mut conn = TmuxConnection::default();
        let gateway = Entity::from_raw_u32(7).expect("entity id");
        conn.adopt(gateway);
        request_detach(&conn);
        assert_eq!(conn.take_outgoing(), b"detach-client\n");
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
