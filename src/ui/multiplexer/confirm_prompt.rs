//! orzma-owned confirm prompt: the kill-pane / kill-window shortcut actions
//! open this prompt (`OpenKillPaneConfirm` / `OpenKillWindowConfirm`). It
//! renders a client-side bottom-bar y/n prompt and fires the matching kill
//! request only on confirm.

use crate::font::TerminalUiFont;
use crate::input::InputPhase;
use crate::multiplexer::request::{
    KillPaneRequest, KillWindowRequest, OpenKillPaneConfirm, OpenKillWindowConfirm,
};
use crate::ui::multiplexer::modal::any_modal_open;
use crate::ui::multiplexer::rename_prompt::RenameState;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;

const CONFIRM_Z: i32 = 330;
/// Background color of the confirm bar: a deliberate, saturated accent so a
/// destructive kill-pane/kill-window prompt reads as distinct from ordinary
/// terminal output.
const CONFIRM_BAR_BG: Color = Color::srgb(0.16, 0.35, 0.60);
/// Foreground (text) color of the confirm bar, contrasting `CONFIRM_BAR_BG`.
const CONFIRM_BAR_FG: Color = Color::srgb(0.95, 0.95, 0.95);
/// Font size of the confirm bar's prompt text.
const CONFIRM_BAR_FONT_SIZE_PX: f32 = 12.0;

/// Registers the kill-pane / kill-window confirm prompt: the Open* message
/// intake, the y/n input handler, and the show/hide render systems.
pub(super) struct ConfirmPromptPlugin;

impl Plugin for ConfirmPromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<OpenKillPaneConfirm>()
            .add_message::<OpenKillWindowConfirm>()
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
                    hide_confirm_ui.run_if(resource_removed::<ConfirmState>),
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
}

impl ConfirmState {
    /// Builds the prompt state for a pending `KillPaneRequest`.
    pub(crate) fn kill_pane(pane: Entity) -> Self {
        Self {
            message: "kill pane? [y/N]".to_string(),
            action: ConfirmAction::KillPane(pane),
        }
    }

    /// Builds the prompt state for a pending `KillWindowRequest`.
    fn kill_window(window: Entity) -> Self {
        Self {
            message: "kill window? [y/N]".to_string(),
            action: ConfirmAction::KillWindow(window),
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
    /// Maps one key to a confirm/cancel decision: `y`/`Y`/Enter confirm,
    /// `n`/`N`/Escape cancel; any other key is ignored (returns `None`).
    fn classify(key: &Key) -> Option<Self> {
        match key {
            Key::Enter => Some(Self::Confirm),
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
            BackgroundColor(CONFIRM_BAR_BG),
            GlobalZIndex(CONFIRM_Z),
            ConfirmBar,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(""),
                ui.text_font(FontSize::Px(CONFIRM_BAR_FONT_SIZE_PX)),
                TextColor(CONFIRM_BAR_FG),
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
    bar_children: Query<&Children, With<ConfirmBar>>,
) {
    let Ok(mut node) = bar.single_mut() else {
        return;
    };
    node.display = Display::Flex;
    if let Ok(children) = bar_children.single() {
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                text.0 = state.message.clone();
            }
        }
    }
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
) {
    let pane_target = kill_pane.read().next().map(|msg| msg.pane);
    let window_target = kill_window.read().next().map(|msg| msg.window);
    if any_modal_open(state, rename) {
        return;
    }
    if let Some(pane) = pane_target {
        commands.insert_resource(ConfirmState::kill_pane(pane));
    } else if let Some(window) = window_target {
        commands.insert_resource(ConfirmState::kill_window(window));
    }
}

fn handle_confirm_input(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    mut armed: Local<bool>,
    state: Res<ConfirmState>,
) {
    // NOTE: the bound key that opened the prompt (e.g. the KillPane shortcut)
    // is still in the shared KeyboardInput buffer; this reader has its own
    // cursor, so skip the open frame — drain past the opening key — or it
    // could be read as the y/n answer (a shortcut bound to `y`/`n` would
    // self-answer instantly).
    if !*armed {
        events.clear();
        *armed = true;
        return;
    }
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match ConfirmStep::classify(&ev.logical_key) {
            None => {}
            Some(ConfirmStep::Confirm) => {
                match state.action {
                    ConfirmAction::KillPane(pane) => {
                        commands.trigger(KillPaneRequest { pane });
                    }
                    ConfirmAction::KillWindow(window) => {
                        commands.trigger(KillWindowRequest { window });
                    }
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
        assert_eq!(
            ConfirmStep::classify(&Key::Enter),
            Some(ConfirmStep::Confirm)
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
        app.world_mut()
            .insert_resource(RenameState::new("build".to_string()));
        let pane = app.world_mut().spawn_empty().id();
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
            .insert_resource(ConfirmState::kill_pane(pane));
        // First update: handle_confirm_input arms and drains the open frame.
        app.update();

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
            .insert_resource(ConfirmState::kill_window(window));
        app.update();

        app.world_mut().write_message(press(Key::Enter));
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
    fn cancel_dismisses_without_kill() {
        let mut app = test_app();
        let pane = app.world_mut().spawn_empty().id();
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(pane));
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
