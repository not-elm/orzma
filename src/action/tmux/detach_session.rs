//! `DetachSessionRequest` — triggers `detach-client` on the target session.

use crate::session::tmux::request_detach;
use bevy::prelude::*;
use orzma_tmux::{TmuxClient, TmuxSession};

/// Detaches the tmux client owning `entity`'s session.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct DetachSessionRequest {
    /// The session entity to detach.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `DetachSessionRequest` apply observer.
pub(super) struct DetachSessionPlugin;

impl Plugin for DetachSessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_detach_session);
    }
}

fn on_detach_session(
    ev: On<DetachSessionRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    sessions: Query<&TmuxSession>,
) {
    if sessions.get(ev.entity).is_err() {
        return;
    }
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    request_detach(client);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::SessionId;

    #[test]
    fn detach_request_without_client_is_noop() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(DetachSessionPlugin);
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(4),
                name: "main".into(),
            })
            .id();
        app.world_mut()
            .trigger(DetachSessionRequest { entity: session });
        app.update();
    }

    #[test]
    fn detach_request_sends_detach_client() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(DetachSessionPlugin);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(4),
                name: "main".into(),
            })
            .id();
        app.world_mut()
            .trigger(DetachSessionRequest { entity: session });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert_eq!(out, "detach-client\n");
    }
}
