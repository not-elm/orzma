//! Forwards focused keyboard input to the active tmux pane via
//! `send-keys -t <pane>`, intercepting a fixed set of ozmux GUI chords. Replaces
//! the legacy `dispatch_focused_key` path for the tmux backend.

use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::tmux_picker::SessionPicker;
use bevy::input::ButtonState;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use ozmux_tmux::{
    ActivePane, Forwarded, KeyBindings, KeyMods, TmuxConnection, TmuxPane, bevy_key_to_tmux_name,
    plan_forward, send_bytes_command, send_pane_keys_command,
};

/// Registers the tmux keyboard-forwarding system.
pub struct OzmuxTmuxInputPlugin;

impl Plugin for OzmuxTmuxInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            forward_keys_to_tmux
                .in_set(crate::input::InputPhase::FocusedKey)
                .run_if(on_message::<KeyboardInput>),
        );
    }
}

/// A GUI-level chord ozmux handles itself (never forwarded to tmux).
enum GuiChord {
    OpenPicker,
    Quit,
    Paste,
    Other,
}

const PASTE_CHUNK_BYTES: usize = 256;

fn forward_keys_to_tmux(
    mut picker: ResMut<SessionPicker>,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut clipboard: ResMut<Clipboard>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut prefix_pending: Local<bool>,
    connection: NonSend<TmuxConnection>,
    keys: Res<ButtonInput<KeyCode>>,
    ime: Res<crate::input::ime::ImeState>,
    bindings: Res<KeyBindings>,
    active_pane: Option<Single<&TmuxPane, With<ActivePane>>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    // NOTE: while the picker is open it owns the keyboard; forwarding would
    // leak picker-navigation keys to the active tmux pane. Drain (don't replay).
    if picker.open {
        *prefix_pending = false;
        events.clear();
        return;
    }
    // NOTE: drain (don't replay) while composing — forwarding preedit
    // navigation keys would both garble IME composition and double-send.
    if ime.is_composing() {
        *prefix_pending = false;
        events.clear();
        return;
    }
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if !focused {
        *prefix_pending = false;
        events.clear();
        return;
    }

    let mods = KeyMods {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        super_: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    };

    // When an inline webview holds focus it owns the keyboard (bevy_cef routes
    // keystrokes to it); forwarding to tmux too would double-send. Ctrl+Shift+Esc
    // releases focus back to the terminal. NOTE: in the current tmux backend the
    // webview-focus machinery is old-multiplexer-driven, so FocusedWebview is
    // usually None here; this handler is correct for when it is set.
    if focused_webview.0.is_some() {
        for ev in events.read() {
            if ev.state == ButtonState::Pressed
                && ev.key_code == KeyCode::Escape
                && mods.ctrl
                && mods.shift
            {
                focused_webview.0 = None;
                break;
            }
        }
        *prefix_pending = false;
        events.clear();
        return;
    }

    // The active pane (if any) is the forward/paste target. GUI chords below do
    // not need it (so quit/picker work before a pane is projected); tmux key
    // dispatch does.
    let target = active_pane
        .as_deref()
        .map(|active| format!("%{}", active.id.0));

    // Collect forwardable tmux key names in event order. Super-modified keys are
    // handled as GUI chords (Paste/Quit/Picker) or swallowed; none reach tmux.
    let mut key_names: Vec<String> = Vec::new();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if let Some(chord) = gui_chord(&ev.key_code, mods) {
            // A GUI action abandons any pending tmux prefix sequence.
            *prefix_pending = false;
            match chord {
                GuiChord::OpenPicker => picker.open = true,
                GuiChord::Quit => {
                    exit.write(AppExit::Success);
                }
                GuiChord::Paste => {
                    let Some(text) = clipboard.read() else {
                        continue;
                    };
                    if text.is_empty() {
                        continue;
                    }
                    let (Some(target), Some(client)) = (target.as_deref(), connection.client())
                    else {
                        continue;
                    };
                    let bytes = build_paste_bytes(&text, false);
                    for chunk in bytes.chunks(PASTE_CHUNK_BYTES) {
                        if let Err(e) = client.handle().send(&send_bytes_command(target, chunk)) {
                            tracing::warn!(?e, "paste send failed");
                            break;
                        }
                    }
                }
                GuiChord::Other => {}
            }
            continue;
        }
        if let Some(name) = bevy_key_to_tmux_name(&ev.logical_key, ev.key_code, mods) {
            key_names.push(name);
        }
    }

    // Dispatch the keys against the tmux bindings: bound keys run their command
    // verbatim, unbound keys forward to the active pane. NOTE: the bound command
    // acts on tmux's current pane; ozmux keeps that synced to the focused pane
    // via select-pane, and both travel this FIFO control connection in order.
    let actions = plan_forward(&mut prefix_pending, &bindings, key_names);
    if actions.is_empty() {
        return;
    }
    let (Some(target), Some(client)) = (target.as_deref(), connection.client()) else {
        return;
    };
    let handle = client.handle();
    for action in actions {
        let cmd = match action {
            Forwarded::Run(command) => command,
            Forwarded::Keys(names) => send_pane_keys_command(target, &names),
        };
        if let Err(e) = handle.send(&cmd) {
            tracing::warn!(?e, "tmux forward send failed");
            break;
        }
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
    if !mods.shift && *key_code == KeyCode::KeyV {
        return Some(GuiChord::Paste);
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
    fn cmd_v_is_paste() {
        assert!(matches!(
            gui_chord(&KeyCode::KeyV, m(false, true)),
            Some(GuiChord::Paste)
        ));
        assert!(matches!(
            gui_chord(&KeyCode::KeyV, m(true, true)),
            Some(GuiChord::Other)
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
