//! tmux connection lifecycle: adoption of a `tmux -CC` gateway, locale and
//! webview-token propagation on the attach edge, and detach/teardown.

mod adopt;
mod locale;
mod webview_tokens;

use crate::app_mode::AppMode;
use crate::configs::OzmuxConfigsResource;
use adopt::AdoptPlugin;
use bevy::prelude::*;
use locale::TmuxLocalePlugin;
use ozmux_tmux::{HistorySeedLines, TmuxClient, TmuxConnectionClosed, TmuxSessionPlugin};
use webview_tokens::WebviewTokensPlugin;

/// Bevy plugin aggregating the tmux connection-lifecycle sub-plugins.
pub(crate) struct TmuxLifecyclePlugin;

impl Plugin for TmuxLifecyclePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_tmux_connection_closed)
            .add_systems(Startup, sync_history_seed_lines)
            .add_plugins((
                TmuxSessionPlugin,
                AdoptPlugin,
                TmuxLocalePlugin,
                WebviewTokensPlugin,
            ));
    }
}

/// Copies `[scrollback] seed-lines` into [`HistorySeedLines`] once at startup,
/// so `ozmux_tmux`'s attach-time history capture uses the configured depth
/// instead of its resource default (the engine's max cap). Safe regardless of
/// `add_plugins` order: `OzmuxConfigsResource` is inserted at `Plugin::build`
/// time and every `Startup` system runs only after all plugins finish build.
fn sync_history_seed_lines(
    mut seed_lines: ResMut<HistorySeedLines>,
    configs: Res<OzmuxConfigsResource>,
) {
    seed_lines.0 = configs.0.scrollback.seed_lines;
}

/// Sends `detach-client` over the live connection, if any.
///
/// The `%exit` notification tmux emits in response drives the teardown path
/// (see `crate::session::tmux::adopt`), which closes the connection and returns to
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
