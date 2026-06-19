//! Keyboard and paste forwarding for `AppMode::Ozma`.
//!
//! `OzmaInputPlugin` registers `forward_keys_to_ozma`, the Ozma-mode
//! equivalent of `forward_keys_to_tmux`. Raw keys are encoded and dispatched
//! as `TerminalKeyInput` entity events; paste is written directly to the PTY
//! via `TerminalHandle::write`.

use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::input::InputPhase;
use crate::input::ime::ImeState;
use crate::input::shortcuts::ResolvedShortcuts;
use crate::ozma::AppMode;
use crate::picker::SessionPicker;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::OzmaTerminal;
use ozma_tty_engine::{
    Coalescer, PtyHandle, TerminalHandle, TerminalKey, TerminalKeyInput, TerminalModifiers,
};
use ozmux_configs::shortcuts::{Modifiers, ShortcutAction};

/// Registers keyboard and paste forwarding for `AppMode::Ozma`.
pub(crate) struct OzmaInputPlugin;

impl Plugin for OzmaInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            forward_keys_to_ozma
                .in_set(InputPhase::FocusedKey)
                .run_if(in_state(AppMode::Ozma))
                .run_if(on_message::<KeyboardInput>),
        );
    }
}

fn forward_keys_to_ozma(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut picker: ResMut<SessionPicker>,
    mut clipboard: ResMut<Clipboard>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut ozma_terminal: Query<
        (Entity, &mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        With<OzmaTerminal>,
    >,
    shortcuts: Res<ResolvedShortcuts>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    ime: Res<ImeState>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if picker.open {
        events.clear();
        return;
    }
    if ime.is_composing() {
        events.clear();
        return;
    }
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if !focused {
        events.clear();
        return;
    }

    let mods = Modifiers {
        ctrl: bevy_keys.pressed(KeyCode::ControlLeft) || bevy_keys.pressed(KeyCode::ControlRight),
        shift: bevy_keys.pressed(KeyCode::ShiftLeft) || bevy_keys.pressed(KeyCode::ShiftRight),
        alt: bevy_keys.pressed(KeyCode::AltLeft) || bevy_keys.pressed(KeyCode::AltRight),
        meta: bevy_keys.pressed(KeyCode::SuperLeft) || bevy_keys.pressed(KeyCode::SuperRight),
    };
    let term_mods = TerminalModifiers {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        meta: mods.meta,
    };

    if focused_webview.0.is_some() {
        for ev in events.read() {
            if ev.state == ButtonState::Pressed
                && shortcuts.is_release_inline_focus(ev.key_code, mods)
            {
                focused_webview.0 = None;
                break;
            }
        }
        events.clear();
        return;
    }

    let entity = {
        let Ok((e, ..)) = ozma_terminal.single_mut() else {
            events.clear();
            return;
        };
        e
    };

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if let Some(action) = shortcuts.match_gui_action(ev.key_code, mods) {
            match action {
                ShortcutAction::Quit => {
                    exit.write(AppExit::Success);
                }
                ShortcutAction::Paste => {
                    let Some(text) = clipboard.read() else {
                        continue;
                    };
                    if text.is_empty() {
                        continue;
                    }
                    let Ok((_, mut handle, mut pty, mut coalescer)) = ozma_terminal.single_mut()
                    else {
                        continue;
                    };
                    if !handle.is_at_bottom() {
                        handle.scroll_to_bottom(&mut coalescer);
                    }
                    let bracketed = handle.bracketed_paste_enabled();
                    let bytes = build_paste_bytes(&text, bracketed);
                    if let Err(e) = handle.write(&mut pty, &bytes) {
                        tracing::warn!(?e, "ozma paste write failed");
                    }
                }
                ShortcutAction::OpenPicker => {
                    picker.open = true;
                }
                ShortcutAction::ReleaseInlineFocus => {}
                ShortcutAction::DetachSession => {}
            }
            continue;
        }
        if mods.meta {
            continue;
        }
        let Some(key) = bevy_key_to_terminal_key(&ev.logical_key) else {
            continue;
        };
        commands.trigger(TerminalKeyInput {
            entity,
            key,
            modifiers: term_mods,
        });
    }
}

fn bevy_key_to_terminal_key(logical_key: &Key) -> Option<TerminalKey> {
    match logical_key {
        Key::Character(s) => Some(TerminalKey::Text(s.to_string())),
        Key::Space => Some(TerminalKey::Text(" ".to_string())),
        Key::Enter => Some(TerminalKey::Enter),
        Key::Backspace => Some(TerminalKey::Backspace),
        Key::Tab => Some(TerminalKey::Tab),
        Key::Escape => Some(TerminalKey::Escape),
        Key::Delete => Some(TerminalKey::Delete),
        Key::ArrowUp => Some(TerminalKey::ArrowUp),
        Key::ArrowDown => Some(TerminalKey::ArrowDown),
        Key::ArrowLeft => Some(TerminalKey::ArrowLeft),
        Key::ArrowRight => Some(TerminalKey::ArrowRight),
        Key::Home => Some(TerminalKey::Home),
        Key::End => Some(TerminalKey::End),
        Key::PageUp => Some(TerminalKey::PageUp),
        Key::PageDown => Some(TerminalKey::PageDown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_char_maps_to_text() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Character("a".into())),
            Some(TerminalKey::Text("a".to_string()))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Character("あ".into())),
            Some(TerminalKey::Text("あ".to_string()))
        );
    }

    #[test]
    fn space_maps_to_text() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Space),
            Some(TerminalKey::Text(" ".to_string()))
        );
    }

    #[test]
    fn control_keys_map_correctly() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Enter),
            Some(TerminalKey::Enter)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Backspace),
            Some(TerminalKey::Backspace)
        );
        assert_eq!(bevy_key_to_terminal_key(&Key::Tab), Some(TerminalKey::Tab));
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Escape),
            Some(TerminalKey::Escape)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Delete),
            Some(TerminalKey::Delete)
        );
    }

    #[test]
    fn navigation_keys_map_correctly() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowUp),
            Some(TerminalKey::ArrowUp)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowDown),
            Some(TerminalKey::ArrowDown)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowLeft),
            Some(TerminalKey::ArrowLeft)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowRight),
            Some(TerminalKey::ArrowRight)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Home),
            Some(TerminalKey::Home)
        );
        assert_eq!(bevy_key_to_terminal_key(&Key::End), Some(TerminalKey::End));
        assert_eq!(
            bevy_key_to_terminal_key(&Key::PageUp),
            Some(TerminalKey::PageUp)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::PageDown),
            Some(TerminalKey::PageDown)
        );
    }

    #[test]
    fn modifier_and_unrecognized_keys_return_none() {
        assert_eq!(bevy_key_to_terminal_key(&Key::Shift), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Control), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Alt), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Super), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::F1), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Insert), None);
    }
}
