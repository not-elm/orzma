//! orzma-owned confirm prompt: the kill-pane / kill-window shortcut actions
//! open this prompt (`OpenKillPaneConfirm` / `OpenKillWindowConfirm`). It
//! renders a client-side bottom-bar y/n prompt and fires the matching kill
//! request only on confirm.

use crate::font::TerminalUiFont;
use crate::input::InputPhase;
use crate::multiplexer::request::{
    KillPaneRequest, KillWindowRequest, OpenKillPaneConfirm, OpenKillWindowConfirm,
};
use crate::ui::multiplexer::modal::{
    ModalKeys, any_modal_open, hide_bar, show_bar_with_text, spawn_bottom_bar,
};
use crate::ui::multiplexer::rename_prompt::RenameState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;

const CONFIRM_Z: i32 = 330;
/// Background color of the confirm bar: a deliberate, saturated accent so a
/// destructive kill-pane/kill-window prompt reads as distinct from ordinary
/// terminal output.
const CONFIRM_BAR_BG: Color = Color::srgb(0.16, 0.35, 0.60);

/// Registers the kill-pane / kill-window confirm prompt: the Open* message
/// intake, the y/n input handler, and the show/hide render systems.
pub(super) struct ConfirmPromptPlugin;

impl Plugin for ConfirmPromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<OpenKillPaneConfirm>()
            .add_message::<OpenKillWindowConfirm>()
            .add_message::<KeyboardInput>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Startup, spawn_confirm_ui)
            .add_systems(
                Update,
                (
                    open_confirm_prompt.run_if(
                        on_message::<OpenKillPaneConfirm>
                            .or_else(on_message::<OpenKillWindowConfirm>),
                    ),
                    handle_confirm_input
                        .after(InputPhase::FocusedKey)
                        .run_if(resource_exists::<ConfirmState>),
                ),
            )
            .add_systems(
                PostUpdate,
                (
                    hide_bar::<ConfirmBar>.run_if(resource_removed::<ConfirmState>),
                    show_confirm_ui.run_if(resource_exists_and_changed::<ConfirmState>),
                ),
            );
    }
}

/// The active kill-pane / kill-window confirm prompt: the message to show
/// and the kill request to fire on confirm. Present as a resource only
/// while awaiting y/n; its existence owns the keyboard like the vi-mode
/// prompt — `apply_type` (`src/input/shortcuts/apply.rs`) gates on it so an
/// answering `y`/`n` never reaches the focused terminal's PTY.
#[derive(Resource)]
pub(crate) struct ConfirmState {
    /// Prompt text shown to the user.
    message: String,
    /// The kill request fired on confirm.
    action: ConfirmAction,
    /// Keys pressed since the prompt opened (see [`ModalKeys`]).
    keys: ModalKeys,
}

impl ConfirmState {
    /// Builds the prompt state for a pending `KillPaneRequest`.
    pub(crate) fn kill_pane(pane: Entity, keys: ModalKeys) -> Self {
        Self {
            message: "kill pane? [y/N]".to_string(),
            action: ConfirmAction::KillPane(pane),
            keys,
        }
    }

    /// Builds the prompt state for a pending `KillWindowRequest`.
    fn kill_window(window: Entity, keys: ModalKeys) -> Self {
        Self {
            message: "kill window? [y/N]".to_string(),
            action: ConfirmAction::KillWindow(window),
            keys,
        }
    }
}

/// The kill request a confirmed prompt fires on `ConfirmStep::Confirm`.
#[derive(Clone, Copy)]
enum ConfirmAction {
    /// Fires `KillPaneRequest` for this pane.
    KillPane(Entity),
    /// Fires `KillWindowRequest` for this window.
    KillWindow(Entity),
}

/// The effect of one key on an open confirm prompt.
#[derive(Debug, PartialEq, Eq)]
enum ConfirmStep {
    Confirm,
    Cancel,
}

impl ConfirmStep {
    /// Maps one key to a confirm/cancel decision: `y`/`Y` confirm;
    /// `n`/`N`/Enter/Escape cancel — Enter takes the advertised `[y/N]`
    /// default (No), matching tmux's `confirm-before`; any other key is
    /// ignored (returns `None`).
    fn classify(key: &Key) -> Option<Self> {
        match key {
            Key::Enter => Some(Self::Cancel),
            Key::Escape => Some(Self::Cancel),
            Key::Character(s) => match s.as_str() {
                "y" | "Y" => Some(Self::Confirm),
                "n" | "N" => Some(Self::Cancel),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Component)]
struct ConfirmBar;

fn spawn_confirm_ui(mut commands: Commands, ui_font: Option<Res<TerminalUiFont>>) {
    spawn_bottom_bar(
        &mut commands,
        ui_font,
        ConfirmBar,
        CONFIRM_BAR_BG,
        CONFIRM_Z,
    );
}

fn show_confirm_ui(
    mut bar: Query<(&mut Node, &Children), With<ConfirmBar>>,
    mut texts: Query<&mut Text>,
    state: Res<ConfirmState>,
) {
    show_bar_with_text(&mut bar, &mut texts, state.message.clone());
}

/// Consumes `OpenKillPaneConfirm` / `OpenKillWindowConfirm`, inserting
/// `ConfirmState` for the requested target. Both readers are always drained
/// (so a stale Open* message from this frame never resurfaces later); a
/// prompt already open is left untouched — an in-flight confirm is never
/// silently redirected to a different kill target. Also refuses to open when
/// a `RenameState` prompt is already up, so a kill-confirm chord pressed
/// while renaming is a no-op rather than opening a second modal.
fn open_confirm_prompt(
    mut commands: Commands,
    mut kill_pane: MessageReader<OpenKillPaneConfirm>,
    mut kill_window: MessageReader<OpenKillWindowConfirm>,
    state: Option<Res<ConfirmState>>,
    rename: Option<Res<RenameState>>,
    messages: Res<Messages<KeyboardInput>>,
) {
    let pane_target = kill_pane.read().last().map(|msg| msg.pane);
    let window_target = kill_window.read().last().map(|msg| msg.window);
    if any_modal_open(state, rename) {
        return;
    }
    if let Some(pane) = pane_target {
        commands.insert_resource(ConfirmState::kill_pane(
            pane,
            ModalKeys::at_current(&messages),
        ));
    } else if let Some(window) = window_target {
        commands.insert_resource(ConfirmState::kill_window(
            window,
            ModalKeys::at_current(&messages),
        ));
    }
}

/// Applies the first `y`/`n`/Enter/Escape decision among the keys pressed
/// since the prompt opened (see [`ModalKeys`] for the intake semantics).
fn handle_confirm_input(
    mut commands: Commands,
    mut state: ResMut<ConfirmState>,
    messages: Res<Messages<KeyboardInput>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if state.keys.is_empty(&messages) {
        return;
    }
    let action = state.action;
    let Some(step) = state
        .keys
        .pressed(&messages, &keys)
        .find_map(ConfirmStep::classify)
    else {
        return;
    };
    if step == ConfirmStep::Confirm {
        match action {
            ConfirmAction::KillPane(pane) => {
                commands.trigger(KillPaneRequest { pane });
            }
            ConfirmAction::KillWindow(window) => {
                commands.trigger(KillWindowRequest { window });
            }
        }
    }
    commands.remove_resource::<ConfirmState>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;

    #[test]
    fn confirm_step_maps_keys() {
        assert_eq!(
            ConfirmStep::classify(&Key::Enter),
            Some(ConfirmStep::Cancel),
            "Enter takes the advertised [y/N] default: No"
        );
        assert_eq!(
            ConfirmStep::classify(&Key::Character("y".into())),
            Some(ConfirmStep::Confirm)
        );
        assert_eq!(
            ConfirmStep::classify(&Key::Character("Y".into())),
            Some(ConfirmStep::Confirm)
        );
        assert_eq!(
            ConfirmStep::classify(&Key::Escape),
            Some(ConfirmStep::Cancel)
        );
        assert_eq!(
            ConfirmStep::classify(&Key::Character("n".into())),
            Some(ConfirmStep::Cancel)
        );
        assert_eq!(ConfirmStep::classify(&Key::Character("x".into())), None);
    }

    #[derive(Resource, Default)]
    struct Captured {
        killed_panes: Vec<Entity>,
        killed_windows: Vec<Entity>,
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .add_plugins(ConfirmPromptPlugin)
            .init_resource::<Captured>()
            .add_observer(|ev: On<KillPaneRequest>, mut c: ResMut<Captured>| {
                c.killed_panes.push(ev.pane);
            })
            .add_observer(|ev: On<KillWindowRequest>, mut c: ResMut<Captured>| {
                c.killed_windows.push(ev.window);
            });
        app
    }

    fn press(key: Key) -> KeyboardInput {
        KeyboardInput {
            key_code: bevy::input::keyboard::KeyCode::KeyA,
            logical_key: key,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    #[test]
    fn open_kill_pane_confirm_inserts_state() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(OpenKillPaneConfirm { pane });
        app.update();

        let state = app
            .world()
            .get_resource::<ConfirmState>()
            .expect("ConfirmState must be inserted after OpenKillPaneConfirm");
        assert!(
            matches!(state.action, ConfirmAction::KillPane(p) if p == pane),
            "ConfirmState must carry ConfirmAction::KillPane(pane)"
        );
    }

    #[test]
    fn already_open_ignores_new_open_message() {
        let mut app = test_app();
        let first = app.world_mut().spawn_empty().id();
        let second = app.world_mut().spawn_empty().id();
        app.world_mut()
            .write_message(OpenKillPaneConfirm { pane: first });
        app.update();
        app.world_mut()
            .write_message(OpenKillPaneConfirm { pane: second });
        app.update();

        let state = app
            .world()
            .get_resource::<ConfirmState>()
            .expect("ConfirmState must still be present");
        assert!(
            matches!(state.action, ConfirmAction::KillPane(p) if p == first),
            "a prompt already open must not be redirected to a later Open* message"
        );
    }

    #[test]
    fn second_modal_refused() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut().insert_resource(RenameState::new(
            pane,
            "build".to_string(),
            ModalKeys::default(),
        ));
        app.world_mut().write_message(OpenKillPaneConfirm { pane });
        app.update();

        assert!(
            app.world().get_resource::<ConfirmState>().is_none(),
            "a kill-confirm request must be refused while a RenameState prompt is open"
        );
    }

    #[test]
    fn confirm_fires_kill_pane_request() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(pane, ModalKeys::default()));

        app.world_mut()
            .write_message(press(Key::Character("y".into())));
        app.update();

        assert_eq!(
            app.world().resource::<Captured>().killed_panes,
            vec![pane],
            "confirming must trigger KillPaneRequest for the prompted pane"
        );
        assert!(
            app.world().get_resource::<ConfirmState>().is_none(),
            "confirming must remove ConfirmState"
        );
    }

    #[test]
    fn confirm_fires_kill_window_request() {
        let mut app = test_app();
        let window = app.world_mut().spawn_empty().id();
        app.world_mut()
            .insert_resource(ConfirmState::kill_window(window, ModalKeys::default()));
        app.update();

        app.world_mut()
            .write_message(press(Key::Character("y".into())));
        app.update();

        assert_eq!(
            app.world().resource::<Captured>().killed_windows,
            vec![window],
            "confirming must trigger KillWindowRequest for the prompted window"
        );
        assert!(
            app.world().get_resource::<ConfirmState>().is_none(),
            "confirming must remove ConfirmState"
        );
    }

    #[test]
    fn enter_answers_no_per_the_advertised_default() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(pane, ModalKeys::default()));
        app.update();

        app.world_mut().write_message(press(Key::Enter));
        app.update();

        assert!(
            app.world().resource::<Captured>().killed_panes.is_empty(),
            "Enter must take the advertised [y/N] default — No — and never kill"
        );
        assert!(
            app.world().get_resource::<ConfirmState>().is_none(),
            "Enter (default No) must dismiss the prompt"
        );
    }

    #[test]
    fn modifier_chord_y_does_not_answer() {
        let mut app = test_app();
        app.init_resource::<ButtonInput<KeyCode>>();
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(pane, ModalKeys::default()));
        app.update();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        app.world_mut()
            .write_message(press(Key::Character("y".into())));
        app.update();

        assert!(
            app.world().resource::<Captured>().killed_panes.is_empty(),
            "Cmd+Y is a chord, not an answer — it must not confirm the kill"
        );
        assert!(
            app.world().get_resource::<ConfirmState>().is_some(),
            "the prompt must stay open after an ignored chord"
        );
    }

    #[test]
    fn fast_answer_after_open_is_not_swallowed() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        // The opening chord key is already in the shared buffer at open time.
        app.world_mut()
            .write_message(press(Key::Character("x".into())));
        let keys = ModalKeys::at_current(app.world().resource::<Messages<KeyboardInput>>());
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(pane, keys));
        // A fast 'y' lands before the handler's first run.
        app.world_mut()
            .write_message(press(Key::Character("y".into())));
        app.update();

        assert_eq!(
            app.world().resource::<Captured>().killed_panes,
            vec![pane],
            "an answer pressed in the prompt's first frame must not be swallowed"
        );
    }

    #[test]
    fn cancel_dismisses_without_kill() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(pane, ModalKeys::default()));
        app.update();

        app.world_mut().write_message(press(Key::Escape));
        app.update();

        assert!(
            app.world().resource::<Captured>().killed_panes.is_empty(),
            "cancel must not trigger KillPaneRequest"
        );
        assert!(
            app.world().get_resource::<ConfirmState>().is_none(),
            "cancel must remove ConfirmState"
        );
    }
}
