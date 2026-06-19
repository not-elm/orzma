//! Host-side input for `AppMode::Ozma`: maintains the crate's `InputDisabled`
//! marker from the coarse guards (picker, IME, focus, webview), and handles the
//! application-level GUI shortcuts the terminal crate does not own (Quit,
//! OpenPicker, DetachSession, ReleaseInlineFocus). Raw-key forwarding and paste
//! are owned by `ozma_terminal`'s dispatcher and `PasteAction`.

use crate::input::InputPhase;
use crate::input::ime::ImeState;
use crate::input::shortcuts::ResolvedShortcuts;
use crate::ozma::AppMode;
use crate::picker::SessionPicker;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{InputDisabled, OzmaTerminal, OzmaTerminalInputSet};
use ozmux_configs::shortcuts::{Modifiers, ShortcutAction};

/// Registers the host-side input systems for `AppMode::Ozma`.
pub(crate) struct OzmaHostInputPlugin;

impl Plugin for OzmaHostInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            maintain_input_disabled
                .before(OzmaTerminalInputSet)
                .run_if(in_state(AppMode::Ozma)),
        )
        .add_systems(
            Update,
            app_shortcut_handler
                .in_set(InputPhase::FocusedKey)
                .run_if(in_state(AppMode::Ozma))
                .run_if(on_message::<KeyboardInput>),
        );
    }
}

fn maintain_input_disabled(
    mut commands: Commands,
    picker: Res<SessionPicker>,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminals: Query<(Entity, Has<InputDisabled>), With<OzmaTerminal>>,
) {
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    let disable = should_disable_input(
        picker.open,
        ime.is_composing(),
        focused,
        focused_webview.0.is_some(),
    );
    for (entity, has) in terminals.iter() {
        if disable && !has {
            commands.entity(entity).insert(InputDisabled);
        } else if !disable && has {
            commands.entity(entity).remove::<InputDisabled>();
        }
    }
}

fn app_shortcut_handler(
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut picker: ResMut<SessionPicker>,
    mut focused_webview: ResMut<FocusedWebview>,
    shortcuts: Res<ResolvedShortcuts>,
    ime: Res<ImeState>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if picker.open || ime.is_composing() || !focused {
        events.clear();
        return;
    }
    let mods = current_modifiers(&bevy_keys);
    let webview_focused = focused_webview.0.is_some();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if webview_focused && shortcuts.is_release_inline_focus(ev.key_code, mods) {
            focused_webview.0 = None;
            continue;
        }
        let Some(action) = shortcuts.match_gui_action(ev.key_code, mods) else {
            continue;
        };
        if gui_action_suppressed_by_webview(webview_focused, action) {
            continue;
        }
        match action {
            ShortcutAction::Quit => {
                exit.write(AppExit::Success);
            }
            ShortcutAction::OpenPicker => {
                picker.open = true;
            }
            ShortcutAction::DetachSession => {}
            ShortcutAction::Paste | ShortcutAction::ReleaseInlineFocus => {}
        }
    }
}

fn should_disable_input(
    picker_open: bool,
    composing: bool,
    window_focused: bool,
    webview_focused: bool,
) -> bool {
    picker_open || composing || !window_focused || webview_focused
}

fn gui_action_suppressed_by_webview(webview_focused: bool, action: ShortcutAction) -> bool {
    webview_focused && action != ShortcutAction::ReleaseInlineFocus
}

fn current_modifiers(keys: &ButtonInput<KeyCode>) -> Modifiers {
    Modifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disables_input_on_any_guard() {
        assert!(!should_disable_input(false, false, true, false));
        assert!(should_disable_input(true, false, true, false));
        assert!(should_disable_input(false, true, true, false));
        assert!(should_disable_input(false, false, false, false));
        assert!(should_disable_input(false, false, true, true));
    }

    #[test]
    fn webview_focus_suppresses_all_but_release() {
        assert!(gui_action_suppressed_by_webview(true, ShortcutAction::Quit));
        assert!(gui_action_suppressed_by_webview(
            true,
            ShortcutAction::OpenPicker
        ));
        assert!(gui_action_suppressed_by_webview(
            true,
            ShortcutAction::DetachSession
        ));
        assert!(!gui_action_suppressed_by_webview(
            true,
            ShortcutAction::ReleaseInlineFocus
        ));
        assert!(!gui_action_suppressed_by_webview(
            false,
            ShortcutAction::Quit
        ));
    }
}
