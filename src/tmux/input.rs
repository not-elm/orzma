//! Forwards focused keyboard and mouse-wheel input to the active tmux pane.
//! Keyboard forwarding intercepts a fixed set of ozmux GUI chords and copy-mode
//! entry commands. Mouse-wheel forwarding accumulates events into cell-deltas
//! (so trackpad / high-resolution `Pixel` scrolling quantizes the same way the
//! native terminal path does), drives tmux copy-mode scroll when the active
//! pane is already in copy mode, and enters copy mode when the wheel binding
//! triggers a copy-mode entry command.

use super::pane_hit::tmux_pane_at_phys;
use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::configs::OzmuxConfigsResource;
use crate::inline_webview::{InlineWebview, PassthroughKeys, focused_inline_of, inline_hit_at};
use crate::input::InputPhase;
use crate::input::shortcuts::ResolvedShortcuts;
use crate::osc_webview::NonInteractive;
use crate::picker::SessionPicker;
use crate::ui::confirm_prompt::{ConfirmState, parse_confirm_before};
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::{CopyPrompt, CopyPromptState};
use crate::ui::rename_prompt::{RenameKind, RenamePrompt, RenameSubject};
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozmux_configs::shortcuts::{Modifiers, ShortcutAction};
use ozmux_tmux::{
    ActivePane, ActiveWindow, CopyAction, CopyModeQueries, CopyQueryKind, Forwarded, KeyBindings,
    KeyMods, PromptKind, TmuxConnection, TmuxPane, TmuxSession, TmuxWindow, bevy_key_to_tmux_name,
    copy_mode_dispatch, plan_forward, send_bytes_command, send_pane_keys_command,
    show_buffer_command,
};

/// Registers the tmux keyboard-forwarding and mouse-wheel systems.
pub(crate) struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxWheelAccumulator>().add_systems(
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
    (mut copy_prompt, confirm_state, rename): (
        ResMut<CopyPrompt>,
        Option<Res<ConfirmState>>,
        RenameParams,
    ),
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut clipboard: ResMut<Clipboard>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut copy_queries: ResMut<CopyModeQueries>,
    mut prefix_pending: Local<bool>,
    mut handles: Query<&mut TerminalHandle>,
    connection: NonSend<TmuxConnection>,
    (keys, ime, bindings, resolved): (
        Res<ButtonInput<KeyCode>>,
        Res<crate::input::ime::ImeState>,
        Res<KeyBindings>,
        Res<ResolvedShortcuts>,
    ),
    active_pane: Option<Single<(Entity, &TmuxPane), With<ActivePane>>>,
    copy_modes: Query<(), With<CopyModeState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    passthrough_keys: Query<&PassthroughKeys>,
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
    // NOTE: while the rename prompt is open it owns the keyboard; its own system
    // reads the typed text. Drain here so no key leaks to tmux or the pane.
    if rename.prompt.is_some() {
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
    let cfg_mods = Modifiers {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        meta: mods.super_,
    };

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

    // When an inline webview holds focus it owns the keyboard (bevy_cef routes
    // keystrokes to it); forwarding to tmux too would double-send. The configured
    // release-inline-focus chord releases focus back to the terminal. NOTE: under the tmux backend
    // `FocusedWebview` is live (set by the inline-click router, the control-plane
    // `SetFocus` op, and the focus-preservation arm in `sync_focused_webview`),
    // so this drain is load-bearing whenever an inline webview is focused —
    // removing it would double-send keystrokes to the page and the pane.
    if let Some(focused_entity) = focused_webview.0 {
        let pass_chords = passthrough_keys
            .get(focused_entity)
            .map(|pk| pk.0.as_slice())
            .unwrap_or(&[]);

        let mut pass_names: Vec<String> = Vec::new();
        for ev in events.read() {
            if ev.state != ButtonState::Pressed {
                continue;
            }
            if resolved.is_release_inline_focus(ev.key_code, cfg_mods) {
                focused_webview.0 = None;
                break;
            }
            if !pass_chords.is_empty()
                && pass_chords.iter().any(|c| {
                    c.code == ev.key_code
                        && c.ctrl == mods.ctrl
                        && c.shift == mods.shift
                        && c.alt == mods.alt
                        && c.logo == mods.super_
                })
                && let Some(name) = bevy_key_to_tmux_name(&ev.logical_key, ev.key_code, mods)
            {
                pass_names.push(name);
            }
        }

        if !pass_names.is_empty() {
            let actions = plan_forward(&mut prefix_pending, &bindings, pass_names);
            if let (Some(target), Some(client)) = (target.as_deref(), connection.client()) {
                let handle = client.handle();
                for action in actions {
                    let cmd = match action {
                        Forwarded::Run(cmd) => cmd,
                        Forwarded::Keys(names) => send_pane_keys_command(target, &names),
                    };
                    if let Err(e) = handle.send(&cmd) {
                        tracing::warn!(?e, "passthrough forward failed");
                        break;
                    }
                }
            }
        }

        *prefix_pending = false;
        events.clear();
        return;
    }

    // Collect forwardable tmux key names in event order. Super-modified keys are
    // matched against the configured ozmux shortcuts or swallowed; none reach tmux.
    let mut key_names: Vec<String> = Vec::new();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if let Some(action) = resolved.match_gui_action(ev.key_code, cfg_mods) {
            // A GUI action abandons any pending tmux prefix sequence.
            *prefix_pending = false;
            match action {
                ShortcutAction::OpenPicker => picker.open = true,
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
                ShortcutAction::ReleaseInlineFocus => {}
            }
            continue;
        }
        // NOTE: tmux/PTY has no Super modifier, so a Cmd-modified key that
        // matched no ozmux shortcut must be swallowed here, never forwarded.
        if mods.super_ {
            *prefix_pending = false;
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

    if !in_copy_mode
        && !key_names.is_empty()
        && let Some(entity) = active_entity
        && let Ok(mut handle) = handles.get_mut(entity)
        && handle.snap_to_bottom_vt_only()
    {
        handle.flush_emit(&mut commands, entity);
    }

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
            && let Some(kind) = RenameKind::parse(command)
            && let Some(subject) = resolve_rename_subject(kind, &rename)
        {
            commands.insert_resource(RenamePrompt::new(subject));
            // NOTE: the prompt now owns the keyboard — stop here so no further
            // actions from this frame are sent to tmux.
            break;
        }
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

/// True when a resolved tmux command can enter copy mode, so ozmux inserts
/// `CopyModeState` alongside running it on tmux.
///
/// Matches a bare `copy-mode` token anywhere in the command, not just at the
/// front: tmux's default mouse-wheel bindings enter copy mode through a
/// conditional (e.g. `WheelUpPane` is
/// `if-shell -F "…" { send-keys -M } { copy-mode -e }`), so a first-token check
/// would miss them. A false positive — the conditional taking the non-copy-mode
/// branch on an alt-screen / mouse-reporting pane — is harmless: the copy-mode
/// refresh loop removes `CopyModeState` again on the first `#{pane_in_mode} == 0`
/// state reply. The `copy-mode` token is matched whole-word, so quoted format
/// strings and the `copy-mode-vi` table name do not trip it.
fn is_copy_mode_entry(command: &str) -> bool {
    command.split_whitespace().any(|token| token == "copy-mode")
}

/// Builds a pane-targeted copy-mode scroll command for one wheel notch:
/// `send-keys -X -t %<id> -N <lines> scroll-up|scroll-down`.
///
/// ozmux drives copy-mode wheel scrolling with this single, targeted command
/// rather than relaying tmux's copy-table `WheelUpPane`/`WheelDownPane`
/// bindings, which are `select-pane \; send-keys …` sequences: relaying a
/// sequence as one control-mode command makes tmux emit an extra reply block
/// the protocol client cannot correlate (the `no pending command` storm).
fn scroll_command(target: &str, up: bool, lines: u32) -> String {
    format!(
        "send-keys -X -t {target} -N {lines} {}",
        if up { "scroll-up" } else { "scroll-down" }
    )
}

/// Carries the sub-notch wheel remainder (in cells) across frames for the tmux
/// scroll path, plus the pane the residual was earned on so switching panes
/// clears stale momentum. Mirrors the native terminal's `WheelAccumulator` so
/// trackpad / high-resolution `Pixel` scrolling quantizes identically rather
/// than firing a full notch per raw event.
#[derive(Resource, Default)]
struct TmuxWheelAccumulator {
    residual_cells: f32,
    last_pane: Option<Entity>,
}

/// A focused inline webview claiming the wheel: the child to forward to and
/// the pointer in its webview-local DIP.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TmuxInlineWheelTarget {
    child: Entity,
    position_dip: Vec2,
}

/// Wheel-routing params bundled to stay within Bevy's system-parameter limit.
/// `browsers` is optional so CEF-less tests construct the system.
#[derive(SystemParam)]
struct TmuxInlineWheelParams<'w, 's> {
    focused_webview: Res<'w, FocusedWebview>,
    inline_parents: Query<'w, 's, &'static ChildOf, With<InlineWebview>>,
    panes: Query<
        'w,
        's,
        (
            Entity,
            &'static TmuxPane,
            &'static ComputedNode,
            &'static UiGlobalTransform,
        ),
    >,
    children: Query<'w, 's, &'static Children>,
    inline: Query<'w, 's, (&'static InlineWebview, Has<NonInteractive>)>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    browsers: Option<NonSend<'w, Browsers>>,
}

/// Rename-interception params bundled to stay within Bevy's system-parameter
/// limit (`forward_keys_to_tmux` is already at 16 top-level params). `prompt`
/// gates the keyboard while a rename is open; the queries resolve the target
/// captured at prompt-open.
#[derive(SystemParam)]
struct RenameParams<'w, 's> {
    prompt: Option<Res<'w, RenamePrompt>>,
    active_window: Query<'w, 's, &'static TmuxWindow, With<ActiveWindow>>,
    session: Query<'w, 's, &'static TmuxSession>,
}

/// Resolves the rename target (id + current name) from ECS for `kind`, or `None`
/// when no active window / attached session exists (the binding then forwards
/// verbatim, as before).
fn resolve_rename_subject(kind: RenameKind, rename: &RenameParams) -> Option<RenameSubject> {
    match kind {
        RenameKind::Window => {
            let w = rename.active_window.single().ok()?;
            Some(RenameSubject::Window {
                id: w.id,
                current_name: w.name.clone(),
            })
        }
        RenameKind::Session => {
            let s = rename.session.single().ok()?;
            Some(RenameSubject::Session {
                id: s.id,
                current_name: s.name.clone(),
            })
        }
    }
}

/// Resolves the focused inline webview under the pointer, or `None` (the tmux
/// path runs). `Some` only when `FocusedWebview` holds an inline child of the
/// pane under the pointer AND the pointer is over that child's rect — the tmux
/// analog of native `resolve_inline_wheel_target`.
fn resolve_tmux_inline_wheel_target(
    params: &TmuxInlineWheelParams,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale_factor: f32,
) -> Option<TmuxInlineWheelTarget> {
    let (terminal, _pane_id, local_phys) = tmux_pane_at_phys(&params.panes, cursor_phys)?;
    let focused_child = focused_inline_of(
        Some(&params.focused_webview),
        &params.inline_parents,
        Some(terminal),
    )?;
    let overlays = params.overlay_rects.get(terminal).ok()?;
    let hit = inline_hit_at(
        &params.children,
        &params.inline,
        overlays,
        terminal,
        local_phys,
        cell_w_phys,
        cell_h_phys,
        scale_factor,
    )?;
    (hit.child == focused_child).then_some(TmuxInlineWheelTarget {
        child: hit.child,
        position_dip: hit.local_dip,
    })
}

/// Converts one `MouseWheel` event to the RAW CEF wheel delta (`Line → ×120`,
/// `Pixel` unchanged, NO sign flip) — identical to native `inline_wheel_delta`.
fn tmux_inline_wheel_delta(unit: MouseScrollUnit, x: f32, y: f32) -> Vec2 {
    match unit {
        MouseScrollUnit::Line => Vec2::new(x, y) * 120.0,
        MouseScrollUnit::Pixel => Vec2::new(x, y),
    }
}

/// Forwards mouse-wheel events to the active tmux pane.
///
/// A focused inline webview under the pointer claims the wheel first
/// (`resolve_tmux_inline_wheel_target`): each event is forwarded RAW to that
/// child's CEF browser and dropped before the tmux accumulator.
///
/// Otherwise events are accumulated into a cell-delta and quantized into integer
/// notches. When the pane is in copy mode (`CopyModeState`), a single targeted
/// `send-keys -X scroll-up|scroll-down` is sent for the total notch count.
/// Outside copy mode, the local `TerminalHandle` VT scrollback is scrolled
/// directly via `scroll_vt_only` + `flush_emit` — no tmux IPC needed.
fn forward_wheel_to_tmux(
    mut commands: Commands,
    mut wheel: MessageReader<MouseWheel>,
    mut accumulator: ResMut<TmuxWheelAccumulator>,
    mut handles: Query<&mut TerminalHandle>,
    inline: TmuxInlineWheelParams,
    connection: NonSend<TmuxConnection>,
    picker: Res<SessionPicker>,
    copy_prompt: Res<CopyPrompt>,
    rename_prompt: Option<Res<RenamePrompt>>,
    configs: Res<OzmuxConfigsResource>,
    metrics: Res<TerminalCellMetricsResource>,
    active_pane: Option<Single<(Entity, &TmuxPane), With<ActivePane>>>,
    copy_modes: Query<(), With<CopyModeState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let dpr = window.scale_factor().max(0.5);
    let cell_h_logical = (metrics.metrics.line_height_phys.floor() / dpr).max(1.0);
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_phys = window.cursor_position().map(|c| c * dpr);

    let target = cursor_phys
        .and_then(|c| resolve_tmux_inline_wheel_target(&inline, c, cell_w_phys, cell_h_phys, dpr));

    let Some(delta_cells) = aggregate_tmux_wheel_cells(
        &mut wheel,
        target,
        inline.browsers.as_deref(),
        cell_h_logical,
    ) else {
        // NOTE: an all-inline frame must reset the residual — leaving carried
        // cells behind would let tmux momentum lurch when the wheel later resumes
        // over the terminal. An empty frame (target None) leaves it intact.
        if target.is_some() {
            accumulator.residual_cells = 0.0;
        }
        return;
    };
    // NOTE: a background scroll must not mutate tmux; mirror the keyboard path.
    // Reset any carried remainder on every guarded early-return so momentum
    // can't accumulate behind a modal / unfocused window and lurch on resume.
    // focused_webview is NOT a guard here — the pointer-gated target above owns
    // webview scrolling, so a focused webview must not steal terminal wheel.
    if !window.focused || picker.open || copy_prompt.open.is_some() || rename_prompt.is_some() {
        accumulator.residual_cells = 0.0;
        return;
    }
    let Some(single) = active_pane else {
        accumulator.residual_cells = 0.0;
        return;
    };
    let (entity, pane) = *single;
    let raw_notches = consume_wheel_notches(
        &mut accumulator,
        entity,
        delta_cells,
        configs.mouse.cells_per_notch,
    );
    if raw_notches == 0 {
        return;
    }
    let up = raw_notches > 0;
    let count = (raw_notches.unsigned_abs() as usize).min(MAX_NOTCHES_PER_FRAME);
    let target = format!("%{}", pane.id.0);
    let lines = configs.mouse.lines_per_notch;
    let in_copy_mode = copy_modes.get(entity).is_ok();
    let total_lines = count as u32 * lines;

    let Some(client) = connection.client() else {
        return;
    };
    let tmux = client.handle();

    if in_copy_mode {
        let cmd = scroll_command(&target, up, total_lines);
        if let Err(e) = tmux.send(&cmd) {
            tracing::warn!(?e, "copy-mode wheel scroll send failed");
        }
    } else {
        let Ok(mut handle) = handles.get_mut(entity) else {
            return;
        };
        let total_delta = if up {
            total_lines as i32
        } else {
            -(total_lines as i32)
        };
        handle.scroll_vt_only(total_delta);
        handle.flush_emit(&mut commands, entity);
    }
}

/// Per-frame cap on emitted wheel notches; one `send-keys` is dispatched per
/// notch, so an uncapped fast fling would flood the control connection.
const MAX_NOTCHES_PER_FRAME: usize = 10;

/// Drains the frame's `MouseWheel` into a signed cell-delta for the tmux path,
/// forking inline-routed events to CEF first. Returns `None` when no
/// terminal-bound events arrived (all forwarded inline, or empty). The tmux
/// analog of native `aggregate_wheel_delta`.
///
/// Terminal-bound events: `Line` units contribute `ev.y` directly (positive =
/// scroll up / toward older lines); `Pixel` units (macOS trackpads,
/// high-resolution wheels) are divided by the cell height so a fixed pixel
/// travel maps to a consistent number of lines. Inline-routed events are
/// forwarded RAW to CEF via `tmux_inline_wheel_delta` (no sign flip).
fn aggregate_tmux_wheel_cells(
    wheel: &mut MessageReader<MouseWheel>,
    target: Option<TmuxInlineWheelTarget>,
    browsers: Option<&Browsers>,
    cell_h_logical: f32,
) -> Option<f32> {
    let mut delta = 0.0f32;
    let mut had_terminal_input = false;
    for ev in wheel.read() {
        if let Some(target) = target {
            if let Some(browsers) = browsers {
                browsers.send_mouse_wheel(
                    &target.child,
                    target.position_dip,
                    tmux_inline_wheel_delta(ev.unit, ev.x, ev.y),
                );
            }
            continue;
        }
        had_terminal_input = true;
        delta += match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y / cell_h_logical,
        };
    }
    had_terminal_input.then_some(delta)
}

/// Adds this frame's cell-delta to the accumulator and returns the signed
/// integer notch count to emit (positive = up), carrying the sub-notch
/// remainder to the next frame. Resets the residual on pane change or sign flip
/// — both signal that prior momentum is stale. Returns `0` until the residual
/// crosses one `cells_per_notch` threshold.
fn consume_wheel_notches(
    accumulator: &mut TmuxWheelAccumulator,
    pane: Entity,
    delta_cells: f32,
    cells_per_notch: f32,
) -> i32 {
    if accumulator.last_pane != Some(pane) {
        accumulator.residual_cells = 0.0;
        accumulator.last_pane = Some(pane);
    } else if accumulator.residual_cells != 0.0
        && accumulator.residual_cells.signum() != delta_cells.signum()
    {
        accumulator.residual_cells = 0.0;
    }
    let threshold = cells_per_notch.max(f32::EPSILON);
    accumulator.residual_cells += delta_cells;
    let notches = (accumulator.residual_cells / threshold).trunc() as i32;
    if notches != 0 {
        accumulator.residual_cells -= notches as f32 * threshold;
    }
    notches
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::mouse::MouseScrollUnit;
    use ozmux_tmux::PromptKind;

    #[test]
    fn scroll_command_up_is_targeted_and_repeated() {
        assert_eq!(
            scroll_command("%3", true, 3),
            "send-keys -X -t %3 -N 3 scroll-up"
        );
    }

    #[test]
    fn scroll_command_down_is_targeted_and_repeated() {
        assert_eq!(
            scroll_command("%3", false, 5),
            "send-keys -X -t %3 -N 5 scroll-down"
        );
    }

    fn make_wheel_event(unit: MouseScrollUnit, y: f32) -> MouseWheel {
        MouseWheel {
            unit,
            x: 0.0,
            y,
            window: Entity::PLACEHOLDER,
        }
    }

    fn cells_from_events(cell_h: f32, evs: &[MouseWheel]) -> Option<f32> {
        let mut app = App::new();
        app.add_message::<MouseWheel>();
        for ev in evs {
            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
                .write(*ev);
        }
        app.world_mut()
            .run_system_once(
                move |mut reader: MessageReader<MouseWheel>,
                      browsers: Option<NonSend<Browsers>>| {
                    aggregate_tmux_wheel_cells(&mut reader, None, browsers.as_deref(), cell_h)
                },
            )
            .unwrap()
    }

    #[test]
    fn aggregate_line_units_pass_through_as_cells() {
        let evs = [make_wheel_event(MouseScrollUnit::Line, 2.0)];
        assert_eq!(cells_from_events(16.0, &evs), Some(2.0));
    }

    #[test]
    fn aggregate_pixel_units_divide_by_cell_height() {
        let evs = [make_wheel_event(MouseScrollUnit::Pixel, 8.0)];
        assert_eq!(cells_from_events(16.0, &evs), Some(0.5));
    }

    #[test]
    fn aggregate_sums_a_frames_events() {
        let evs = [
            make_wheel_event(MouseScrollUnit::Pixel, 4.0),
            make_wheel_event(MouseScrollUnit::Pixel, 4.0),
        ];
        assert_eq!(cells_from_events(16.0, &evs), Some(0.5));
    }

    #[test]
    fn aggregate_no_events_is_none() {
        assert_eq!(cells_from_events(16.0, &[]), None);
    }

    fn make_tmux_wheel_app() -> (App, Entity, Entity) {
        use bevy::window::WindowResolution;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<FocusedWebview>();

        // Pane host node at window center (400, 300), size 800x600 → top-left
        // at (0, 0). Rect rows 2..12, cols 3..43 → phys y 32..192, x 24..344 at
        // 8x16 px.
        let mut overlays = TerminalOverlays::default();
        overlays.rects[0] = IVec4::new(2, 3, 10, 40);
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: ozmux_tmux::PaneId(1),
                    dims: CellDims {
                        width: 100,
                        height: 37,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                ComputedNode {
                    size: Vec2::new(800.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(400.0, 300.0),
                overlays,
            ))
            .id();
        let child = app
            .world_mut()
            .spawn((
                ChildOf(pane),
                InlineWebview {
                    view_id: "inline".into(),
                    instance_id: None,
                    slot: 0,
                },
            ))
            .id();

        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        (app, pane, child)
    }

    fn run_resolve_wheel_target(app: &mut App, cursor_phys: Vec2) -> Option<TmuxInlineWheelTarget> {
        app.world_mut()
            .run_system_once(move |params: TmuxInlineWheelParams| {
                resolve_tmux_inline_wheel_target(&params, cursor_phys, 8.0, 16.0, 1.0)
            })
            .unwrap()
    }

    #[test]
    fn wheel_target_resolves_when_focused_inline_under_pointer() {
        let (mut app, _pane, child) = make_tmux_wheel_app();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        assert_eq!(
            run_resolve_wheel_target(&mut app, Vec2::new(40.0, 48.0)).map(|t| t.child),
            Some(child)
        );
    }

    #[test]
    fn wheel_target_none_off_rect() {
        let (mut app, _pane, child) = make_tmux_wheel_app();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        assert_eq!(
            run_resolve_wheel_target(&mut app, Vec2::new(400.0, 400.0)),
            None
        );
    }

    #[test]
    fn wheel_target_none_when_unfocused() {
        let (mut app, _pane, _child) = make_tmux_wheel_app();
        assert_eq!(
            run_resolve_wheel_target(&mut app, Vec2::new(40.0, 48.0)),
            None
        );
    }

    #[test]
    fn consume_notches_quantizes_against_threshold() {
        let pane = Entity::from_raw_u32(1).unwrap();
        let mut acc = TmuxWheelAccumulator::default();
        // 0.4 cells is below the 0.5 threshold — no notch yet, remainder carried.
        assert_eq!(consume_wheel_notches(&mut acc, pane, 0.4, 0.5), 0);
        // +0.4 → 0.8 cells, crosses one threshold; 0.3 carries.
        assert_eq!(consume_wheel_notches(&mut acc, pane, 0.4, 0.5), 1);
    }

    #[test]
    fn consume_notches_is_signed_for_direction() {
        let pane = Entity::from_raw_u32(1).unwrap();
        let mut acc = TmuxWheelAccumulator::default();
        assert_eq!(consume_wheel_notches(&mut acc, pane, -1.5, 0.5), -3);
    }

    #[test]
    fn consume_notches_resets_residual_on_pane_change() {
        let a = Entity::from_raw_u32(1).unwrap();
        let b = Entity::from_raw_u32(2).unwrap();
        let mut acc = TmuxWheelAccumulator::default();
        assert_eq!(consume_wheel_notches(&mut acc, a, 0.4, 0.5), 0);
        // Switching panes drops the carried 0.4 cells; 0.4 alone is sub-threshold.
        assert_eq!(consume_wheel_notches(&mut acc, b, 0.4, 0.5), 0);
    }

    #[test]
    fn consume_notches_resets_residual_on_sign_flip() {
        let pane = Entity::from_raw_u32(1).unwrap();
        let mut acc = TmuxWheelAccumulator::default();
        assert_eq!(consume_wheel_notches(&mut acc, pane, 0.4, 0.5), 0);
        // A reversed direction discards the upward remainder, then 0.4 down is
        // still sub-threshold.
        assert_eq!(consume_wheel_notches(&mut acc, pane, -0.4, 0.5), 0);
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

    #[test]
    fn detects_copy_mode_entry_inside_wheel_conditional() {
        // tmux's default `WheelUpPane` root binding enters copy mode through an
        // if-shell conditional, not a leading `copy-mode` token. ozmux must still
        // recognize the entry so it inserts `CopyModeState` (the refresh loop
        // removes it again if the conditional took the send-keys branch).
        assert!(is_copy_mode_entry(
            "if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -e }"
        ));
        // A wheel that only forwards a mouse event to the app is not an entry.
        assert!(!is_copy_mode_entry("send-keys -M"));
        assert!(!is_copy_mode_entry("send-keys -X scroll-up"));
    }
}
