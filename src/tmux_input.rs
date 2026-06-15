//! Forwards focused keyboard input to the active tmux pane via
//! `send-keys -t <pane>`, intercepting a fixed set of ozmux GUI chords. Replaces
//! the legacy `dispatch_focused_key` path for the tmux backend.

use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::tmux_picker::SessionPicker;
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::{CopyPrompt, CopyPromptState};
use bevy::input::ButtonState;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use ozmux_tmux::{
    ActivePane, CopyAction, CopyModeQueries, CopyQueryKind, Forwarded, KeyBindings, KeyMods,
    PromptKind, TmuxConnection, TmuxPane, bevy_key_to_tmux_name, copy_mode_dispatch, plan_forward,
    send_bytes_command, send_pane_keys_command, show_buffer_command,
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

/// One key's effect while in copy mode: a verbatim command to relay (if any),
/// whether to exit copy mode afterward, whether to bridge the tmux paste buffer
/// to the system clipboard, and a prompt to open (search/jump).
#[derive(Debug, Default, PartialEq)]
struct CopyOutcome {
    command: Option<String>,
    exit: bool,
    bridge: bool,
    prompt: Option<PromptKind>,
}

/// Maps a dispatched `CopyAction` to its `CopyOutcome`. The bound command runs
/// verbatim; ozmux adds the marker toggle / prompt. `bridge` is `true` for
/// non-pipe copy commands so the tmux paste buffer is mirrored to the system
/// clipboard via `show-buffer`; `false` for pipe commands (they already exfiltrate
/// externally, e.g. `pbcopy`, so bridging would read stale/foreign content).
fn outcome_of(action: CopyAction) -> CopyOutcome {
    match action {
        CopyAction::Relay(command) => CopyOutcome {
            command: Some(command),
            ..Default::default()
        },
        CopyAction::Copy {
            command,
            pipes,
            and_cancel,
        } => CopyOutcome {
            command: Some(command),
            exit: and_cancel,
            bridge: !pipes,
            ..Default::default()
        },
        CopyAction::Exit(command) => CopyOutcome {
            command: Some(command),
            exit: true,
            ..Default::default()
        },
        CopyAction::Prompt { kind } => CopyOutcome {
            prompt: Some(kind),
            ..Default::default()
        },
        CopyAction::Ignore => CopyOutcome::default(),
    }
}

fn forward_keys_to_tmux(
    mut commands: Commands,
    mut picker: ResMut<SessionPicker>,
    mut copy_prompt: ResMut<CopyPrompt>,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut clipboard: ResMut<Clipboard>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut copy_queries: ResMut<CopyModeQueries>,
    mut prefix_pending: Local<bool>,
    connection: NonSend<TmuxConnection>,
    keys: Res<ButtonInput<KeyCode>>,
    ime: Res<crate::input::ime::ImeState>,
    bindings: Res<KeyBindings>,
    active_pane: Option<Single<(Entity, &TmuxPane), With<ActivePane>>>,
    copy_modes: Query<(), With<CopyModeState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    // NOTE: while the picker is open it owns the keyboard; forwarding would
    // leak picker-navigation keys to the active tmux pane. Drain (don't replay).
    if picker.open {
        *prefix_pending = false;
        events.clear();
        return;
    }
    // NOTE: while the copy-mode prompt is open it owns the keyboard; the prompt's
    // own system handles raw keys. Drain here so no key leaks to tmux or the
    // prefix state machine.
    if copy_prompt.open.is_some() {
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
    let (active_entity, active_pane_id, target) = match active_pane {
        Some(single) => {
            let (entity, pane) = *single;
            (Some(entity), Some(pane.id), Some(format!("%{}", pane.id.0)))
        }
        None => (None, None, None),
    };

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

    // NOTE: this branch must return before plan_forward — a copy-mode entry
    // binding pressed while already in copy mode is relayed here (not
    // re-intercepted), which would otherwise re-insert CopyModeState each press.
    let in_copy_mode = active_entity.is_some_and(|e| copy_modes.get(e).is_ok());
    if in_copy_mode {
        let Some(client) = connection.client() else {
            return;
        };
        let handle = client.handle();
        for name in key_names {
            let outcome = outcome_of(copy_mode_dispatch(&bindings, &name));
            if let Some(cmd) = &outcome.command {
                match handle.send(cmd) {
                    Ok(_) if outcome.bridge => {
                        if let Some(pane_id) = active_pane_id {
                            match handle.send(&show_buffer_command()) {
                                Ok(buf_id) => {
                                    copy_queries.register(buf_id, pane_id, CopyQueryKind::Buffer);
                                }
                                Err(e) => {
                                    tracing::warn!(?e, "show-buffer send failed");
                                }
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(?e, "copy-mode relay send failed");
                        break;
                    }
                }
            }
            if outcome.exit
                && let Some(entity) = active_entity
            {
                commands.entity(entity).remove::<CopyModeState>();
            }
            if let Some(kind) = outcome.prompt
                && let Some(pane_id) = active_pane_id
            {
                copy_prompt.open = Some(CopyPromptState {
                    kind,
                    pane: pane_id,
                    text: String::new(),
                });
            }
        }
        return;
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
        let enters_copy_mode = matches!(&action, Forwarded::Run(cmd) if is_copy_mode_entry(cmd));
        let cmd = match action {
            Forwarded::Run(command) => command,
            Forwarded::Keys(names) => send_pane_keys_command(target, &names),
        };
        if let Err(e) = handle.send(&cmd) {
            tracing::warn!(?e, "tmux forward send failed");
            break;
        }
        if enters_copy_mode && let Some(entity) = active_entity {
            commands.entity(entity).insert(CopyModeState);
        }
    }
}

/// True when a resolved tmux command enters copy mode (`copy-mode`, with any
/// flags). ozmux intercepts these to insert `CopyModeState` alongside running
/// the command on tmux.
fn is_copy_mode_entry(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .is_some_and(|first| first == "copy-mode")
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
    use ozmux_tmux::PromptKind;

    #[test]
    fn relay_outcome_sends_command_only() {
        let o = outcome_of(CopyAction::Relay("send-keys -X cursor-down".into()));
        assert_eq!(o.command.as_deref(), Some("send-keys -X cursor-down"));
        assert!(!o.exit);
        assert!(!o.bridge);
        assert!(o.prompt.is_none());
    }

    #[test]
    fn copy_and_cancel_outcome_relays_exits_and_bridges() {
        let o = outcome_of(CopyAction::Copy {
            command: "send-keys -X copy-selection-and-cancel".into(),
            pipes: false,
            and_cancel: true,
        });
        assert_eq!(
            o.command.as_deref(),
            Some("send-keys -X copy-selection-and-cancel")
        );
        assert!(o.exit);
        assert!(o.bridge, "non-pipe copy must set bridge=true");
    }

    #[test]
    fn copy_without_cancel_does_not_exit_but_bridges() {
        let o = outcome_of(CopyAction::Copy {
            command: "send-keys -X copy-selection".into(),
            pipes: false,
            and_cancel: false,
        });
        assert!(!o.exit);
        assert!(o.command.is_some());
        assert!(o.bridge, "non-pipe copy must set bridge=true");
    }

    #[test]
    fn copy_pipe_does_not_bridge() {
        let o = outcome_of(CopyAction::Copy {
            command: "send-keys -X copy-pipe pbcopy".into(),
            pipes: true,
            and_cancel: false,
        });
        assert!(o.command.is_some());
        assert!(
            !o.bridge,
            "pipe copy must NOT bridge (pbcopy already handles it)"
        );
    }

    #[test]
    fn exit_outcome_relays_cancel_and_exits() {
        let o = outcome_of(CopyAction::Exit("send-keys -X cancel".into()));
        assert!(o.exit);
        assert!(!o.bridge);
        assert_eq!(o.command.as_deref(), Some("send-keys -X cancel"));
    }

    #[test]
    fn prompt_outcome_carries_kind_no_command() {
        let o = outcome_of(CopyAction::Prompt {
            kind: PromptKind::SearchForward,
        });
        assert!(o.command.is_none());
        assert!(!o.exit);
        assert!(!o.bridge);
        assert_eq!(o.prompt, Some(PromptKind::SearchForward));
    }

    #[test]
    fn ignore_outcome_is_empty() {
        let o = outcome_of(CopyAction::Ignore);
        assert!(o.command.is_none() && !o.exit && !o.bridge && o.prompt.is_none());
    }

    #[test]
    fn detects_copy_mode_entry_command() {
        assert!(is_copy_mode_entry("copy-mode"));
        assert!(is_copy_mode_entry("copy-mode -u"));
        assert!(is_copy_mode_entry("copy-mode -eu"));
        assert!(!is_copy_mode_entry("copy-selection"));
        assert!(!is_copy_mode_entry("new-window"));
    }

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
