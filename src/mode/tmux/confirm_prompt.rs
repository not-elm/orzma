//! ozmux-owned confirm prompt: the kill-pane / kill-window shortcut actions
//! open this prompt. It renders a client-side modal y/n prompt and runs the
//! inner command only on confirm.

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
use ozmux_tmux::TmuxClient;

const CONFIRM_Z: i32 = 330;

/// Registers the confirm-prompt input system and the show/hide render systems.
pub(crate) struct ConfirmPromptPlugin;

impl Plugin for ConfirmPromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_confirm_ui)
            .add_systems(
                Update,
                handle_confirm_input
                    .after(InputPhase::FocusedKey)
                    .run_if(resource_exists::<ConfirmState>),
            )
            .add_systems(
                PostUpdate,
                (
                    hide_confirm_ui.run_if(resource_removed::<ConfirmState>),
                    show_confirm_ui.run_if(resource_exists_and_changed::<ConfirmState>),
                ),
            );
    }
}

/// The active `confirm-before` prompt: the message to show and the tmux command
/// to run on confirm. Present as a resource only while awaiting y/n; its
/// existence owns the keyboard like the copy-mode prompt and the session picker.
#[derive(Resource)]
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

fn hide_confirm_ui(mut bar: Query<&mut Node, With<ConfirmBar>>) {
    if let Ok(mut node) = bar.single_mut() {
        node.display = Display::None;
    }
}

fn show_confirm_ui(
    mut bar: Query<&mut Node, With<ConfirmBar>>,
    mut texts: Query<&mut Text>,
    state: Res<ConfirmState>,
    children_query: Query<&Children, With<ConfirmBar>>,
) {
    let Ok(mut node) = bar.single_mut() else {
        return;
    };
    node.display = Display::Flex;
    if let Ok(children) = children_query.single() {
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                text.0 = state.message.clone();
            }
        }
    }
}

fn handle_confirm_input(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    mut armed: Local<bool>,
    mut client: Option<Single<&mut TmuxClient>>,
    state: Res<ConfirmState>,
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
        match confirm_step(&ev.logical_key) {
            None => {}
            Some(ConfirmStep::Confirm) => {
                if let Some(client) = client.as_deref_mut()
                    && let Err(e) = client.send_effect(&state.command)
                {
                    tracing::warn!(?e, "confirm-before command send failed");
                }
                commands.remove_resource::<ConfirmState>();
                *armed = false;
                break;
            }
            Some(ConfirmStep::Cancel) => {
                commands.remove_resource::<ConfirmState>();
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
