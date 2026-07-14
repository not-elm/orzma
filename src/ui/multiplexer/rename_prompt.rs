//! orzma-owned rename prompt: the rename-window shortcut action opens this
//! prompt (`RenameWindowRequest`). It renders a client-side bottom-bar edit
//! field pre-filled with the active window's current name, and commits
//! `MultiplexerWindow.name` directly on submit.

use crate::font::TerminalUiFont;
use crate::input::InputPhase;
use crate::multiplexer::request::RenameWindowRequest;
use crate::multiplexer::window::{ActiveMultiplexerWindow, MultiplexerWindow};
use crate::ui::multiplexer::modal::{ModalKeys, hide_bar, show_bar_with_text, spawn_bottom_bar};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;

const RENAME_Z: i32 = 340;
/// Background color of the rename bar: a distinct accent so the modal edit
/// bar reads apart from ordinary terminal output.
const RENAME_BAR_BG: Color = Color::srgb(0.30, 0.24, 0.55);

/// Registers the rename-prompt message intake, the input handler, and the
/// show/hide render systems.
pub(super) struct RenamePromptPlugin;

impl Plugin for RenamePromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<RenameWindowRequest>()
            .add_message::<KeyboardInput>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Startup, spawn_rename_ui)
            .add_systems(
                Update,
                (
                    open_rename_prompt.run_if(on_message::<RenameWindowRequest>),
                    handle_rename_input
                        .after(InputPhase::FocusedKey)
                        .run_if(resource_exists::<RenameState>),
                ),
            )
            .add_systems(
                PostUpdate,
                (
                    hide_bar::<RenameBar>.run_if(resource_removed::<RenameState>),
                    show_rename_ui.run_if(resource_exists_and_changed::<RenameState>),
                ),
            );
    }
}

/// The active window-rename edit buffer. Present as a resource only while
/// editing; its existence owns the keyboard — `apply_type`
/// (`src/input/shortcuts/apply.rs`) gates on it so a typed character never
/// leaks into the focused terminal's PTY.
#[derive(Resource)]
pub(crate) struct RenameState {
    /// The edit buffer, pre-filled with the target window's current name.
    text: String,
    /// The window this prompt renames — captured at open time so a window
    /// switch (window-bar click, select-window chord, or auto-activation
    /// after a window closes) mid-edit cannot redirect the submit to a
    /// different window.
    target: Entity,
    /// Keys pressed since the prompt opened (see [`ModalKeys`]).
    keys: ModalKeys,
}

impl RenameState {
    /// Opens a prompt renaming `target`, pre-filled with `current_name` (the
    /// window's current name, or empty if unset).
    pub(crate) fn new(target: Entity, current_name: String, keys: ModalKeys) -> Self {
        Self {
            text: current_name,
            target,
            keys,
        }
    }

    /// Appends committed IME text to the edit buffer (`read_ime_events`
    /// routes commits here while this prompt owns the keyboard).
    pub(crate) fn append(&mut self, s: &str) {
        self.text.push_str(s);
    }

    /// The current edit buffer (test-only observation point for the IME
    /// commit-routing test in `src/input/ime.rs`).
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Applies one key: Enter submits, Escape cancels, Backspace deletes the
    /// last char, a character or Space is appended. Other keys are ignored.
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
            Key::Space => {
                self.text.push(' ');
                RenameStep::Continue
            }
            _ => RenameStep::Continue,
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
    spawn_bottom_bar(&mut commands, ui_font, RenameBar, RENAME_BAR_BG, RENAME_Z);
}

fn show_rename_ui(
    mut bar: Query<(&mut Node, &Children), With<RenameBar>>,
    mut texts: Query<&mut Text>,
    state: Res<RenameState>,
) {
    show_bar_with_text(
        &mut bar,
        &mut texts,
        format!("Rename window: {}\u{258f}", state.text),
    );
}

/// Consumes `RenameWindowRequest`, inserting `RenameState` pre-filled from
/// the active window's current name. A prompt already open is left
/// untouched — an in-flight rename is never silently reset by a later
/// request.
fn open_rename_prompt(
    mut commands: Commands,
    mut requests: MessageReader<RenameWindowRequest>,
    state: Option<Res<RenameState>>,
    active_windows: Query<(Entity, &MultiplexerWindow), With<ActiveMultiplexerWindow>>,
    messages: Res<Messages<KeyboardInput>>,
) {
    let requested = requests.read().last().is_some();
    if !requested || state.is_some() {
        return;
    }
    let Ok((target, window)) = active_windows.single() else {
        return;
    };
    commands.insert_resource(RenameState::new(
        target,
        window.name.clone().unwrap_or_default(),
        ModalKeys::at_current(&messages),
    ));
}

/// Edits the buffer with the keys pressed since the prompt opened (see
/// [`ModalKeys`] for the intake semantics), and on Enter commits the name to
/// the window captured at open time — never to whatever window happens to be
/// active at submit time.
fn handle_rename_input(
    mut commands: Commands,
    mut state: ResMut<RenameState>,
    mut windows: Query<&mut MultiplexerWindow>,
    messages: Res<Messages<KeyboardInput>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if state.keys.is_empty(&messages) {
        return;
    }
    let pressed: Vec<Key> = state.keys.pressed(&messages, &keys).cloned().collect();
    for key in &pressed {
        match state.apply_key(key) {
            RenameStep::Continue => {}
            RenameStep::Submit => {
                if let Ok(mut window) = windows.get_mut(state.target) {
                    let trimmed = state.text.trim();
                    let next_name = if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    };
                    if window.name != next_name {
                        window.name = next_name;
                    }
                }
                commands.remove_resource::<RenameState>();
                break;
            }
            RenameStep::Cancel => {
                commands.remove_resource::<RenameState>();
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

    fn char_key(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn new_prefills_text_from_current_name() {
        let state = RenameState::new(
            Entity::PLACEHOLDER,
            "nvim".to_string(),
            ModalKeys::default(),
        );
        assert_eq!(state.text, "nvim");
    }

    #[test]
    fn apply_key_escape_cancels() {
        let mut state = RenameState::new(
            Entity::PLACEHOLDER,
            "nvim".to_string(),
            ModalKeys::default(),
        );
        assert_eq!(state.apply_key(&Key::Escape), RenameStep::Cancel);
    }

    #[test]
    fn apply_key_enter_submits() {
        let mut state = RenameState::new(
            Entity::PLACEHOLDER,
            "nvim".to_string(),
            ModalKeys::default(),
        );
        assert_eq!(state.apply_key(&Key::Enter), RenameStep::Submit);
    }

    #[test]
    fn apply_key_char_appends_and_continues() {
        let mut state = RenameState::new(Entity::PLACEHOLDER, String::new(), ModalKeys::default());
        assert_eq!(state.apply_key(&char_key("a")), RenameStep::Continue);
        assert_eq!(state.apply_key(&char_key("b")), RenameStep::Continue);
        assert_eq!(state.text, "ab");
    }

    #[test]
    fn apply_key_backspace_pops_last_char() {
        let mut state = RenameState::new(
            Entity::PLACEHOLDER,
            "nvim".to_string(),
            ModalKeys::default(),
        );
        assert_eq!(state.apply_key(&Key::Backspace), RenameStep::Continue);
        assert_eq!(state.text, "nvi");
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

    /// Spawns an active window (with a dummy pane, no PTY) carrying `name`.
    /// Returns the window entity.
    fn spawn_active_window(app: &mut App, name: Option<&str>) -> Entity {
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: name.map(str::to_string),
                    active_pane: pane,
                },
                ActiveMultiplexerWindow,
            ))
            .id()
    }

    fn input_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(
                Update,
                handle_rename_input.run_if(resource_exists::<RenameState>),
            );
        app
    }

    #[test]
    fn open_frame_skips_the_opening_key() {
        let mut app = input_app();
        let window = spawn_active_window(&mut app, None);
        // The opening rename-window shortcut key is still in the shared
        // buffer when the prompt opens; the open-time cursor skips it.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(char_key(","), KeyCode::Comma));
        let keys = ModalKeys::at_current(app.world().resource::<Messages<KeyboardInput>>());
        app.world_mut()
            .insert_resource(RenameState::new(window, "nvim".to_string(), keys));
        app.update();

        let state = app.world().resource::<RenameState>();
        assert_eq!(
            state.text, "nvim",
            "the opening key must not leak into the prefilled text"
        );
    }

    #[test]
    fn fast_first_char_after_open_is_not_swallowed() {
        let mut app = input_app();
        let window = spawn_active_window(&mut app, None);
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(char_key(","), KeyCode::Comma));
        let keys = ModalKeys::at_current(app.world().resource::<Messages<KeyboardInput>>());
        app.world_mut()
            .insert_resource(RenameState::new(window, "nvim".to_string(), keys));
        // A fast first character lands before the handler's first run.
        app.world_mut()
            .write_message(key_event(char_key("a"), KeyCode::KeyA));
        app.update();

        assert_eq!(
            app.world().resource::<RenameState>().text,
            "nvima",
            "a character typed in the prompt's first frame must not be swallowed"
        );
    }

    #[test]
    fn modifier_chord_char_is_not_appended() {
        let mut app = input_app();
        let window = spawn_active_window(&mut app, None);
        app.world_mut().insert_resource(RenameState::new(
            window,
            "nvim".to_string(),
            ModalKeys::default(),
        ));
        app.update();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        app.world_mut()
            .write_message(key_event(char_key("v"), KeyCode::KeyV));
        app.update();

        assert_eq!(
            app.world().resource::<RenameState>().text,
            "nvim",
            "a character riding a held Cmd chord is a command, not typed text"
        );
    }

    #[test]
    fn submit_renames_the_window_the_prompt_opened_for() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .add_message::<RenameWindowRequest>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(
                Update,
                (
                    open_rename_prompt,
                    handle_rename_input.run_if(resource_exists::<RenameState>),
                )
                    .chain(),
            );
        let window_a = spawn_active_window(&mut app, Some("old-a"));
        let pane_b = app.world_mut().spawn_empty().id();
        let window_b = app
            .world_mut()
            .spawn(MultiplexerWindow {
                index: 1,
                name: Some("old-b".into()),
                active_pane: pane_b,
            })
            .id();

        app.world_mut().write_message(RenameWindowRequest);
        app.update();
        app.update();

        // The user switches windows mid-edit: the marker moves to window B.
        app.world_mut()
            .entity_mut(window_a)
            .remove::<ActiveMultiplexerWindow>();
        app.world_mut()
            .entity_mut(window_b)
            .insert(ActiveMultiplexerWindow);

        app.world_mut().resource_mut::<RenameState>().text = "renamed".to_string();
        app.world_mut()
            .write_message(key_event(Key::Enter, KeyCode::Enter));
        app.update();

        assert_eq!(
            app.world().get::<MultiplexerWindow>(window_a).unwrap().name,
            Some("renamed".to_string()),
            "the submit must land on the window the prompt opened for"
        );
        assert_eq!(
            app.world().get::<MultiplexerWindow>(window_b).unwrap().name,
            Some("old-b".to_string()),
            "the window that became active mid-edit must be untouched"
        );
    }

    #[test]
    fn apply_key_space_appends_space() {
        let mut state =
            RenameState::new(Entity::PLACEHOLDER, "a".to_string(), ModalKeys::default());
        assert_eq!(state.apply_key(&Key::Space), RenameStep::Continue);
        assert_eq!(
            state.text, "a ",
            "the spacebar arrives as Key::Space and must append a space"
        );
    }

    #[test]
    fn submit_sets_window_name() {
        let mut app = input_app();
        let window = spawn_active_window(&mut app, None);
        app.world_mut().insert_resource(RenameState::new(
            window,
            "build".to_string(),
            ModalKeys::default(),
        ));
        app.update();

        app.world_mut()
            .write_message(key_event(Key::Enter, KeyCode::Enter));
        app.update();

        assert_eq!(
            app.world().get::<MultiplexerWindow>(window).unwrap().name,
            Some("build".to_string()),
            "Enter must commit the edited text as the window's name"
        );
        assert!(
            app.world().get_resource::<RenameState>().is_none(),
            "submitting must remove RenameState"
        );
    }

    #[test]
    fn submit_empty_clears_name_to_none() {
        let mut app = input_app();
        let window = spawn_active_window(&mut app, Some("old"));
        app.world_mut().insert_resource(RenameState::new(
            window,
            "   ".to_string(),
            ModalKeys::default(),
        ));
        app.update();

        app.world_mut()
            .write_message(key_event(Key::Enter, KeyCode::Enter));
        app.update();

        assert_eq!(
            app.world().get::<MultiplexerWindow>(window).unwrap().name,
            None,
            "submitting a blank (or whitespace-only) name clears back to the auto-title"
        );
    }

    #[test]
    fn cancel_leaves_name_unchanged() {
        let mut app = input_app();
        let window = spawn_active_window(&mut app, Some("old"));
        app.world_mut().insert_resource(RenameState::new(
            window,
            "something-else".to_string(),
            ModalKeys::default(),
        ));
        app.update();

        app.world_mut()
            .write_message(key_event(Key::Escape, KeyCode::Escape));
        app.update();

        assert_eq!(
            app.world().get::<MultiplexerWindow>(window).unwrap().name,
            Some("old".to_string()),
            "Escape must not change the window's name"
        );
        assert!(
            app.world().get_resource::<RenameState>().is_none(),
            "cancel must remove RenameState"
        );
    }

    #[test]
    fn open_rename_prompt_inserts_state_prefilled_from_active_window_name() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<RenameWindowRequest>()
            .add_message::<KeyboardInput>()
            .add_systems(Update, open_rename_prompt);
        spawn_active_window(&mut app, Some("nvim"));

        app.world_mut().write_message(RenameWindowRequest);
        app.update();

        let state = app
            .world()
            .get_resource::<RenameState>()
            .expect("RenameState must be inserted after RenameWindowRequest");
        assert_eq!(state.text, "nvim");
    }

    #[test]
    fn already_open_ignores_new_rename_request() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<RenameWindowRequest>()
            .add_message::<KeyboardInput>()
            .add_systems(Update, open_rename_prompt);
        spawn_active_window(&mut app, Some("nvim"));

        app.world_mut().write_message(RenameWindowRequest);
        app.update();
        app.world_mut().resource_mut::<RenameState>().text = "edited".to_string();

        app.world_mut().write_message(RenameWindowRequest);
        app.update();

        assert_eq!(
            app.world().resource::<RenameState>().text,
            "edited",
            "a prompt already open must not be reset by a later RenameWindowRequest"
        );
    }
}
