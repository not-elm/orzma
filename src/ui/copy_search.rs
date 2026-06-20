//! Copy-mode search and jump-char prompt overlay. Opens when `forward_keys_to_tmux`
//! dispatches a `CopyAction::Prompt`; owns the keyboard until the user submits
//! (Enter / first char for jump kinds) or cancels (Escape). On submit, sends
//! `send-keys -X -t %N <kind> -- '<text>'` to tmux via the active connection.

use crate::font::TerminalUiFont;
use crate::theme;
use bevy::app::{App, Plugin};
use bevy::ecs::message::MessageReader;
use bevy::ecs::schedule::common_conditions::resource_exists_and_changed;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use ozmux_tmux::{PaneId, Prompt, PromptKind, TmuxCommand, TmuxConnection};

const PROMPT_Z: i32 = 320;

/// Registers the copy-mode prompt resource, input system, and render system.
pub(crate) struct CopyPromptPlugin;

impl Plugin for CopyPromptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CopyPrompt>()
            .add_systems(Startup, spawn_prompt_ui)
            .add_systems(
                Update,
                handle_prompt_input
                    .after(crate::input::InputPhase::FocusedKey)
                    .run_if(|p: Res<CopyPrompt>| p.open.is_some()),
            )
            .add_systems(
                PostUpdate,
                sync_prompt_ui.run_if(resource_exists_and_changed::<CopyPrompt>),
            );
    }
}

/// The active copy-mode prompt (search regex or jump char). Present while the
/// user is typing; owns the keyboard like the session picker.
#[derive(Resource, Default)]
pub(crate) struct CopyPrompt {
    /// The pending prompt, if open.
    pub(crate) open: Option<CopyPromptState>,
}

/// In-progress copy-mode prompt input.
pub(crate) struct CopyPromptState {
    /// Which copy command to run on submit.
    pub(crate) kind: PromptKind,
    /// The pane the result targets.
    pub(crate) pane: PaneId,
    /// Text typed so far.
    pub(crate) text: String,
}

/// The effect of one key on an open prompt.
#[derive(Debug, PartialEq)]
enum PromptStep {
    Continue,
    Submit,
    Cancel,
}

/// Applies one key press to the prompt text, returning what to do. Jump prompts
/// (single-char) submit on the first typed character; search prompts submit on
/// Enter. Escape cancels; Backspace edits.
fn apply_prompt_key(state: &mut CopyPromptState, key: &Key) -> PromptStep {
    match key {
        Key::Escape => PromptStep::Cancel,
        Key::Enter => PromptStep::Submit,
        Key::Backspace => {
            state.text.pop();
            PromptStep::Continue
        }
        Key::Character(s) => {
            state.text.push_str(s);
            if state.kind.is_single_char() {
                PromptStep::Submit
            } else {
                PromptStep::Continue
            }
        }
        _ => PromptStep::Continue,
    }
}

/// Returns the prompt label character (shown before the typed text).
fn prompt_label(kind: PromptKind) -> &'static str {
    match kind {
        PromptKind::SearchForward => "/",
        PromptKind::SearchBackward => "?",
        PromptKind::JumpForward => "f",
        PromptKind::JumpBackward => "F",
        PromptKind::JumpToForward => "t",
        PromptKind::JumpToBackward => "T",
    }
}

#[derive(Component)]
struct PromptBar;

fn spawn_prompt_ui(mut commands: Commands, ui_font: Option<Res<TerminalUiFont>>) {
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
                padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(theme::PANEL),
            GlobalZIndex(PROMPT_Z),
            PromptBar,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(""),
                TextFont {
                    font,
                    font_size: theme::UI_FONT_SIZE,
                    ..default()
                },
                TextColor(theme::FOREGROUND),
            ));
        });
}

fn sync_prompt_ui(
    mut bar: Query<&mut Node, With<PromptBar>>,
    mut texts: Query<&mut Text>,
    prompt: Res<CopyPrompt>,
    children_query: Query<&Children, With<PromptBar>>,
) {
    let Ok(mut node) = bar.single_mut() else {
        return;
    };
    match &prompt.open {
        None => {
            node.display = Display::None;
        }
        Some(state) => {
            node.display = Display::Flex;
            if let Ok(children) = children_query.single() {
                for child in children.iter() {
                    if let Ok(mut text) = texts.get_mut(child) {
                        text.0 = format!("{}{}", prompt_label(state.kind), state.text);
                    }
                }
            }
        }
    }
}

fn handle_prompt_input(
    mut copy_prompt: ResMut<CopyPrompt>,
    mut events: MessageReader<KeyboardInput>,
    mut armed: Local<bool>,
    connection: NonSend<TmuxConnection>,
) {
    // NOTE: the keystroke that opened the prompt (e.g. `/`, or `f` for a jump)
    // is still in the shared KeyboardInput buffer; each reader has its own
    // cursor, so `forward_keys_to_tmux` clearing it does not advance ours. Skip
    // the open frame — drain past the opening key without processing it — or the
    // opening char leaks into the prompt text (and single-char jumps submit on
    // it immediately).
    if !*armed {
        events.clear();
        *armed = true;
        return;
    }
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        let Some(state) = copy_prompt.open.as_mut() else {
            continue;
        };
        let step = apply_prompt_key(state, &ev.logical_key);
        match step {
            PromptStep::Continue => {}
            PromptStep::Submit => {
                let cmd = Prompt {
                    pane: state.pane,
                    kind: state.kind,
                    text: &state.text,
                }
                .into_raw_command();
                if let Some(client) = connection.client()
                    && let Err(e) = client.handle().send(&cmd)
                {
                    tracing::warn!(?e, "copy-mode prompt submit failed");
                }
                copy_prompt.open = None;
                *armed = false;
                break;
            }
            PromptStep::Cancel => {
                copy_prompt.open = None;
                *armed = false;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::PaneId;

    fn state(kind: PromptKind) -> CopyPromptState {
        CopyPromptState {
            kind,
            pane: PaneId(0),
            text: String::new(),
        }
    }

    fn char_key(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn escape_cancels() {
        let mut s = state(PromptKind::SearchForward);
        assert_eq!(apply_prompt_key(&mut s, &Key::Escape), PromptStep::Cancel);
    }

    #[test]
    fn enter_submits_search() {
        let mut s = state(PromptKind::SearchForward);
        s.text = "foo".into();
        assert_eq!(apply_prompt_key(&mut s, &Key::Enter), PromptStep::Submit);
    }

    #[test]
    fn backspace_pops_last_char() {
        let mut s = state(PromptKind::SearchForward);
        s.text = "ab".into();
        assert_eq!(
            apply_prompt_key(&mut s, &Key::Backspace),
            PromptStep::Continue
        );
        assert_eq!(s.text, "a");
    }

    #[test]
    fn backspace_on_empty_continues() {
        let mut s = state(PromptKind::SearchForward);
        assert_eq!(
            apply_prompt_key(&mut s, &Key::Backspace),
            PromptStep::Continue
        );
        assert_eq!(s.text, "");
    }

    #[test]
    fn search_accumulates_chars_and_waits_for_enter() {
        let mut s = state(PromptKind::SearchForward);
        assert_eq!(
            apply_prompt_key(&mut s, &char_key("h")),
            PromptStep::Continue
        );
        assert_eq!(
            apply_prompt_key(&mut s, &char_key("i")),
            PromptStep::Continue
        );
        assert_eq!(s.text, "hi");
        assert_eq!(apply_prompt_key(&mut s, &Key::Enter), PromptStep::Submit);
    }

    #[test]
    fn backward_search_also_multi_char() {
        let mut s = state(PromptKind::SearchBackward);
        assert_eq!(
            apply_prompt_key(&mut s, &char_key("x")),
            PromptStep::Continue
        );
        assert_eq!(
            apply_prompt_key(&mut s, &char_key("y")),
            PromptStep::Continue
        );
        assert_eq!(s.text, "xy");
        assert_eq!(apply_prompt_key(&mut s, &Key::Enter), PromptStep::Submit);
    }

    #[test]
    fn jump_forward_submits_on_first_char() {
        let mut s = state(PromptKind::JumpForward);
        assert_eq!(apply_prompt_key(&mut s, &char_key("w")), PromptStep::Submit);
        assert_eq!(s.text, "w");
    }

    #[test]
    fn jump_backward_submits_on_first_char() {
        let mut s = state(PromptKind::JumpBackward);
        assert_eq!(apply_prompt_key(&mut s, &char_key("a")), PromptStep::Submit);
        assert_eq!(s.text, "a");
    }

    #[test]
    fn jump_to_forward_submits_on_first_char() {
        let mut s = state(PromptKind::JumpToForward);
        assert_eq!(apply_prompt_key(&mut s, &char_key("e")), PromptStep::Submit);
        assert_eq!(s.text, "e");
    }

    #[test]
    fn jump_to_backward_submits_on_first_char() {
        let mut s = state(PromptKind::JumpToBackward);
        assert_eq!(apply_prompt_key(&mut s, &char_key("b")), PromptStep::Submit);
        assert_eq!(s.text, "b");
    }

    #[test]
    fn unknown_key_continues() {
        let mut s = state(PromptKind::SearchForward);
        assert_eq!(
            apply_prompt_key(&mut s, &Key::ArrowUp),
            PromptStep::Continue
        );
        assert_eq!(s.text, "");
    }

    use bevy::input::ButtonState;
    use bevy::input::keyboard::KeyCode;

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
            .init_resource::<CopyPrompt>()
            .insert_non_send_resource(TmuxConnection::default())
            .add_systems(
                Update,
                handle_prompt_input.run_if(|p: Res<CopyPrompt>| p.open.is_some()),
            );
        app
    }

    #[test]
    fn open_frame_skips_the_opening_key_for_a_jump_prompt() {
        let mut app = armed_skip_app();
        app.world_mut().resource_mut::<CopyPrompt>().open = Some(CopyPromptState {
            kind: PromptKind::JumpForward,
            pane: PaneId(0),
            text: String::new(),
        });
        // The opening `f` is still in the shared buffer when the prompt opens.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(char_key("f"), KeyCode::KeyF));
        app.update();

        let prompt = app.world().resource::<CopyPrompt>();
        assert!(
            prompt.open.is_some(),
            "single-char jump must NOT submit on the opening key (open frame is skipped)"
        );
        assert_eq!(
            prompt.open.as_ref().unwrap().text,
            "",
            "the opening key must not leak into the prompt text"
        );
    }

    #[test]
    fn target_key_after_open_frame_submits_a_jump_prompt() {
        let mut app = armed_skip_app();
        app.world_mut().resource_mut::<CopyPrompt>().open = Some(CopyPromptState {
            kind: PromptKind::JumpForward,
            pane: PaneId(0),
            text: String::new(),
        });
        // Open frame: the opening `f` is drained, prompt stays open.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(char_key("f"), KeyCode::KeyF));
        app.update();
        assert!(app.world().resource::<CopyPrompt>().open.is_some());

        // Next frame: the real target char submits (no client → just closes).
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(char_key("x"), KeyCode::KeyX));
        app.update();
        assert!(
            app.world().resource::<CopyPrompt>().open.is_none(),
            "the first post-open char submits a single-char jump"
        );
    }

    #[test]
    fn prompt_label_returns_correct_glyphs() {
        assert_eq!(prompt_label(PromptKind::SearchForward), "/");
        assert_eq!(prompt_label(PromptKind::SearchBackward), "?");
        assert_eq!(prompt_label(PromptKind::JumpForward), "f");
        assert_eq!(prompt_label(PromptKind::JumpBackward), "F");
        assert_eq!(prompt_label(PromptKind::JumpToForward), "t");
        assert_eq!(prompt_label(PromptKind::JumpToBackward), "T");
    }
}
