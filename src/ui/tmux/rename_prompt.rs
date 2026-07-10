//! orzma-owned rename prompt: the rename-window / rename-session shortcut
//! actions open this prompt. It owns the keyboard, pre-fills the current
//! name, and on submit sends a freshly-rebuilt, safely-quoted rename command.

use crate::font::TerminalUiFont;
use crate::input::InputPhase;
use crate::theme;
use bevy::app::{App, Plugin};
use bevy::ecs::message::MessageReader;
use bevy::ecs::schedule::common_conditions::{
    resource_exists, resource_exists_and_changed, resource_removed,
};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use orzma_tmux::{RenameSession, RenameWindow, SessionId, TmuxClient, TmuxCommand, WindowId};

const RENAME_Z: i32 = 340;

/// Registers the rename-prompt input system and the show/hide render systems.
pub(super) struct RenamePromptPlugin;

impl Plugin for RenamePromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_rename_ui)
            .add_systems(
                Update,
                handle_rename_input
                    .after(InputPhase::FocusedKey)
                    .run_if(resource_exists::<RenamePrompt>),
            )
            .add_systems(
                PostUpdate,
                (
                    hide_rename_ui.run_if(resource_removed::<RenamePrompt>),
                    show_rename_ui.run_if(resource_exists_and_changed::<RenamePrompt>),
                ),
            );
    }
}

/// What is being renamed: the captured target id plus its current name. One
/// enum so an invalid kind/id pairing is unrepresentable.
pub(crate) enum RenameSubject {
    /// A window, targeted by `@id`.
    Window {
        /// tmux window id captured at prompt-open.
        id: WindowId,
        /// The window's name at prompt-open (used to pre-fill the field).
        current_name: String,
    },
    /// A session, targeted by `$id`.
    Session {
        /// tmux session id captured at prompt-open.
        id: SessionId,
        /// The session's name at prompt-open (used to pre-fill the field).
        current_name: String,
    },
}

impl RenameSubject {
    fn current_name(&self) -> &str {
        match self {
            RenameSubject::Window { current_name, .. }
            | RenameSubject::Session { current_name, .. } => current_name,
        }
    }

    /// The prompt bar's leading label for this subject.
    fn label(&self) -> &'static str {
        match self {
            RenameSubject::Window { .. } => "Rename window: ",
            RenameSubject::Session { .. } => "Rename session: ",
        }
    }
}

/// The active rename prompt. Present as a resource only while editing; its
/// existence owns the keyboard like the confirm prompt and the session picker.
#[derive(Resource)]
pub(crate) struct RenamePrompt {
    /// What is being renamed.
    subject: RenameSubject,
    /// The edit buffer, pre-filled with the subject's current name.
    text: String,
}

impl RenamePrompt {
    /// Opens a prompt for `subject`, pre-filling the edit buffer with its
    /// current name.
    pub(crate) fn new(subject: RenameSubject) -> Self {
        let text = subject.current_name().to_string();
        Self { subject, text }
    }

    /// Applies one key: Enter submits, Escape cancels, Backspace deletes the
    /// last char, a character is appended. Other keys are ignored.
    fn apply_key(&mut self, key: &Key) -> RenameStep {
        match key {
            Key::Escape => RenameStep::Cancel,
            Key::Enter => RenameStep::Submit,
            Key::Backspace => {
                self.text.pop();
                RenameStep::Continue
            }
            Key::Character(s) => {
                self.text.push_str(s);
                RenameStep::Continue
            }
            _ => RenameStep::Continue,
        }
    }

    /// Builds the tmux command sent on submit from the subject and typed text.
    fn submit_command(&self) -> String {
        match &self.subject {
            RenameSubject::Window { id, .. } => RenameWindow {
                id: *id,
                name: &self.text,
            }
            .into_raw_command(),
            RenameSubject::Session { id, .. } => RenameSession {
                id: *id,
                name: &self.text,
            }
            .into_raw_command(),
        }
    }
}

/// The effect of one key on an open rename prompt.
#[derive(Debug, PartialEq, Eq)]
enum RenameStep {
    Continue,
    Submit,
    Cancel,
}

#[derive(Component)]
struct RenameBar;

fn spawn_rename_ui(mut commands: Commands, ui_font: Option<Res<TerminalUiFont>>) {
    let ui = ui_font.as_deref().cloned().unwrap_or_default();
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Auto,
                display: Display::None,
                padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(theme::SELECTION),
            GlobalZIndex(RENAME_Z),
            RenameBar,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(""),
                ui.text_font(FontSize::Px(theme::UI_FONT_SIZE)),
                TextColor(theme::SELECTION_FG),
            ));
        });
}

fn hide_rename_ui(mut bar: Query<&mut Node, With<RenameBar>>) {
    if let Ok(mut node) = bar.single_mut() {
        node.display = Display::None;
    }
}

fn show_rename_ui(
    mut bar: Query<&mut Node, With<RenameBar>>,
    mut texts: Query<&mut Text>,
    prompt: Res<RenamePrompt>,
    children_query: Query<&Children, With<RenameBar>>,
) {
    let Ok(mut node) = bar.single_mut() else {
        return;
    };
    node.display = Display::Flex;
    if let Ok(children) = children_query.single() {
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                text.0 = format!("{}{}\u{258f}", prompt.subject.label(), prompt.text);
            }
        }
    }
}

fn handle_rename_input(
    mut commands: Commands,
    mut prompt: ResMut<RenamePrompt>,
    mut events: MessageReader<KeyboardInput>,
    mut armed: Local<bool>,
    mut client: Option<Single<&mut TmuxClient>>,
) {
    // NOTE: the bound key that opened the prompt (`,` / `$`) is still in the
    // shared KeyboardInput buffer; this reader has its own cursor, so skip the
    // open frame — drain past the opening key — or it is appended to the name.
    if !*armed {
        events.clear();
        *armed = true;
        return;
    }
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match prompt.apply_key(&ev.logical_key) {
            RenameStep::Continue => {}
            RenameStep::Submit => {
                let cmd = prompt.submit_command();
                if let Some(client) = client.as_deref_mut()
                    && let Err(e) = client.send_raw(&cmd)
                {
                    tracing::warn!(?e, "rename submit failed");
                }
                commands.remove_resource::<RenamePrompt>();
                *armed = false;
                break;
            }
            RenameStep::Cancel => {
                commands.remove_resource::<RenamePrompt>();
                *armed = false;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::KeyCode;

    fn win_subject() -> RenameSubject {
        RenameSubject::Window {
            id: WindowId(2),
            current_name: "nvim".to_string(),
        }
    }

    fn char_key(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn new_prefills_text_from_current_name() {
        let p = RenamePrompt::new(win_subject());
        assert_eq!(p.text, "nvim");
    }

    #[test]
    fn escape_cancels() {
        let mut p = RenamePrompt::new(win_subject());
        assert_eq!(p.apply_key(&Key::Escape), RenameStep::Cancel);
    }

    #[test]
    fn enter_submits() {
        let mut p = RenamePrompt::new(win_subject());
        assert_eq!(p.apply_key(&Key::Enter), RenameStep::Submit);
    }

    #[test]
    fn char_appends_and_continues() {
        let mut p = RenamePrompt::new(win_subject());
        p.text.clear();
        assert_eq!(p.apply_key(&char_key("a")), RenameStep::Continue);
        assert_eq!(p.apply_key(&char_key("b")), RenameStep::Continue);
        assert_eq!(p.text, "ab");
    }

    #[test]
    fn backspace_pops_last_char() {
        let mut p = RenamePrompt::new(win_subject());
        assert_eq!(p.apply_key(&Key::Backspace), RenameStep::Continue);
        assert_eq!(p.text, "nvi");
    }

    #[test]
    fn submit_command_uses_window_builder() {
        let p = RenamePrompt {
            subject: RenameSubject::Window {
                id: WindowId(2),
                current_name: "old".to_string(),
            },
            text: "new name".to_string(),
        };
        assert_eq!(p.submit_command(), "rename-window -t @2 -- 'new name'");
    }

    #[test]
    fn submit_command_uses_session_builder() {
        let p = RenamePrompt {
            subject: RenameSubject::Session {
                id: SessionId(1),
                current_name: "old".to_string(),
            },
            text: "proj".to_string(),
        };
        assert_eq!(p.submit_command(), "rename-session -t $1 -- proj");
    }

    #[test]
    fn label_matches_subject() {
        assert_eq!(
            RenameSubject::Window {
                id: WindowId(0),
                current_name: String::new(),
            }
            .label(),
            "Rename window: "
        );
        assert_eq!(
            RenameSubject::Session {
                id: SessionId(0),
                current_name: String::new(),
            }
            .label(),
            "Rename session: "
        );
    }

    fn key_event(logical: Key, code: KeyCode) -> KeyboardInput {
        KeyboardInput {
            key_code: code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    fn armed_skip_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .add_systems(
                Update,
                handle_rename_input.run_if(resource_exists::<RenamePrompt>),
            );
        app
    }

    #[test]
    fn open_frame_skips_the_opening_key() {
        let mut app = armed_skip_app();
        app.world_mut()
            .insert_resource(RenamePrompt::new(win_subject()));
        // The opening `,` is still in the shared buffer when the prompt opens.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(Key::Character(",".into()), KeyCode::Comma));
        app.update();

        let p = app.world().resource::<RenamePrompt>();
        assert_eq!(
            p.text, "nvim",
            "the opening key must not leak into the prefilled text"
        );
    }

    #[test]
    fn escape_after_open_frame_removes_resource() {
        let mut app = armed_skip_app();
        app.world_mut()
            .insert_resource(RenamePrompt::new(win_subject()));
        // Open frame drains the opening key.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(Key::Character(",".into()), KeyCode::Comma));
        app.update();
        assert!(app.world().get_resource::<RenamePrompt>().is_some());

        // Next frame: Escape cancels → resource removed.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(Key::Escape, KeyCode::Escape));
        app.update();
        assert!(
            app.world().get_resource::<RenamePrompt>().is_none(),
            "Escape must close the prompt"
        );
    }
}
