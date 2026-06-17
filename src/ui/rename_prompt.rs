//! ozmux-owned rename prompt for tmux's `command-prompt`-wrapped `rename-window`
//! / `rename-session` bindings, which a `-CC` control client cannot render.
//! `forward_keys_to_tmux` detects such a binding and inserts `RenamePrompt`
//! instead of forwarding it; this prompt owns the keyboard, pre-fills the
//! current name, and on submit sends a freshly-rebuilt, safely-quoted rename
//! command. The recognizer (`parse_command_prompt_rename`) is added in the
//! interception task.

use crate::font::TerminalUiFont;
use crate::theme;
use bevy::app::{App, Plugin};
use bevy::ecs::message::MessageReader;
use bevy::ecs::schedule::common_conditions::{
    resource_exists, resource_exists_and_changed, resource_removed,
};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use ozmux_tmux::{
    SessionId, TmuxConnection, WindowId, rename_session_command, rename_window_command,
};

const RENAME_Z: i32 = 340;

/// Registers the rename-prompt input system and the show/hide render systems.
pub(crate) struct RenamePromptPlugin;

impl Plugin for RenamePromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_rename_ui)
            .add_systems(
                Update,
                handle_rename_input
                    .after(crate::input::InputPhase::FocusedKey)
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
}

/// The effect of one key on an open rename prompt.
#[derive(Debug, PartialEq, Eq)]
enum RenameStep {
    Continue,
    Submit,
    Cancel,
}

/// Applies one key to the prompt: Enter submits, Escape cancels, Backspace
/// deletes the last char, a character is appended. Other keys are ignored.
fn apply_rename_key(prompt: &mut RenamePrompt, key: &Key) -> RenameStep {
    match key {
        Key::Escape => RenameStep::Cancel,
        Key::Enter => RenameStep::Submit,
        Key::Backspace => {
            prompt.text.pop();
            RenameStep::Continue
        }
        Key::Character(s) => {
            prompt.text.push_str(s);
            RenameStep::Continue
        }
        _ => RenameStep::Continue,
    }
}

/// Builds the tmux command sent on submit from the prompt's subject and text.
fn submit_command(prompt: &RenamePrompt) -> String {
    match &prompt.subject {
        RenameSubject::Window { id, .. } => rename_window_command(*id, &prompt.text),
        RenameSubject::Session { id, .. } => rename_session_command(*id, &prompt.text),
    }
}

/// The bar's leading label for the subject.
fn rename_label(subject: &RenameSubject) -> &'static str {
    match subject {
        RenameSubject::Window { .. } => "Rename window: ",
        RenameSubject::Session { .. } => "Rename session: ",
    }
}

#[derive(Component)]
struct RenameBar;

fn spawn_rename_ui(mut commands: Commands, ui_font: Option<Res<TerminalUiFont>>) {
    let font = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();
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
                TextFont {
                    font,
                    font_size: theme::UI_FONT_SIZE,
                    ..default()
                },
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
                text.0 = format!("{}{}\u{258f}", rename_label(&prompt.subject), prompt.text);
            }
        }
    }
}

fn handle_rename_input(
    mut commands: Commands,
    mut prompt: ResMut<RenamePrompt>,
    mut events: MessageReader<KeyboardInput>,
    mut armed: Local<bool>,
    connection: NonSend<TmuxConnection>,
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
        match apply_rename_key(&mut prompt, &ev.logical_key) {
            RenameStep::Continue => {}
            RenameStep::Submit => {
                let cmd = submit_command(&prompt);
                if let Some(client) = connection.client()
                    && let Err(e) = client.handle().send(&cmd)
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
        assert_eq!(apply_rename_key(&mut p, &Key::Escape), RenameStep::Cancel);
    }

    #[test]
    fn enter_submits() {
        let mut p = RenamePrompt::new(win_subject());
        assert_eq!(apply_rename_key(&mut p, &Key::Enter), RenameStep::Submit);
    }

    #[test]
    fn char_appends_and_continues() {
        let mut p = RenamePrompt::new(win_subject());
        p.text.clear();
        assert_eq!(apply_rename_key(&mut p, &char_key("a")), RenameStep::Continue);
        assert_eq!(apply_rename_key(&mut p, &char_key("b")), RenameStep::Continue);
        assert_eq!(p.text, "ab");
    }

    #[test]
    fn backspace_pops_last_char() {
        let mut p = RenamePrompt::new(win_subject());
        assert_eq!(apply_rename_key(&mut p, &Key::Backspace), RenameStep::Continue);
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
        assert_eq!(submit_command(&p), "rename-window -t @2 -- 'new name'");
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
        assert_eq!(submit_command(&p), "rename-session -t $1 -- proj");
    }

    #[test]
    fn label_matches_subject() {
        assert_eq!(
            rename_label(&RenameSubject::Window {
                id: WindowId(0),
                current_name: String::new(),
            }),
            "Rename window: "
        );
        assert_eq!(
            rename_label(&RenameSubject::Session {
                id: SessionId(0),
                current_name: String::new(),
            }),
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
            .insert_non_send_resource(TmuxConnection::default())
            .add_systems(
                Update,
                handle_rename_input.run_if(resource_exists::<RenamePrompt>),
            );
        app
    }

    #[test]
    fn open_frame_skips_the_opening_key() {
        let mut app = armed_skip_app();
        app.world_mut().insert_resource(RenamePrompt::new(win_subject()));
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
        app.world_mut().insert_resource(RenamePrompt::new(win_subject()));
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
