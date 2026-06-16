//! ozmux-owned `confirm-before` prompt. tmux's `confirm-before <cmd>` opens a
//! client-side modal y/n prompt that a `-CC` control client cannot render or
//! answer, so `forward_keys_to_tmux` detects such a binding, opens this prompt
//! instead of forwarding it, and runs the inner command only on confirm.

use crate::font::TerminalUiFont;
use crate::theme;
use bevy::app::{App, Plugin};
use bevy::ecs::message::MessageReader;
use bevy::ecs::schedule::common_conditions::resource_exists_and_changed;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use ozmux_tmux::TmuxConnection;

const CONFIRM_Z: i32 = 330;

/// Registers the confirm-prompt resource, input system, and render system.
pub(crate) struct ConfirmPromptPlugin;

impl Plugin for ConfirmPromptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConfirmPrompt>()
            .add_systems(Startup, spawn_confirm_ui)
            .add_systems(
                Update,
                handle_confirm_input
                    .after(crate::input::InputPhase::FocusedKey)
                    .run_if(|p: Res<ConfirmPrompt>| p.open.is_some()),
            )
            .add_systems(
                PostUpdate,
                sync_confirm_ui.run_if(resource_exists_and_changed::<ConfirmPrompt>),
            );
    }
}

/// The active `confirm-before` prompt. Present while awaiting y/n; owns the
/// keyboard like the copy-mode prompt and the session picker.
#[derive(Resource, Default)]
pub(crate) struct ConfirmPrompt {
    /// The pending confirmation, if open.
    pub(crate) open: Option<ConfirmState>,
}

/// A pending confirmation: the message to show and the tmux command to run on
/// confirm.
pub(crate) struct ConfirmState {
    /// Prompt text shown to the user (from `-p`, or a default).
    pub(crate) message: String,
    /// The inner tmux command sent verbatim on confirm.
    pub(crate) command: String,
}

/// The effect of one key on an open confirm prompt.
#[derive(Debug, PartialEq, Eq)]
enum ConfirmStep {
    Confirm,
    Cancel,
}

/// Maps one key to a confirm/cancel decision: `y`/`Y`/Enter confirm, `n`/`N`/
/// Escape cancel; any other key is ignored (returns `None`).
fn confirm_step(key: &Key) -> Option<ConfirmStep> {
    match key {
        Key::Enter => Some(ConfirmStep::Confirm),
        Key::Escape => Some(ConfirmStep::Cancel),
        Key::Character(s) => match s.as_str() {
            "y" | "Y" => Some(ConfirmStep::Confirm),
            "n" | "N" => Some(ConfirmStep::Cancel),
            _ => None,
        },
        _ => None,
    }
}

/// Parses a `confirm-before [-by] [-c key] [-p prompt] [-t client] <command>`
/// binding into `(message, inner command)`. Returns `None` if the command is
/// not a `confirm-before` or has no inner command. The message defaults to
/// `"<command>? (y/n)"` when `-p` is absent.
pub(crate) fn parse_confirm_before(command: &str) -> Option<(String, String)> {
    let tokens = tokenize(command);
    let mut it = tokens.into_iter();
    if it.next().as_deref() != Some("confirm-before") {
        return None;
    }
    let mut message: Option<String> = None;
    let mut inner: Vec<String> = Vec::new();
    while let Some(tok) = it.next() {
        match tok.as_str() {
            "-p" => message = it.next(),
            "-c" | "-t" => {
                it.next();
            }
            "-b" | "-y" => {}
            _ => {
                inner.push(tok);
                inner.extend(it.by_ref());
            }
        }
    }
    if inner.is_empty() {
        return None;
    }
    let command = inner.join(" ");
    let message = message.unwrap_or_else(|| format!("{command}? (y/n)"));
    Some((message, command))
}

/// Splits a tmux command line into tokens, honoring single and double quotes
/// (quotes are stripped; whitespace inside quotes is preserved). Empty quoted
/// tokens (`''`) yield an empty token.
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut in_single = false;
    let mut in_double = false;
    for c in line.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                started = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                started = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if started {
                    tokens.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(cur);
    }
    tokens
}

#[derive(Component)]
struct ConfirmBar;

fn spawn_confirm_ui(mut commands: Commands, ui_font: Option<Res<TerminalUiFont>>) {
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
            GlobalZIndex(CONFIRM_Z),
            ConfirmBar,
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

fn sync_confirm_ui(
    mut bar: Query<&mut Node, With<ConfirmBar>>,
    mut texts: Query<&mut Text>,
    confirm: Res<ConfirmPrompt>,
    children_query: Query<&Children, With<ConfirmBar>>,
) {
    let Ok(mut node) = bar.single_mut() else {
        return;
    };
    match &confirm.open {
        None => node.display = Display::None,
        Some(state) => {
            node.display = Display::Flex;
            if let Ok(children) = children_query.single() {
                for child in children.iter() {
                    if let Ok(mut text) = texts.get_mut(child) {
                        text.0 = state.message.clone();
                    }
                }
            }
        }
    }
}

fn handle_confirm_input(
    mut confirm: ResMut<ConfirmPrompt>,
    mut events: MessageReader<KeyboardInput>,
    mut armed: Local<bool>,
    connection: NonSend<TmuxConnection>,
) {
    // NOTE: the bound key that opened the prompt (e.g. M-t) is still in the
    // shared KeyboardInput buffer; this reader has its own cursor, so skip the
    // open frame — drain past the opening key — or it could be read as the y/n
    // answer (a confirm-before bound to `y`/`n` would self-answer instantly).
    if !*armed {
        events.clear();
        *armed = true;
        return;
    }
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        let Some(state) = confirm.open.as_ref() else {
            continue;
        };
        match confirm_step(&ev.logical_key) {
            None => {}
            Some(ConfirmStep::Confirm) => {
                if let Some(client) = connection.client()
                    && let Err(e) = client.handle().send(&state.command)
                {
                    tracing::warn!(?e, "confirm-before command send failed");
                }
                confirm.open = None;
                *armed = false;
                break;
            }
            Some(ConfirmStep::Cancel) => {
                confirm.open = None;
                *armed = false;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_inner_command_with_default_message() {
        let (msg, cmd) = parse_confirm_before("confirm-before 'kill-window'").unwrap();
        assert_eq!(cmd, "kill-window");
        assert_eq!(msg, "kill-window? (y/n)");
    }

    #[test]
    fn parses_p_message_and_inner_command() {
        let (msg, cmd) =
            parse_confirm_before(r#"confirm-before -p "kill-window #W? (y/n)" kill-window"#)
                .unwrap();
        assert_eq!(cmd, "kill-window");
        assert_eq!(msg, "kill-window #W? (y/n)");
    }

    #[test]
    fn skips_boolean_and_arg_flags() {
        let (_, cmd) = parse_confirm_before(r#"confirm-before -b -c y -t main kill-pane"#).unwrap();
        assert_eq!(cmd, "kill-pane");
    }

    #[test]
    fn joins_multi_token_inner_command() {
        let (_, cmd) = parse_confirm_before("confirm-before kill-window -t @1").unwrap();
        assert_eq!(cmd, "kill-window -t @1");
    }

    #[test]
    fn rejects_non_confirm_before() {
        assert!(parse_confirm_before("kill-window").is_none());
        assert!(parse_confirm_before("confirm-before").is_none());
        assert!(parse_confirm_before("confirm-before -p \"msg\"").is_none());
    }

    #[test]
    fn confirm_step_maps_keys() {
        assert_eq!(confirm_step(&Key::Enter), Some(ConfirmStep::Confirm));
        assert_eq!(
            confirm_step(&Key::Character("y".into())),
            Some(ConfirmStep::Confirm)
        );
        assert_eq!(
            confirm_step(&Key::Character("Y".into())),
            Some(ConfirmStep::Confirm)
        );
        assert_eq!(confirm_step(&Key::Escape), Some(ConfirmStep::Cancel));
        assert_eq!(
            confirm_step(&Key::Character("n".into())),
            Some(ConfirmStep::Cancel)
        );
        assert_eq!(confirm_step(&Key::Character("x".into())), None);
    }
}
