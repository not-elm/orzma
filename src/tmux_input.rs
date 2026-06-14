//! Forwards focused keyboard input to tmux via `send-keys -K`, intercepting a
//! fixed set of ozmux GUI chords. Replaces the legacy `dispatch_focused_key`
//! path for the tmux backend.

use crate::tmux_picker::SessionPicker;
use bevy::input::ButtonState;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::prelude::*;
use ozmux_tmux::{KeyMods, TmuxConnection, bevy_key_to_tmux_name, send_keys_command};

/// Registers the tmux keyboard-forwarding system.
pub struct OzmuxTmuxInputPlugin;

impl Plugin for OzmuxTmuxInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, forward_keys_to_tmux);
    }
}

/// A GUI-level chord ozmux handles itself (never forwarded to tmux).
enum GuiChord {
    OpenPicker,
    Quit,
    Other,
}

fn forward_keys_to_tmux(
    mut picker: ResMut<SessionPicker>,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    connection: NonSend<TmuxConnection>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    // NOTE: while the picker is open it owns the keyboard; forwarding would
    // leak picker-navigation keys to the active tmux pane. Drain (don't replay).
    if picker.open {
        events.clear();
        return;
    }

    let mods = KeyMods {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        super_: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    };

    let mut names: Vec<String> = Vec::new();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if let Some(chord) = gui_chord(&ev.key_code, mods) {
            match chord {
                GuiChord::OpenPicker => picker.open = true,
                GuiChord::Quit => {
                    exit.write(AppExit::Success);
                }
                GuiChord::Other => {}
            }
            continue;
        }
        if let Some(name) = bevy_key_to_tmux_name(&ev.logical_key, mods) {
            names.push(name);
        }
    }

    if names.is_empty() {
        return;
    }
    let Some(client) = connection.client_name() else {
        return;
    };
    if let Some(c) = connection.client()
        && let Err(e) = c.handle().send(&send_keys_command(client, &names))
    {
        tracing::warn!(?e, "send-keys forward failed");
    }
}

/// Classifies a key event as a GUI chord (matched on physical `key_code` + the
/// `Super`/Cmd modifier — layout-stable). Any other `Super`-modified key is
/// swallowed (`Other`) so it is never forwarded (tmux has no Super modifier).
fn gui_chord(key_code: &KeyCode, mods: KeyMods) -> Option<GuiChord> {
    if !mods.super_ {
        return None;
    }
    if mods.shift && *key_code == KeyCode::KeyP {
        return Some(GuiChord::OpenPicker);
    }
    if !mods.shift && *key_code == KeyCode::KeyQ {
        return Some(GuiChord::Quit);
    }
    Some(GuiChord::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(shift: bool, super_: bool) -> KeyMods {
        KeyMods {
            ctrl: false,
            alt: false,
            shift,
            super_,
        }
    }

    #[test]
    fn cmd_shift_p_opens_picker() {
        assert!(matches!(
            gui_chord(&KeyCode::KeyP, m(true, true)),
            Some(GuiChord::OpenPicker)
        ));
    }

    #[test]
    fn cmd_q_quits() {
        assert!(matches!(
            gui_chord(&KeyCode::KeyQ, m(false, true)),
            Some(GuiChord::Quit)
        ));
    }

    #[test]
    fn other_super_chord_is_swallowed() {
        assert!(matches!(
            gui_chord(&KeyCode::KeyH, m(false, true)),
            Some(GuiChord::Other)
        ));
    }

    #[test]
    fn non_super_key_is_not_a_chord() {
        assert!(gui_chord(&KeyCode::KeyA, m(false, false)).is_none());
    }
}
