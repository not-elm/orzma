//! Forwards focused keyboard and mouse-wheel input to the active tmux pane.
//! Keyboard forwarding intercepts a fixed set of ozmux GUI chords and copy-mode
//! entry commands. Mouse-wheel forwarding drives tmux copy-mode scroll when
//! the active pane is already in copy mode, and enters copy mode when the wheel
//! binding triggers a copy-mode entry command.

use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::input::InputPhase;
use crate::tmux_picker::SessionPicker;
use crate::ui::confirm_prompt::{ConfirmState, parse_confirm_before};
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::{CopyPrompt, CopyPromptState};
use bevy::input::ButtonState;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use ozmux_tmux::{
    ActivePane, CopyAction, CopyModeQueries, CopyQueryKind, Forwarded, KeyBindings, KeyMods,
    PromptKind, TmuxConnection, TmuxPane, bevy_key_to_tmux_name, copy_mode_dispatch, plan_forward,
    send_bytes_command, send_pane_keys_command, show_buffer_command,
};

/// Registers the tmux keyboard-forwarding and mouse-wheel systems.
pub struct OzmuxTmuxInputPlugin;

impl Plugin for OzmuxTmuxInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                forward_keys_to_tmux
                    .in_set(InputPhase::FocusedKey)
                    .run_if(on_message::<KeyboardInput>),
                forward_wheel_to_tmux
                    .in_set(InputPhase::Dispatch)
                    .run_if(on_message::<MouseWheel>),
            ),
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
    (mut copy_prompt, confirm_state): (ResMut<CopyPrompt>, Option<Res<ConfirmState>>),
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
    // NOTE: while the confirm-before prompt is open it owns the keyboard; its own
    // system reads the y/n answer. Drain here so no key leaks to tmux or the pane.
    if confirm_state.is_some() {
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
        if let Forwarded::Run(command) = &action
            && let Some((message, inner)) = parse_confirm_before(command)
        {
            commands.insert_resource(ConfirmState {
                message,
                command: inner,
            });
            // NOTE: the prompt now owns the keyboard — stop here so any further
            // actions decoded from this same frame are NOT sent to tmux (that
            // would bypass the confirmation the prompt is gating).
            break;
        }
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

/// The tmux key name for one mouse-wheel notch in the given direction.
fn wheel_key_name(up: bool) -> &'static str {
    if up { "WheelUpPane" } else { "WheelDownPane" }
}

/// The fallback copy-mode scroll command used when the copy table does not bind
/// the wheel key — ensures the wheel always scrolls while in copy mode.
fn fallback_scroll_command(up: bool) -> String {
    format!(
        "send-keys -X {}",
        if up { "scroll-up" } else { "scroll-down" }
    )
}

/// Forwards mouse-wheel events to the active tmux pane.
///
/// When the active pane is in copy mode each notch relays the copy table's
/// `WheelUpPane`/`WheelDownPane` binding (falling back to `scroll-up`/
/// `scroll-down` when unbound). When the active pane is not in copy mode each
/// notch dispatches against the root/prefix tables; if the resolved command
/// enters copy mode, `CopyModeState` is inserted on the pane entity.
fn forward_wheel_to_tmux(
    mut commands: Commands,
    mut wheel: MessageReader<MouseWheel>,
    connection: NonSend<TmuxConnection>,
    bindings: Res<KeyBindings>,
    picker: Res<SessionPicker>,
    copy_prompt: Res<CopyPrompt>,
    focused_webview: Res<FocusedWebview>,
    active_pane: Option<Single<(Entity, &TmuxPane), With<ActivePane>>>,
    copy_modes: Query<(), With<CopyModeState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let notches = collect_notches(&mut wheel);
    if notches.is_empty() {
        return;
    }
    // NOTE: a background scroll must not mutate tmux; mirror the keyboard path.
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if !focused {
        return;
    }
    // NOTE: while the picker or copy-mode prompt is open it owns the overlay;
    // scrolling/entering copy mode behind the modal would be invisible and wrong.
    if picker.open || copy_prompt.open.is_some() {
        return;
    }
    // When an inline webview holds focus it owns input; forwarding the wheel to
    // tmux too would scroll both. Parity with the keyboard path.
    if focused_webview.0.is_some() {
        return;
    }
    let Some(single) = active_pane else {
        return;
    };
    let (entity, pane) = *single;
    let target = format!("%{}", pane.id.0);
    let in_copy_mode = copy_modes.get(entity).is_ok();

    let Some(client) = connection.client() else {
        return;
    };
    let handle = client.handle();

    for up in notches {
        let key = wheel_key_name(up);
        if in_copy_mode {
            let cmd = match copy_mode_dispatch(&bindings, key) {
                CopyAction::Relay(c) | CopyAction::Copy { command: c, .. } => c,
                CopyAction::Exit(c) => {
                    if let Err(e) = handle.send(&c) {
                        tracing::warn!(?e, "copy-mode wheel relay send failed");
                        break;
                    }
                    commands.entity(entity).remove::<CopyModeState>();
                    continue;
                }
                CopyAction::Prompt { .. } | CopyAction::Ignore => fallback_scroll_command(up),
            };
            if let Err(e) = handle.send(&cmd) {
                tracing::warn!(?e, "copy-mode wheel relay send failed");
                break;
            }
        } else {
            let mut prefix = false;
            let actions = plan_forward(&mut prefix, &bindings, vec![key.to_string()]);
            for action in actions {
                let enters = matches!(&action, Forwarded::Run(cmd) if is_copy_mode_entry(cmd));
                let cmd = match action {
                    Forwarded::Run(command) => command,
                    Forwarded::Keys(names) => send_pane_keys_command(&target, &names),
                };
                if let Err(e) = handle.send(&cmd) {
                    tracing::warn!(?e, "tmux wheel forward send failed");
                    break;
                }
                if enters {
                    commands.entity(entity).insert(CopyModeState);
                }
            }
        }
    }
}

/// Per-frame cap on emitted wheel notches; one `send-keys` is dispatched per
/// notch, so an uncapped fast fling would flood the control connection.
const MAX_NOTCHES_PER_FRAME: usize = 10;

/// Drains all `MouseWheel` messages for this frame into a list of per-notch
/// up/down booleans. `Line` units contribute one bool per integer notch;
/// `Pixel` units contribute a single notch in the dominant direction.
///
/// NOTE: sub-1.0 `Line` deltas are intentionally dropped (no residual carry in
/// v1), and the total is capped at `MAX_NOTCHES_PER_FRAME` so a fast trackpad
/// fling cannot flood the control connection with `send-keys` commands.
fn collect_notches(wheel: &mut MessageReader<MouseWheel>) -> Vec<bool> {
    let mut out = Vec::new();
    for ev in wheel.read() {
        match ev.unit {
            MouseScrollUnit::Line => {
                let count = ev.y.abs() as i32;
                let up = ev.y > 0.0;
                for _ in 0..count {
                    out.push(up);
                }
            }
            MouseScrollUnit::Pixel => {
                if ev.y > 0.0 {
                    out.push(true);
                } else if ev.y < 0.0 {
                    out.push(false);
                }
            }
        }
    }
    out.truncate(MAX_NOTCHES_PER_FRAME);
    out
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
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::mouse::MouseScrollUnit;
    use ozmux_tmux::PromptKind;

    #[test]
    fn wheel_key_name_up_is_wheel_up_pane() {
        assert_eq!(wheel_key_name(true), "WheelUpPane");
    }

    #[test]
    fn wheel_key_name_down_is_wheel_down_pane() {
        assert_eq!(wheel_key_name(false), "WheelDownPane");
    }

    #[test]
    fn fallback_scroll_command_up() {
        assert_eq!(fallback_scroll_command(true), "send-keys -X scroll-up");
    }

    #[test]
    fn fallback_scroll_command_down() {
        assert_eq!(fallback_scroll_command(false), "send-keys -X scroll-down");
    }

    fn make_wheel_event(unit: MouseScrollUnit, y: f32) -> MouseWheel {
        MouseWheel {
            unit,
            x: 0.0,
            y,
            window: Entity::PLACEHOLDER,
        }
    }

    fn notches_from_events(evs: &[MouseWheel]) -> Vec<bool> {
        let mut app = App::new();
        app.add_message::<MouseWheel>();
        for ev in evs {
            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
                .write(*ev);
        }
        app.world_mut()
            .run_system_once(|mut reader: MessageReader<MouseWheel>| collect_notches(&mut reader))
            .unwrap()
    }

    #[test]
    fn collect_notches_line_up_two_notches() {
        let evs = [make_wheel_event(MouseScrollUnit::Line, 2.0)];
        assert_eq!(notches_from_events(&evs), vec![true, true]);
    }

    #[test]
    fn collect_notches_line_down_one_notch() {
        let evs = [make_wheel_event(MouseScrollUnit::Line, -1.0)];
        assert_eq!(notches_from_events(&evs), vec![false]);
    }

    #[test]
    fn collect_notches_pixel_up_one_notch() {
        let evs = [make_wheel_event(MouseScrollUnit::Pixel, 5.0)];
        assert_eq!(notches_from_events(&evs), vec![true]);
    }

    #[test]
    fn collect_notches_pixel_down_one_notch() {
        let evs = [make_wheel_event(MouseScrollUnit::Pixel, -5.0)];
        assert_eq!(notches_from_events(&evs), vec![false]);
    }

    #[test]
    fn collect_notches_pixel_zero_no_notch() {
        let evs = [make_wheel_event(MouseScrollUnit::Pixel, 0.0)];
        assert_eq!(notches_from_events(&evs), Vec::<bool>::new());
    }

    #[test]
    fn collect_notches_clamps_fast_fling() {
        let evs = [make_wheel_event(MouseScrollUnit::Line, 50.0)];
        let notches = notches_from_events(&evs);
        assert_eq!(notches.len(), MAX_NOTCHES_PER_FRAME);
        assert!(notches.iter().all(|&up| up));
    }

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
