//! `PreviousSessionRequest` — switches the tmux client to its previous session.

use bevy::prelude::*;
use orzma_tmux::{SwitchClientPrevious, TmuxClient, TmuxSession};

/// Switches the tmux session owning `entity` to the previous session.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct PreviousSessionRequest {
    /// The session entity whose client cycles to the previous session.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `PreviousSessionRequest` apply observer.
pub(super) struct PreviousSessionPlugin;

impl Plugin for PreviousSessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_previous_session);
    }
}

fn on_previous_session(
    ev: On<PreviousSessionRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    sessions: Query<&TmuxSession>,
) {
    if sessions.get(ev.entity).is_err() {
        return;
    }
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(SwitchClientPrevious) {
        tracing::warn!(?e, "previous-session send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::SessionId;

    #[test]
    fn previous_session_sends_switch_client_previous() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_previous_session);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(4),
                name: "main".into(),
            })
            .id();
        app.world_mut()
            .trigger(PreviousSessionRequest { entity: session });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("switch-client -p"), "got {out:?}");
    }
}
