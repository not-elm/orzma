//! `NextWindowRequest` — switches the target session to its next window.

use bevy::prelude::*;
use orzma_tmux::{NextWindow, TmuxClient, TmuxSession};

/// Switches the tmux session owning `entity` to its next window.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct NextWindowRequest {
    /// The session entity to cycle.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `NextWindowRequest` apply observer.
pub(super) struct NextWindowPlugin;

impl Plugin for NextWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_next_window);
    }
}

fn on_next_window(
    ev: On<NextWindowRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    sessions: Query<&TmuxSession>,
) {
    let Ok(session) = sessions.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(NextWindow {
        session: session.id,
    }) {
        tracing::warn!(?e, "next-window send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::SessionId;

    #[test]
    fn next_window_targets_session_id() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_next_window);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(4),
                name: "main".into(),
            })
            .id();
        app.world_mut()
            .trigger(NextWindowRequest { entity: session });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(out.contains("next-window -t $4"), "got {out:?}");
    }
}
