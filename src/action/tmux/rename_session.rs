//! `RenameSessionRequest` — opens the orzma rename prompt pre-filled with the
//! target session's current name.

use crate::font::TerminalUiFont;
use crate::theme;
use crate::ui::text_prompt::{ActiveTextPrompt, TextPromptSpec, spawn_text_prompt};
use crate::ui::tmux::rename_prompt::{RenameIntent, RenameSubject};
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use orzma_tmux::TmuxSession;

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
    mut input_focus: ResMut<InputFocus>,
    mut active: ResMut<ActiveTextPrompt>,
    sessions: Query<&TmuxSession>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let Ok(session) = sessions.get(ev.entity) else {
        return;
    };
    let subject = RenameSubject::Session {
        id: session.id,
        current_name: session.name.clone(),
    };
    let font = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();
    let editable = spawn_text_prompt(
        &mut commands,
        &mut input_focus,
        &mut active,
        font,
        TextPromptSpec {
            label: subject.label().to_string(),
            initial: subject.current_name().to_string(),
            submit_on_first_char: false,
            select_all: true,
            bg: theme::SELECTION,
            fg: theme::SELECTION_FG,
        },
    );
    commands.entity(editable).insert(RenameIntent(subject));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text_prompt::TextPrompt;
    use tmux_control_parser::SessionId;

    #[test]
    fn rename_session_opens_prompt() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputFocus>()
            .init_resource::<ActiveTextPrompt>();
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
        assert!(
            app.world().resource::<ActiveTextPrompt>().0.is_some(),
            "opening a rename prompt must set ActiveTextPrompt"
        );
        let editable = app.world().resource::<ActiveTextPrompt>().0.unwrap();
        assert!(app.world().get::<RenameIntent>(editable).is_some());
        assert!(app.world().get::<TextPrompt>(editable).is_some());
    }
}
