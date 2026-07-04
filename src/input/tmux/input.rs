//! Forwards focused keyboard and mouse-wheel input to the active tmux pane.
//! Keyboard forwarding dispatches a fixed set of ozmux GUI chords as
//! per-command action events (`crate::action::tmux`) and copy-mode
//! entry commands; while a pane is in copy mode, keys resolve against the
//! shared config-driven key table (`crate::action::vi::ResolvedCopyModeKeys`)
//! and fire the shared VI events for `crate::action::vi::tmux_mode` to apply;
//! unmatched keys forward straight to the pane in one `SendPaneKeys` batch
//! per frame. Mouse-wheel forwarding handles only the
//! cases `crate::input::mouse::wheel::dispatch_mouse_wheel` does not own (it
//! now runs on every tmux pane, gated off solely by `MouseDisabled`): an
//! inline webview under the pointer (forwarded to CEF), a copy-mode pane (a
//! targeted `send-keys -X scroll-up|scroll-down`), and the alt-screen
//! residual where ozma's viewport scroll would no-op (cursor-key
//! `send-keys`). Events accumulate into cell-deltas (so trackpad /
//! high-resolution `Pixel` scrolling quantizes the same way the native
//! terminal path does); every other case is ceded to ozma.

use super::pane_hit::tmux_pane_at_phys;
use crate::action::tmux::{
    KillPaneRequest, KillWindowRequest, NewWindowRequest, NextWindowRequest, PreviousWindowRequest,
    RenameSessionRequest, RenameWindowRequest, SelectPaneRequest, SelectWindowRequest,
    SplitPaneRequest, ZoomPaneRequest,
};
use crate::action::vi::{ResolvedCopyModeKeys, trigger_copy_mode_action};
use crate::app_mode::{AppMode, TmuxActiveSet};
use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::input::shortcuts::{
    LeaderGate, LeaderPhase, LeaderStep, Shortcuts, clear_leader_phase, step_leader,
};
use crate::session::tmux::request_detach;
use crate::ui::copy_mode::{CopyModeState, EnterCopyModeActionEvent};
use crate::ui::copy_search::CopyPrompt;
use crate::ui::tmux::confirm_prompt::ConfirmState;
use crate::ui::tmux::rename_prompt::RenamePrompt;
use crate::webview_pointer::{webview_wheel_delta, webview_wheel_target};
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::time::Real;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use ozma_tty_engine::{TermMode, TerminalHandle};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozma_webview::{ForwardKeys, NonInteractive, Webview};
use ozmux_configs::shortcuts::{
    Modifiers, PaneDirection as CfgPaneDirection, ShortcutAction,
    SplitOrientation as CfgSplitOrientation,
};
use ozmux_tmux::{
    ActivePane, ActiveWindow, KeyMods, PaneDirection, SendBytes, SendPaneKeys, SplitDirection,
    TmuxClient, TmuxCommand, TmuxPane, TmuxSession, TmuxWindow, bevy_key_to_tmux_name,
};

/// Registers the tmux keyboard-forwarding and mouse-wheel systems.
pub(super) struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxWheelAccumulator>().add_systems(
            Update,
            (
                forward_keys_to_tmux
                    .in_set(InputPhase::FocusedKey)
                    .in_set(LeaderGate::Advance)
                    .run_if(in_state(AppMode::Tmux))
                    .run_if(on_message::<KeyboardInput>),
                forward_wheel_to_tmux
                    .in_set(InputPhase::Dispatch)
                    .run_if(on_message::<MouseWheel>),
            )
                .in_set(TmuxActiveSet),
        );
    }
}

const PASTE_CHUNK_BYTES: usize = 256;

fn forward_keys_to_tmux(
    mut commands: Commands,
    (copy_prompt, mut exit): (Res<CopyPrompt>, MessageWriter<AppExit>),
    (confirm_state, rename_prompt): (Option<Res<ConfirmState>>, Option<Res<RenamePrompt>>),
    mut events: MessageReader<KeyboardInput>,
    mut clipboard: ResMut<Clipboard>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut leader_phase: ResMut<LeaderPhase>,
    mut handles: Query<&mut TerminalHandle>,
    mut client: Option<Single<&mut TmuxClient>>,
    (keys, ime, resolved, resolved_copy, targets, time): (
        Res<ButtonInput<KeyCode>>,
        Res<crate::input::ime::ImeState>,
        Res<Shortcuts>,
        Res<ResolvedCopyModeKeys>,
        ActionTargets,
        Res<Time<Real>>,
    ),
    active_pane: Option<Single<(Entity, &TmuxPane), With<ActivePane>>>,
    copy_modes: Query<(), With<CopyModeState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    forward_keys: Query<&ForwardKeys>,
) {
    // NOTE: while the copy-mode prompt is open it owns the keyboard; the prompt's
    // own system handles raw keys. Drain here so no key leaks to tmux or the
    // prefix state machine.
    if copy_prompt.open.is_some() {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }
    // NOTE: while the confirm-before prompt is open it owns the keyboard; its own
    // system reads the y/n answer. Drain here so no key leaks to tmux or the pane.
    if confirm_state.is_some() {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }
    // NOTE: while the rename prompt is open it owns the keyboard; its own system
    // reads the typed text. Drain here so no key leaks to tmux or the pane.
    if rename_prompt.is_some() {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }
    // NOTE: drain (don't replay) while composing — forwarding preedit
    // navigation keys would both garble IME composition and double-send.
    if ime.is_composing() {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if !focused {
        clear_leader_phase(&mut leader_phase);
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
    let now = time.elapsed();

    // The active pane (if any) is the forward/paste target. GUI chords below do
    // not need it (so quit and similar chords work before a pane is projected);
    // tmux key dispatch does.
    let (active_entity, target) = match active_pane {
        Some(single) => {
            let (entity, pane) = *single;
            (Some(entity), Some(format!("%{}", pane.id.0)))
        }
        None => (None, None),
    };

    // NOTE: read once before the main loop — the repeat-window close and the
    // in-loop copy-mode arm must agree on one snapshot of the state; the
    // EnterCopyMode arm separately re-checks `copy_modes` live for its
    // re-entry guard.
    let in_copy_mode = active_entity.is_some_and(|e| copy_modes.get(e).is_ok());

    // When a webview holds focus it owns the keyboard (bevy_cef routes
    // keystrokes to it); forwarding to tmux too would double-send. The configured
    // release-webview-focus chord releases focus back to the terminal. NOTE: under the tmux backend
    // `FocusedWebview` is live (set by the inline-click router, the control-plane
    // `SetFocus` op, and the focus-preservation arm in `sync_focused_webview`),
    // so this drain is load-bearing whenever a webview is focused —
    // removing it would double-send keystrokes to the page and the pane.
    if let Some(focused_entity) = focused_webview.0 {
        let forward_chords = forward_keys
            .get(focused_entity)
            .map(|pk| pk.0.as_slice())
            .unwrap_or(&[]);

        let mut forward_names: Vec<String> = Vec::new();
        for ev in events.read() {
            if ev.state != ButtonState::Pressed {
                continue;
            }
            if resolved.is_release_webview_focus(ev.key_code, cfg_mods) {
                focused_webview.0 = None;
                break;
            }
            if !forward_chords.is_empty()
                && forward_chords.iter().any(|c| {
                    c.code == ev.key_code
                        && c.ctrl == mods.ctrl
                        && c.shift == mods.shift
                        && c.alt == mods.alt
                        && c.logo == mods.super_
                })
                && let Some(name) = bevy_key_to_tmux_name(&ev.logical_key, ev.key_code, mods)
            {
                forward_names.push(name);
            }
        }

        if !forward_names.is_empty() {
            if let (Some(target), Some(client)) = (target.as_deref(), client.as_deref_mut())
                && let Err(e) = client.send(SendPaneKeys {
                    pane: target,
                    names: &forward_names,
                })
            {
                tracing::warn!(?e, "forward-key send failed");
            }
            if let Some(entity) = active_entity
                && let Ok(mut handle) = handles.get_mut(entity)
                && handle.snap_to_bottom_vt_only()
            {
                handle.flush_emit(&mut commands, entity);
            }
        }

        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }

    // Collect forwardable tmux key names in event order. Super-modified keys are
    // matched against the configured ozmux shortcuts or swallowed; none reach tmux.
    // NOTE: an open repeat window must not intercept copy-mode keys — a
    // repeat-marked key doubling as a copy-mode key (vi h/j/k/l etc.) would
    // re-fire the bound action and re-arm the window instead of reaching the
    // `resolved_copy.resolve` arm below. Close the window before stepping;
    // Pending semantics (leader + second key inside copy mode) stay unchanged.
    if in_copy_mode && matches!(*leader_phase, LeaderPhase::Repeat { .. }) {
        *leader_phase = LeaderPhase::Idle;
    }
    let mut key_names: Vec<String> = Vec::new();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        // NOTE: leader dispatch must run before direct matching and tmux
        // forwarding so a swallowed/resolved leader key never reaches the
        // plain key-forward batch below. cfg_mods is a per-frame snapshot, so
        // a same-frame leader+second-key batch shares one modifier read.
        // NOTE: OS key auto-repeat delivers extra Pressed events. Outside the
        // repeat window they must not step the machine (pending would toggle by
        // parity; a held leader chord must stay swallowed, not forwarded).
        // Inside the window they must step it so held keys keep firing.
        let step = if ev.repeat {
            match *leader_phase {
                LeaderPhase::Pending => continue,
                LeaderPhase::Repeat { .. } => {
                    step_leader(&mut leader_phase, &resolved, ev.key_code, cfg_mods, now)
                }
                LeaderPhase::Idle => LeaderStep::Passthrough,
            }
        } else {
            step_leader(&mut leader_phase, &resolved, ev.key_code, cfg_mods, now)
        };
        let gui_action = match step {
            LeaderStep::RunAction(action) => Some(action),
            LeaderStep::Swallow => {
                continue;
            }
            LeaderStep::Passthrough => resolved.match_gui_action(ev.key_code, cfg_mods),
        };
        if let Some(action) = gui_action {
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
                    let (Some(target), Some(tmux)) = (target.as_deref(), client.as_deref_mut())
                    else {
                        continue;
                    };
                    if let Some(entity) = active_entity
                        && let Ok(mut handle) = handles.get_mut(entity)
                        && handle.snap_to_bottom_vt_only()
                    {
                        handle.flush_emit(&mut commands, entity);
                    }
                    let bytes = build_paste_bytes(&text, false);
                    for chunk in bytes.chunks(PASTE_CHUNK_BYTES) {
                        if let Err(e) = tmux.send(SendBytes {
                            pane: target,
                            bytes: chunk,
                        }) {
                            tracing::warn!(?e, "paste send failed");
                            break;
                        }
                    }
                }
                ShortcutAction::ReleaseWebviewFocus => {}
                ShortcutAction::DetachSession => {
                    if let Some(client) = client.as_deref_mut() {
                        request_detach(client);
                    }
                }
                ShortcutAction::EnterCopyMode => {
                    // NOTE: re-entry guard — re-triggering while already in copy
                    // mode would double-insert CopyModeState and re-enter vi mode.
                    if let Some(entity) = active_entity
                        && copy_modes.get(entity).is_err()
                    {
                        commands.trigger(EnterCopyModeActionEvent { entity });
                    }
                }
                ShortcutAction::SelectPane(direction) => {
                    if let Some(entity) = active_entity {
                        commands.trigger(SelectPaneRequest {
                            entity,
                            direction: tmux_pane_direction(direction),
                        });
                    }
                }
                ShortcutAction::SplitPane(orientation) => {
                    if let Some(entity) = active_entity {
                        commands.trigger(SplitPaneRequest {
                            entity,
                            direction: tmux_split_direction(orientation),
                        });
                    }
                }
                ShortcutAction::KillPane => {
                    if let Some(entity) = active_entity {
                        commands.trigger(KillPaneRequest { entity });
                    }
                }
                ShortcutAction::ZoomPane => {
                    if let Some(entity) = active_entity {
                        commands.trigger(ZoomPaneRequest { entity });
                    }
                }
                ShortcutAction::NewWindow => {
                    if let Ok(entity) = targets.session.single() {
                        commands.trigger(NewWindowRequest { entity });
                    }
                }
                ShortcutAction::NextWindow => {
                    if let Ok(entity) = targets.session.single() {
                        commands.trigger(NextWindowRequest { entity });
                    }
                }
                ShortcutAction::PreviousWindow => {
                    if let Ok(entity) = targets.session.single() {
                        commands.trigger(PreviousWindowRequest { entity });
                    }
                }
                ShortcutAction::SelectWindow(index) => {
                    if let Some(entity) = targets
                        .windows
                        .iter()
                        .find(|(_, window)| window.index == u32::from(index))
                        .map(|(entity, _)| entity)
                    {
                        commands.trigger(SelectWindowRequest { entity });
                    }
                }
                ShortcutAction::KillWindow => {
                    if let Ok(entity) = targets.active_window.single() {
                        commands.trigger(KillWindowRequest { entity });
                    }
                }
                ShortcutAction::RenameWindow => {
                    if let Ok(entity) = targets.active_window.single() {
                        commands.trigger(RenameWindowRequest { entity });
                    }
                }
                ShortcutAction::RenameSession => {
                    if let Ok(entity) = targets.session.single() {
                        commands.trigger(RenameSessionRequest { entity });
                    }
                }
            }
            continue;
        }
        // NOTE: tmux/PTY has no Super modifier, so a Cmd-modified key that
        // matched no ozmux shortcut must be swallowed here, never forwarded.
        if mods.super_ {
            continue;
        }
        if in_copy_mode {
            if let Some(entity) = active_entity
                && let Some(action) = resolved_copy.resolve(&ev.logical_key, ev.key_code, cfg_mods)
            {
                trigger_copy_mode_action(&mut commands, entity, action);
            }
            continue;
        }
        if let Some(name) = bevy_key_to_tmux_name(&ev.logical_key, ev.key_code, mods) {
            key_names.push(name);
        }
    }

    if !in_copy_mode
        && !key_names.is_empty()
        && let Some(entity) = active_entity
        && let Ok(mut handle) = handles.get_mut(entity)
        && handle.snap_to_bottom_vt_only()
    {
        handle.flush_emit(&mut commands, entity);
    }

    if key_names.is_empty() {
        return;
    }
    let (Some(target), Some(client)) = (target.as_deref(), client.as_deref_mut()) else {
        return;
    };
    if let Err(e) = client.send(SendPaneKeys {
        pane: target,
        names: &key_names,
    }) {
        tracing::warn!(?e, "tmux key forward failed");
    }
}

/// Maps the config-facing pane direction (named after the neighbor) to the
/// tmux command enum.
fn tmux_pane_direction(direction: CfgPaneDirection) -> PaneDirection {
    match direction {
        CfgPaneDirection::Left => PaneDirection::Left,
        CfgPaneDirection::Down => PaneDirection::Down,
        CfgPaneDirection::Up => PaneDirection::Up,
        CfgPaneDirection::Right => PaneDirection::Right,
    }
}

/// Maps the config-facing split orientation (named after the DIVIDER) to the
/// tmux flag enum (named after the layout axis) — the two cross on purpose.
fn tmux_split_direction(orientation: CfgSplitOrientation) -> SplitDirection {
    match orientation {
        CfgSplitOrientation::Vertical => SplitDirection::Horizontal,
        CfgSplitOrientation::Horizontal => SplitDirection::Vertical,
    }
}

/// `send-keys -X -t %<id> -N <lines> scroll-up|scroll-down` — one copy-mode wheel notch.
///
/// ozmux drives copy-mode wheel scrolling with this single, targeted command
/// rather than relaying tmux's copy-table `WheelUpPane`/`WheelDownPane`
/// bindings, which are `select-pane \; send-keys …` sequences: relaying a
/// sequence as one control-mode command makes tmux emit an extra reply block
/// the protocol client cannot correlate (the `no pending command` storm).
struct Scroll<'a> {
    target: &'a str,
    up: bool,
    lines: u32,
}
impl TmuxCommand for Scroll<'_> {
    fn into_raw_command(self) -> String {
        format!(
            "send-keys -X -t {} -N {} {}",
            self.target,
            self.lines,
            if self.up { "scroll-up" } else { "scroll-down" }
        )
    }
}

/// `send-keys -t %<id> -N <lines> Up|Down` — scrolls an alt-screen (non-copy-mode) pane.
struct AltScreenScroll<'a> {
    target: &'a str,
    up: bool,
    lines: u32,
}
impl TmuxCommand for AltScreenScroll<'_> {
    fn into_raw_command(self) -> String {
        format!(
            "send-keys -t {} -N {} {}",
            self.target,
            self.lines,
            if self.up { "Up" } else { "Down" }
        )
    }
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

/// A focused webview claiming the wheel: the child to forward to and
/// the pointer in its webview-local DIP.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TmuxWebviewWheelTarget {
    child: Entity,
    position_dip: Vec2,
}

/// Wheel-routing params bundled to stay within Bevy's system-parameter limit.
/// `browsers` is optional so CEF-less tests construct the system.
#[derive(SystemParam)]
struct TmuxWebviewWheelParams<'w, 's> {
    focused_webview: Res<'w, FocusedWebview>,
    webview_parents: Query<'w, 's, &'static ChildOf, With<Webview>>,
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
    webviews: Query<'w, 's, (&'static Webview, Has<NonInteractive>)>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    browsers: Option<NonSend<'w, Browsers>>,
}

/// Target-entity lookups for the tmux shortcut actions, bundled to stay
/// within Bevy's system-parameter limit.
#[derive(SystemParam)]
struct ActionTargets<'w, 's> {
    active_window: Query<'w, 's, Entity, With<ActiveWindow>>,
    session: Query<'w, 's, Entity, With<TmuxSession>>,
    windows: Query<'w, 's, (Entity, &'static TmuxWindow)>,
}

/// Resolves the focused webview under the pointer, or `None` (the tmux
/// path runs). `Some` only when `FocusedWebview` holds an inline child of the
/// pane under the pointer AND the pointer is over that child's rect — the tmux
/// analog of native `resolve_inline_wheel_target`.
fn resolve_tmux_webview_wheel_target(
    params: &TmuxWebviewWheelParams,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale_factor: f32,
) -> Option<TmuxWebviewWheelTarget> {
    let (terminal, _pane_id, local_phys) = tmux_pane_at_phys(&params.panes, cursor_phys)?;
    let (child, position_dip) = webview_wheel_target(
        &params.focused_webview,
        &params.webview_parents,
        &params.children,
        &params.webviews,
        &params.overlay_rects,
        terminal,
        local_phys,
        cell_w_phys,
        cell_h_phys,
        scale_factor,
    )?;
    Some(TmuxWebviewWheelTarget {
        child,
        position_dip,
    })
}

/// Converts one `MouseWheel` event to the RAW CEF wheel delta (`Line → ×120`,
/// `Pixel` unchanged, NO sign flip) — identical to native `inline_wheel_delta`.
/// Which layer owns a wheel gesture over a tmux pane.
///
/// `forward_wheel_to_tmux` handles only `CopyMode` and `AltScreenResidual`;
/// every other case is `CededToOzma` — `crate::input::mouse::wheel::dispatch_mouse_wheel`
/// runs on the same pane (gated off only by `MouseDisabled`) and owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WheelOwner {
    /// Copy-mode pane: tmux drives scrolling via `send-keys -X scroll-up|down`.
    /// These panes carry `MouseDisabled`, so ozma never acts on them.
    CopyMode,
    /// Alt-screen pane with neither `ALTERNATE_SCROLL` nor any `MOUSE_MODE` bit:
    /// the host's `WheelAction` resolves to a `ScrollViewport` that is a
    /// no-op on the alt buffer, so tmux forwards cursor keys instead.
    AltScreenResidual,
    /// Anything ozma usefully handles: normal-pane local scrollback, mouse-mode
    /// SGR/X10 reports, or alt-screen with `ALTERNATE_SCROLL` (SS3 arrows). tmux
    /// drops the wheel and must not touch the accumulator for this pane.
    CededToOzma,
}

/// Picks the wheel owner as the exact complement of `WheelAction::route`.
///
/// `route` (in `ozma_tty_engine::wheel`) acts usefully when the pane is in a
/// mouse mode (`MOUSE_MODE` → SGR/X10), or in alt-screen with `ALTERNATE_SCROLL`
/// (→ SS3 arrows), or in a normal screen (→ real scrollback). The only case its
/// `ScrollViewport` is a no-op is alt-screen WITHOUT `ALTERNATE_SCROLL` and
/// WITHOUT a mouse mode — that residual is what tmux owns here. Copy-mode panes
/// never reach `route` (they carry `MouseDisabled`), so tmux owns them outright.
fn decide_wheel_owner(in_copy_mode: bool, in_alt_screen: bool, modes: TermMode) -> WheelOwner {
    if in_copy_mode {
        return WheelOwner::CopyMode;
    }
    if in_alt_screen
        && !modes.contains(TermMode::ALTERNATE_SCROLL)
        && !modes.intersects(TermMode::MOUSE_MODE)
    {
        return WheelOwner::AltScreenResidual;
    }
    WheelOwner::CededToOzma
}

/// Forwards mouse-wheel events to the tmux pane UNDER THE POINTER for the cases
/// `crate::input::mouse::wheel::dispatch_mouse_wheel` does NOT usefully own.
///
/// A focused webview under the pointer claims the wheel first
/// (`resolve_tmux_webview_wheel_target`): each event is forwarded RAW to that
/// child's CEF browser and dropped before the tmux accumulator.
///
/// Otherwise the owner is decided for the CURSOR pane (resolved via
/// `tmux_pane_at_phys`) as the exact complement of
/// `ozma_tty_engine::wheel::WheelAction::route` (see `decide_wheel_owner`):
/// a copy-mode pane (`CopyModeState`, always `MouseDisabled`) gets a targeted
/// `send-keys -X scroll-up|scroll-down`; an alt-screen pane with neither
/// `ALTERNATE_SCROLL` nor a `MOUSE_MODE` bit — where ozma's `ScrollViewport`
/// would no-op on the alt buffer — gets `AltScreenScroll` (cursor
/// keys). Every other case is ceded to ozma (local scrollback / SGR / SS3) and
/// the accumulator is left untouched for that pane so no residual notch bleeds.
///
/// # Invariants
///
/// The target pane MUST be the pane under the cursor, not `ActivePane`:
/// `crate::input::mouse::wheel::dispatch_mouse_wheel` and `crate::input::tmux::gate` both key off the
/// cursor pane, so keying this system off the active pane would let both fire on
/// different panes (cursor pane ≠ active pane) and double-act. The
/// "complement gated solely by `MouseDisabled`" invariant holds only when both
/// systems target the same pane.
fn forward_wheel_to_tmux(
    mut wheel: MessageReader<MouseWheel>,
    mut accumulator: ResMut<TmuxWheelAccumulator>,
    mut client: Option<Single<&mut TmuxClient>>,
    handles: Query<&TerminalHandle>,
    wheel_params: TmuxWebviewWheelParams,
    copy_prompt: Res<CopyPrompt>,
    rename_prompt: Option<Res<RenamePrompt>>,
    configs: Res<OzmuxConfigsResource>,
    metrics: Res<TerminalCellMetricsResource>,
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

    let target = cursor_phys.and_then(|c| {
        resolve_tmux_webview_wheel_target(&wheel_params, c, cell_w_phys, cell_h_phys, dpr)
    });

    let Some(delta_cells) = aggregate_tmux_wheel_cells(
        &mut wheel,
        target,
        wheel_params.browsers.as_deref(),
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
    if !window.focused || copy_prompt.open.is_some() || rename_prompt.is_some() {
        accumulator.residual_cells = 0.0;
        return;
    }
    // The wheel acts on the pane under the pointer — the same basis ozma's
    // dispatch_mouse_wheel and the gate's MouseDisabled use (see # Invariants).
    let Some((entity, pane_id, _local)) =
        cursor_phys.and_then(|c| tmux_pane_at_phys(&wheel_params.panes, c))
    else {
        accumulator.residual_cells = 0.0;
        return;
    };

    // NOTE: decide the owner BEFORE touching the accumulator. A ceded frame must
    // leave the residual untouched for this pane — advancing then dropping it
    // would bleed a stale notch into the next copy-mode / alt-fallback gesture,
    // and resetting it would fight the accumulation ozma performs independently.
    let in_copy_mode = copy_modes.get(entity).is_ok();
    let owner = match handles.get(entity) {
        Ok(handle) => decide_wheel_owner(
            in_copy_mode,
            handle.is_in_alt_screen(),
            handle.current_modes(),
        ),
        Err(_) => decide_wheel_owner(in_copy_mode, false, TermMode::empty()),
    };
    if owner == WheelOwner::CededToOzma {
        return;
    }

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
    let target = format!("%{}", pane_id.0);
    let lines = configs.mouse.lines_per_notch;
    let total_lines = count as u32 * lines;

    let Some(tmux) = client.as_deref_mut() else {
        return;
    };

    let (cmd, failure) = match owner {
        WheelOwner::CopyMode => (
            Scroll {
                target: &target,
                up,
                lines: total_lines,
            }
            .into_raw_command(),
            "copy-mode wheel scroll send failed",
        ),
        WheelOwner::AltScreenResidual => (
            AltScreenScroll {
                target: &target,
                up,
                lines: total_lines,
            }
            .into_raw_command(),
            "alt-screen wheel scroll send failed",
        ),
        WheelOwner::CededToOzma => return,
    };
    if let Err(e) = tmux.send(&cmd) {
        tracing::warn!(?e, failure);
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
/// forwarded RAW to CEF via `webview_wheel_delta` (no sign flip).
fn aggregate_tmux_wheel_cells(
    wheel: &mut MessageReader<MouseWheel>,
    target: Option<TmuxWebviewWheelTarget>,
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
                    webview_wheel_delta(ev.unit, ev.x, ev.y),
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
        && delta_cells != 0.0
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

    #[test]
    fn scroll_up_is_targeted_and_repeated() {
        assert_eq!(
            Scroll {
                target: "%3",
                up: true,
                lines: 3,
            }
            .into_raw_command(),
            "send-keys -X -t %3 -N 3 scroll-up"
        );
    }

    #[test]
    fn wheel_copy_mode_pane_is_owned_by_tmux() {
        // Copy-mode panes carry MouseDisabled (ozma never runs), so tmux keeps
        // the Scroll path regardless of screen / mode bits.
        assert_eq!(
            decide_wheel_owner(true, false, TermMode::empty()),
            WheelOwner::CopyMode
        );
        assert_eq!(
            decide_wheel_owner(true, true, TermMode::ALTERNATE_SCROLL),
            WheelOwner::CopyMode
        );
    }

    #[test]
    fn wheel_owned_by_ozma_outside_copymode_altscreen_inline() {
        // A normal pane (not copy-mode, not alt-screen, no mouse mode) is ceded
        // to the local terminal — forward_wheel_to_tmux emits no send-keys for it.
        assert_eq!(
            decide_wheel_owner(false, false, TermMode::empty()),
            WheelOwner::CededToOzma
        );
    }

    #[test]
    fn wheel_alt_screen_without_alternate_scroll_is_tmux_residual() {
        // ozma's WheelAction returns ScrollViewport here, which no-ops on the
        // alt buffer; tmux owns the residual with cursor-key send-keys.
        assert_eq!(
            decide_wheel_owner(false, true, TermMode::ALT_SCREEN),
            WheelOwner::AltScreenResidual
        );
    }

    #[test]
    fn wheel_alt_screen_with_alternate_scroll_is_ceded_to_ozma() {
        // ALTERNATE_SCROLL makes ozma's route emit SS3 arrows; tmux must cede so
        // the wheel is not double-forwarded.
        assert_eq!(
            decide_wheel_owner(
                false,
                true,
                TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL
            ),
            WheelOwner::CededToOzma
        );
    }

    #[test]
    fn wheel_mouse_mode_pane_is_ceded_to_ozma() {
        // Any MOUSE_MODE bit routes to ozma's SGR/X10 path (priority over the
        // alt-screen translation), so tmux cedes even in alt-screen.
        for bit in [
            TermMode::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_DRAG,
            TermMode::MOUSE_MOTION,
        ] {
            assert_eq!(
                decide_wheel_owner(false, false, bit),
                WheelOwner::CededToOzma
            );
            assert_eq!(
                decide_wheel_owner(false, true, TermMode::ALT_SCREEN | bit),
                WheelOwner::CededToOzma
            );
        }
    }

    fn spawn_pane_node(
        app: &mut App,
        copy_mode: bool,
        active: bool,
        center_x: f32,
        size_x: f32,
    ) -> Entity {
        use tmux_control_parser::CellDims;

        let mut e = app.world_mut().spawn((
            TmuxPane {
                id: ozmux_tmux::PaneId(1),
                dims: CellDims {
                    width: 50,
                    height: 37,
                    xoff: 0,
                    yoff: 0,
                },
            },
            ComputedNode {
                size: Vec2::new(size_x, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(center_x, 300.0),
            TerminalOverlays::default(),
        ));
        if copy_mode {
            e.insert(CopyModeState);
        }
        if active {
            e.insert(ActivePane);
        }
        e.id()
    }

    #[test]
    fn wheel_owner_is_decided_from_cursor_pane_not_active_pane() {
        // Two non-overlapping panes: the cursor is over a NORMAL pane (left half),
        // while the ActivePane is a copy-mode pane (right half). The wheel owner
        // MUST be decided from the cursor pane (→ CededToOzma), not the active
        // pane (which would give CopyMode). This pins the cursor-pane targeting
        // that keeps the complement aligned with ozma + the gate.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let cursor_pane = spawn_pane_node(&mut app, false, false, 200.0, 400.0);
        let _active_copy_pane = spawn_pane_node(&mut app, true, true, 600.0, 400.0);

        let (resolved, owner) = app
            .world_mut()
            .run_system_once(
                move |panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
                      copy_modes: Query<(), With<CopyModeState>>| {
                    let (entity, _id, _local) =
                        tmux_pane_at_phys(&panes, Vec2::new(200.0, 300.0)).unwrap();
                    let in_copy_mode = copy_modes.get(entity).is_ok();
                    // No TerminalHandle in this harness → the Err arm: a normal,
                    // non-alt cursor pane resolves to CededToOzma.
                    let owner = decide_wheel_owner(in_copy_mode, false, TermMode::empty());
                    (entity, owner)
                },
            )
            .unwrap();

        assert_eq!(resolved, cursor_pane, "wheel must target the cursor pane");
        assert_eq!(
            owner,
            WheelOwner::CededToOzma,
            "cursor pane is normal → ceded to ozma, despite the active pane being in copy mode"
        );
    }

    #[test]
    fn scroll_down_is_targeted_and_repeated() {
        assert_eq!(
            Scroll {
                target: "%3",
                up: false,
                lines: 5,
            }
            .into_raw_command(),
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
                Webview {
                    view_id: "webview".into(),
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

    fn run_resolve_wheel_target(
        app: &mut App,
        cursor_phys: Vec2,
    ) -> Option<TmuxWebviewWheelTarget> {
        app.world_mut()
            .run_system_once(move |params: TmuxWebviewWheelParams| {
                resolve_tmux_webview_wheel_target(&params, cursor_phys, 8.0, 16.0, 1.0)
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
    fn split_orientation_crosses_to_tmux_flag() {
        assert_eq!(
            tmux_split_direction(CfgSplitOrientation::Vertical),
            SplitDirection::Horizontal
        );
        assert_eq!(
            tmux_split_direction(CfgSplitOrientation::Horizontal),
            SplitDirection::Vertical
        );
    }

    #[test]
    fn pane_direction_maps_one_to_one() {
        assert_eq!(
            tmux_pane_direction(CfgPaneDirection::Left),
            PaneDirection::Left
        );
        assert_eq!(
            tmux_pane_direction(CfgPaneDirection::Right),
            PaneDirection::Right
        );
    }
}
