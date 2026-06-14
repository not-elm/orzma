//! Boot-time tmux auto-connect: queries sessions, picks a target, opens a
//! control-mode connection, and drives `ConnectionState`.

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use ozmux_tmux::{ConnectionState, TmuxConnection, attach_or_create, select_attach_target};
use tmux_control::TmuxServer;

/// Registers the `Startup` auto-connect system.
pub(crate) struct TmuxBootPlugin;

impl Plugin for TmuxBootPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, auto_connect_tmux);
    }
}

fn auto_connect_tmux(
    mut state: ResMut<ConnectionState>,
    mut connection: NonSendMut<TmuxConnection>,
    configs: Res<OzmuxConfigsResource>,
) {
    let cfg = &configs.tmux;
    let mut server = TmuxServer::new().program(&cfg.program);
    if let Some(name) = &cfg.socket_name {
        server = server.socket_name(name);
    }
    let sessions = match server.list_sessions() {
        Ok(sessions) => sessions,
        Err(e) => {
            *state = ConnectionState::Error {
                reason: format!("tmux unavailable: {e}"),
            };
            return;
        }
    };
    match attach_or_create(&server, &select_attach_target(&sessions)) {
        Ok(client) => {
            connection.set(client);
            *state = ConnectionState::Connecting;
        }
        Err(e) => {
            *state = ConnectionState::Error {
                reason: format!("tmux connect failed: {e}"),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxSessionPlugin;

    #[test]
    #[ignore = "requires a real tmux binary and a controlling PTY"]
    fn boot_attempts_connection_and_leaves_idle_state() {
        let mut app = App::new();
        app.add_plugins((TmuxSessionPlugin, TmuxBootPlugin));
        app.insert_resource(OzmuxConfigsResource::default());
        app.update();
        assert_ne!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Idle,
            "boot always attempts a connection, so it must leave Idle",
        );
    }
}
