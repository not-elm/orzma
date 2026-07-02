//! `NewWindowRequest` — opens a new tmux window in the client's current
//! session.

use bevy::prelude::*;
use ozmux_tmux::{NewWindow, TmuxClient, TmuxSession};

/// Opens a new window in the client's current session, starting in the active
/// pane's current directory. The `new-window` deliberately carries no `-t` (its
/// `-t` selects an insertion position, not just a session) and inherits the cwd
/// via `-c`; `entity` only gates the request on a projected session still
/// existing.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct NewWindowRequest {
    /// The projected session entity used as an existence guard.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `NewWindowRequest` apply observer.
pub(super) struct NewWindowPlugin;

impl Plugin for NewWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_new_window);
    }
}

fn on_new_window(
    ev: On<NewWindowRequest>,
    mut client: Option<Single<&mut TmuxClient>>,
    sessions: Query<&TmuxSession>,
) {
    if sessions.get(ev.entity).is_err() {
        return;
    }
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(NewWindow) {
        tracing::warn!(?e, "new-window send failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::SessionId;

    #[test]
    fn new_window_sends_new_window_with_cwd() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_new_window);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(0),
                name: "main".into(),
            })
            .id();
        app.world_mut()
            .trigger(NewWindowRequest { entity: session });
        app.update();
        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert!(
            out.contains("new-window -c \"#{pane_current_path}\""),
            "got {out:?}"
        );
    }
}
