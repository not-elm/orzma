//! `NextSessionRequest` — switches the tmux client to its next session.

use bevy::prelude::*;
use orzma_tmux::{SwitchClientNext, TmuxClient, TmuxSession};

/// Switches the tmux session owning `entity` to the next session.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct NextSessionRequest {
    /// The session entity whose client cycles to the next session.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `NextSessionRequest` apply observer.
pub(super) struct NextSessionPlugin;

impl Plugin for NextSessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_next_session);
    }
}

fn on_next_session(
    ev: On<NextSessionRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    sessions: Query<&TmuxSession>,
) {
    if sessions.get(ev.entity).is_err() {
        return;
    }
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(SwitchClientNext) {
        tracing::warn!(?e, "next-session send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::SessionId;

    #[test]
    fn next_session_sends_switch_client_next() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_next_session);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(4),
                name: "main".into(),
            })
            .id();
        app.world_mut()
            .trigger(NextSessionRequest { entity: session });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("switch-client -n"), "got {out:?}");
    }
}
