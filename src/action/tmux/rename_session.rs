//! `RenameSessionRequest` — opens the ozmux rename prompt pre-filled with the
//! target session's current name.

use crate::ui::tmux::rename_prompt::{RenamePrompt, RenameSubject};
use bevy::prelude::*;
use ozmux_tmux::TmuxSession;

/// Opens the rename prompt for the tmux session owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct RenameSessionRequest {
    /// The session entity to rename.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `RenameSessionRequest` apply observer.
pub(super) struct RenameSessionPlugin;

impl Plugin for RenameSessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_rename_session);
    }
}

fn on_rename_session(
    ev: On<RenameSessionRequest>,
    mut commands: Commands,
    sessions: Query<&TmuxSession>,
) {
    let Ok(session) = sessions.get(ev.entity) else {
        return;
    };
    commands.insert_resource(RenamePrompt::new(RenameSubject::Session {
        id: session.id,
        current_name: session.name.clone(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::SessionId;

    #[test]
    fn rename_session_opens_prompt() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_rename_session);
        let target = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(0),
                name: "main".into(),
            })
            .id();
        app.world_mut()
            .trigger(RenameSessionRequest { entity: target });
        app.update();
        assert!(app.world().contains_resource::<RenamePrompt>());
    }
}
