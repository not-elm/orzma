//! orzma-owned rename prompt: the rename-window shortcut action opens this
//! prompt (`RenameWindowRequest`). It renders a client-side bottom-bar edit
//! field pre-filled with the active window's current name, and commits
//! `MultiplexerWindow.name` directly on submit.

use crate::font::TerminalUiFont;
use crate::input::InputPhase;
use crate::multiplexer::request::RenameWindowRequest;
use crate::multiplexer::window::{ActiveMultiplexerWindow, MultiplexerWindow};
use crate::ui::multiplexer::confirm_prompt::ConfirmState;
use crate::ui::multiplexer::modal::any_modal_open;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;

const RENAME_Z: i32 = 340;
/// Background color of the rename bar: distinct from the kill-pane /
/// kill-window confirm bar's destructive blue, so an in-progress edit reads
/// as a different kind of modal.
const RENAME_BAR_BG: Color = Color::srgb(0.30, 0.24, 0.55);
/// Foreground (text) color of the rename bar, contrasting `RENAME_BAR_BG`.
const RENAME_BAR_FG: Color = Color::srgb(0.95, 0.95, 0.95);
/// Font size of the rename bar's edit text.
const RENAME_BAR_FONT_SIZE_PX: f32 = 12.0;

/// Registers the rename-prompt message intake, the input handler, and the
/// show/hide render systems.
pub(super) struct RenamePromptPlugin;

impl Plugin for RenamePromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<RenameWindowRequest>()
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
                    hide_rename_ui.run_if(resource_removed::<RenameState>),
                    show_rename_ui.run_if(resource_exists_and_changed::<RenameState>),
                ),
            );
    }
}

/// The active window-rename edit buffer. Present as a resource only while
/// editing; its existence owns the keyboard like the confirm prompt —
/// `apply_type` (`src/input/shortcuts/apply.rs`) gates on it so a typed
/// character never leaks into the focused terminal's PTY.
#[derive(Resource)]
pub(crate) struct RenameState {
    /// The edit buffer, pre-filled with the active window's current name.
    text: String,
}

impl RenameState {
    /// Opens a prompt pre-filled with `current_name` (the active window's
    /// current name, or empty if unset).
    pub(crate) fn new(current_name: String) -> Self {
        Self { text: current_name }
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
            BackgroundColor(RENAME_BAR_BG),
            GlobalZIndex(RENAME_Z),
            RenameBar,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(""),
                ui.text_font(FontSize::Px(RENAME_BAR_FONT_SIZE_PX)),
                TextColor(RENAME_BAR_FG),
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
    state: Res<RenameState>,
    bar_children: Query<&Children, With<RenameBar>>,
) {
    let Ok(mut node) = bar.single_mut() else {
        return;
    };
    node.display = Display::Flex;
    if let Ok(children) = bar_children.single() {
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                text.0 = format!("Rename window: {}\u{258f}", state.text);
            }
        }
    }
}

/// Consumes `RenameWindowRequest`, inserting `RenameState` pre-filled from
/// the active window's current name. A prompt already open is left
/// untouched — an in-flight rename is never silently reset by a later
/// request. Also refuses to open when a `ConfirmState` prompt is already up,
/// so a rename chord pressed while a kill-confirm is open is a no-op rather
/// than opening a second modal.
fn open_rename_prompt(
    mut commands: Commands,
    mut requests: MessageReader<RenameWindowRequest>,
    state: Option<Res<RenameState>>,
    confirm: Option<Res<ConfirmState>>,
    active_windows: Query<&MultiplexerWindow, With<ActiveMultiplexerWindow>>,
) {
    let requested = requests.read().next().is_some();
    if !requested || any_modal_open(confirm, state) {
        return;
    }
    let Ok(window) = active_windows.single() else {
        return;
    };
    commands.insert_resource(RenameState::new(window.name.clone().unwrap_or_default()));
}

fn handle_rename_input(
    mut commands: Commands,
    mut state: ResMut<RenameState>,
    mut events: MessageReader<KeyboardInput>,
    mut armed: Local<bool>,
    mut active_windows: Query<&mut MultiplexerWindow, With<ActiveMultiplexerWindow>>,
) {
    // NOTE: the bound key that opened the prompt (the RenameWindow shortcut)
    // is still in the shared KeyboardInput buffer; this reader has its own
    // cursor, so skip the open frame — drain past the opening key — or it is
    // appended to the pre-filled name.
    if !*armed {
        events.clear();
        *armed = true;
        return;
    }
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match state.apply_key(&ev.logical_key) {
            RenameStep::Continue => {}
            RenameStep::Submit => {
                if let Ok(mut window) = active_windows.single_mut() {
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
                *armed = false;
                break;
            }
            RenameStep::Cancel => {
                commands.remove_resource::<RenameState>();
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

    fn char_key(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn new_prefills_text_from_current_name() {
        let state = RenameState::new("nvim".to_string());
        assert_eq!(state.text, "nvim");
    }

    #[test]
    fn apply_key_escape_cancels() {
        let mut state = RenameState::new("nvim".to_string());
        assert_eq!(state.apply_key(&Key::Escape), RenameStep::Cancel);
    }

    #[test]
    fn apply_key_enter_submits() {
        let mut state = RenameState::new("nvim".to_string());
        assert_eq!(state.apply_key(&Key::Enter), RenameStep::Submit);
    }

    #[test]
    fn apply_key_char_appends_and_continues() {
        let mut state = RenameState::new(String::new());
        assert_eq!(state.apply_key(&char_key("a")), RenameStep::Continue);
        assert_eq!(state.apply_key(&char_key("b")), RenameStep::Continue);
        assert_eq!(state.text, "ab");
    }

    #[test]
    fn apply_key_backspace_pops_last_char() {
        let mut state = RenameState::new("nvim".to_string());
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
            .add_systems(
                Update,
                handle_rename_input.run_if(resource_exists::<RenameState>),
            );
        app
    }

    #[test]
    fn open_frame_skips_the_opening_key() {
        let mut app = input_app();
        spawn_active_window(&mut app, None);
        app.world_mut()
            .insert_resource(RenameState::new("nvim".to_string()));
        // The opening rename-window shortcut key is still in the shared
        // buffer when the prompt opens.
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(key_event(char_key(","), KeyCode::Comma));
        app.update();

        let state = app.world().resource::<RenameState>();
        assert_eq!(
            state.text, "nvim",
            "the opening key must not leak into the prefilled text"
        );
    }

    #[test]
    fn submit_sets_window_name() {
        let mut app = input_app();
        let window = spawn_active_window(&mut app, None);
        app.world_mut()
            .insert_resource(RenameState::new("build".to_string()));
        app.update(); // arms: drains the open frame

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
        app.world_mut()
            .insert_resource(RenameState::new("   ".to_string()));
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
        app.world_mut()
            .insert_resource(RenameState::new("something-else".to_string()));
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

    #[test]
    fn second_modal_refused() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<RenameWindowRequest>()
            .add_systems(Update, open_rename_prompt);
        let window = spawn_active_window(&mut app, Some("nvim"));
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(window));

        app.world_mut().write_message(RenameWindowRequest);
        app.update();

        assert!(
            app.world().get_resource::<RenameState>().is_none(),
            "a rename request must be refused while a ConfirmState prompt is open"
        );
    }
}
