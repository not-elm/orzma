//! `RenameWindowRequest` — opens the orzma rename prompt pre-filled with the
//! target window's current name.

use crate::font::TerminalUiFont;
use crate::theme;
use crate::ui::text_prompt::{ActiveTextPrompt, TextPromptSpec, spawn_text_prompt};
use crate::ui::tmux::rename_prompt::{RenameIntent, RenameSubject};
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use orzma_tmux::TmuxWindow;

/// Opens the rename prompt for the tmux window owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct RenameWindowRequest {
    /// The window entity to rename.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `RenameWindowRequest` apply observer.
pub(super) struct RenameWindowPlugin;

impl Plugin for RenameWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_rename_window);
    }
}

fn on_rename_window(
    ev: On<RenameWindowRequest>,
    mut commands: Commands,
    mut input_focus: ResMut<InputFocus>,
    mut active: ResMut<ActiveTextPrompt>,
    windows: Query<&TmuxWindow>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let Ok(window) = windows.get(ev.entity) else {
        return;
    };
    let subject = RenameSubject::Window {
        id: window.id,
        current_name: window.name.clone(),
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
    use tmux_control_parser::WindowId;

    #[test]
    fn rename_window_opens_prompt() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<InputFocus>()
            .init_resource::<ActiveTextPrompt>();
        app.add_observer(on_rename_window);
        let target = app
            .world_mut()
            .spawn(TmuxWindow {
                id: WindowId(2),
                index: 0,
                name: "editor".into(),
            })
            .id();
        app.world_mut()
            .trigger(RenameWindowRequest { entity: target });
        app.update();
        let editable = app
            .world()
            .resource::<ActiveTextPrompt>()
            .0
            .expect("prompt opened");
        assert!(app.world().get::<RenameIntent>(editable).is_some());
        assert!(app.world().get::<TextPrompt>(editable).is_some());
    }
}
