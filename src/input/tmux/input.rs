//! Forwards focused keyboard and mouse-wheel input to the active tmux pane.
//! Keyboard forwarding dispatches a fixed set of ozmux GUI chords as
//! per-command action events (`crate::action::tmux`) and copy-mode
//! entry commands; while a pane is in copy mode, keys resolve against the
//! shared config-driven key table (`crate::action::vi::ResolvedCopyModeKeys`)
//! and fire the shared VI events, which `crate::action::vi::applier`
//! applies for every pane (tmux and non-tmux alike) rather than a
//! tmux-specific applier module; unmatched keys forward straight to the pane
//! in one `SendPaneKeys` batch
//! per frame. Mouse-wheel forwarding handles only the
//! cases `crate::input::mouse::wheel::dispatch_mouse_wheel` does not own (it
//! now runs on every tmux pane, gated off solely by `MouseDisabled`): an
//! inline webview under the pointer (forwarded to CEF), a copy-mode pane
//! (scrolled directly via `TerminalHandle::scroll`), and the alt-screen
//! residual where ozma's viewport scroll would no-op (cursor-key
//! `send-keys`). Events accumulate into cell-deltas (so trackpad /
//! high-resolution `Pixel` scrolling quantizes the same way the native
//! terminal path does); every other case is ceded to ozma.

use super::forward::ForwardPaneKeysRequest;
use super::pane_hit::tmux_pane_at_phys;
use crate::action::terminal::PasteAction;
use crate::action::tmux::{
    DetachSessionRequest, KillPaneRequest, KillWindowRequest, NewWindowRequest, NextWindowRequest,
    PreviousWindowRequest, RenameSessionRequest, RenameWindowRequest, SelectPaneRequest,
    SelectWindowRequest, SplitPaneRequest, ZoomPaneRequest,
};
use crate::action::vi::{ResolvedCopyModeKeys, trigger_copy_mode_action};
use crate::app_mode::{AppMode, TmuxActiveSet};
use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::input::current_modifiers;
use crate::input::ime::ImeState;
use crate::input::resolve::{BatchContext, KeyEffect, classify_key_batch};
use crate::input::shortcuts::{LeaderGate, LeaderPhase, Shortcuts, clear_leader_phase};
use crate::ui::copy_mode::{CopyModeState, EnterCopyModeActionEvent};
use crate::ui::copy_search::CopyPrompt;
use crate::ui::tmux::confirm_prompt::ConfirmState;
use crate::ui::tmux::rename_prompt::RenamePrompt;
use crate::webview_pointer::{webview_wheel_delta, webview_wheel_target};
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::time::Real;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use ozma_tty_engine::{Coalescer, TermMode, TerminalHandle};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozma_webview::{ForwardKeys, NonInteractive, Webview};
use ozmux_configs::shortcuts::{
    PaneDirection as CfgPaneDirection, ShortcutAction, SplitOrientation as CfgSplitOrientation,
};
use ozmux_tmux::{
    ActivePane, ActiveWindow, KeyMods, PaneDirection, SplitDirection, TmuxClient, TmuxCommand,
    TmuxPane, TmuxSession, TmuxWindow, bevy_key_to_tmux_name,
};

/// Registers the tmux keyboard-forwarding and mouse-wheel systems.
pub(super) struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxWheelAccumulator>().add_systems(
            Update,
            (
                apply_tmux_shortcuts
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

/// Applies `AppMode::Tmux` keyboard shortcuts: resolves the frame's pressed keys
/// through the pure `classify_key_batch` decider, then triggers the matching
/// events on the active pane / session / window — Quit (`AppExit`), copy-mode
/// entry, paste (`PasteAction`, applied by `on_paste_tmux`), detach
/// (`DetachSessionRequest`), the pane/window action requests, the shared
/// `[copy-mode]` key table, and raw-key forwarding batched into one
/// `ForwardPaneKeysRequest` per frame. Registered in `InputPhase::FocusedKey` /
/// `LeaderGate::Advance`, gated on `in_state(AppMode::Tmux)` +
/// `on_message::<KeyboardInput>`.
fn apply_tmux_shortcuts(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut leader_phase: ResMut<LeaderPhase>,
    (copy_prompt, confirm_state, rename_prompt, ime): (
        Res<CopyPrompt>,
        Option<Res<ConfirmState>>,
        Option<Res<RenamePrompt>>,
        Res<ImeState>,
    ),
    shortcuts: Res<Shortcuts>,
    resolved_copy: Res<ResolvedCopyModeKeys>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    targets: ActionTargets,
    active_pane: Option<Single<Entity, (With<ActivePane>, With<TmuxPane>)>>,
    copy_modes: Query<(), With<CopyModeState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    forward_keys: Query<&ForwardKeys>,
) {
    // NOTE: each of these modal owners (copy-mode prompt, confirm-before prompt,
    // rename prompt) holds the keyboard and reads raw keys in its own system;
    // while composing, replaying preedit keys would garble IME + double-send; an
    // unfocused window must not act. Drain (don't replay) so no key leaks to
    // tmux, the pane, or the prefix state machine.
    if copy_prompt.open.is_some()
        || confirm_state.is_some()
        || rename_prompt.is_some()
        || ime.is_composing()
        || !windows.single().map(|w| w.focused).unwrap_or(false)
    {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }

    let active_entity = active_pane.map(|pane| *pane);
    let in_copy_mode = active_entity.is_some_and(|entity| copy_modes.get(entity).is_ok());
    let forward_chords = focused_webview
        .0
        .and_then(|entity| forward_keys.get(entity).ok())
        .map(|chords| chords.0.as_slice())
        .unwrap_or(&[]);
    let mods = current_modifiers(&bevy_keys);
    let kmods = KeyMods {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        super_: mods.meta,
    };
    let ctx = BatchContext {
        mods,
        now: time.elapsed(),
        in_copy_mode,
        webview_focused: focused_webview.0.is_some(),
        forward_chords,
    };
    let effects = classify_key_batch(
        &mut leader_phase,
        &shortcuts,
        &resolved_copy,
        events.read(),
        ctx,
    );

    let mut names: Vec<String> = Vec::new();
    for effect in effects {
        match effect {
            KeyEffect::Action {
                action: ShortcutAction::Quit,
                ..
            } => {
                exit.write(AppExit::Success);
            }
            KeyEffect::Action {
                action: ShortcutAction::EnterCopyMode,
                ..
            } => {
                // NOTE: re-entry guard — re-triggering while already in copy mode
                // would double-insert CopyModeState and re-enter vi mode.
                if let Some(entity) = active_entity
                    && !in_copy_mode
                {
                    commands.trigger(EnterCopyModeActionEvent { entity });
                }
            }
            KeyEffect::Action {
                action: ShortcutAction::Paste,
                ..
            } => {
                if let Some(entity) = active_entity {
                    commands.trigger(PasteAction { entity });
                }
            }
            KeyEffect::Action {
                action: ShortcutAction::DetachSession,
                ..
            } => {
                if let Ok(entity) = targets.session.single() {
                    commands.trigger(DetachSessionRequest { entity });
                }
            }
            KeyEffect::Action { action, .. } => {
                dispatch_tmux_action(&mut commands, action, active_entity, &targets);
            }
            KeyEffect::CopyMode(action) => {
                if let Some(entity) = active_entity {
                    trigger_copy_mode_action(&mut commands, entity, action);
                }
            }
            KeyEffect::Type { logical, key_code }
            | KeyEffect::WebviewForward { logical, key_code } => {
                if let Some(name) = bevy_key_to_tmux_name(&logical, key_code, kmods) {
                    names.push(name);
                }
            }
            KeyEffect::ReleaseWebviewFocus => focused_webview.0 = None,
        }
    }

    if let Some(entity) = active_entity
        && !names.is_empty()
    {
        commands.trigger(ForwardPaneKeysRequest { entity, names });
    }
}

/// Triggers the tmux pane/window action request for `action` on the resolved
/// target: pane actions on `active_entity`, window actions on the active window
/// or the display-indexed window, session-scoped actions on the session. The
/// non-tmux actions (handled by the caller before this helper) are no-ops.
fn dispatch_tmux_action(
    commands: &mut Commands,
    action: ShortcutAction,
    active_entity: Option<Entity>,
    targets: &ActionTargets,
) {
    match action {
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
        ShortcutAction::Quit
        | ShortcutAction::EnterCopyMode
        | ShortcutAction::Paste
        | ShortcutAction::DetachSession
        | ShortcutAction::ReleaseWebviewFocus => {}
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
    /// Copy-mode pane: `forward_wheel_to_tmux` scrolls the local
    /// `TerminalHandle` directly. These panes carry `MouseDisabled`, so ozma
    /// never acts on them.
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
/// never reach `route` (they carry `MouseDisabled`), so this system scrolls
/// them locally instead.
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
/// a copy-mode pane (`CopyModeState`, always `MouseDisabled`) scrolls the
/// local `TerminalHandle` directly via `TerminalHandle::scroll`; an
/// alt-screen pane with neither `ALTERNATE_SCROLL` nor a `MOUSE_MODE` bit —
/// where ozma's `ScrollViewport` would no-op on the alt buffer — gets
/// `AltScreenScroll` (cursor keys) sent to tmux. Every other case is ceded to
/// ozma (local scrollback / SGR / SS3) and the accumulator is left untouched
/// for that pane so no residual notch bleeds.
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
    mut handles: Query<(&mut TerminalHandle, &mut Coalescer)>,
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
        Ok((handle, _)) => decide_wheel_owner(
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

    let signed = if up {
        total_lines as i32
    } else {
        -(total_lines as i32)
    };
    match owner {
        WheelOwner::CopyMode => {
            if let Ok((mut handle, mut coalescer)) = handles.get_mut(entity) {
                handle.scroll(&mut coalescer, signed);
            }
        }
        WheelOwner::AltScreenResidual => {
            let Some(tmux) = client.as_deref_mut() else {
                return;
            };
            let cmd = AltScreenScroll {
                target: &target,
                up,
                lines: total_lines,
            }
            .into_raw_command();
            if let Err(e) = tmux.send(&cmd) {
                tracing::warn!(?e, "alt-screen wheel scroll send failed");
            }
        }
        WheelOwner::CededToOzma => {}
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
    use crate::input::shortcuts::{
        test_shortcuts_with_direct_chord, test_shortcuts_with_repeat_prefix,
    };
    use bevy::ecs::system::RunSystemOnce;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::Key;
    use bevy::input::mouse::MouseScrollUnit;
    use ozmux_configs::shortcuts::Modifiers;
    use ozmux_tmux::PaneId;
    use std::time::Duration;

    #[test]
    fn wheel_copy_mode_pane_owner_ignores_screen_and_mode_bits() {
        // Copy-mode panes carry MouseDisabled (ozma never runs), so the
        // wheel path scrolls the local handle regardless of screen / mode bits.
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
                id: PaneId(1),
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
    fn wheel_copy_mode_scrolls_local_handle() {
        use bevy::window::WindowResolution;
        use ozma_tty_renderer::CellMetrics;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseWheel>();
        app.init_resource::<FocusedWebview>();
        app.init_resource::<CopyPrompt>();
        app.init_resource::<TmuxWheelAccumulator>();
        app.insert_resource(OzmuxConfigsResource::default());
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        });

        // A pane covering the whole 800x600 window so the cursor position
        // below resolves to this entity via `tmux_pane_at_phys`.
        let pane = spawn_pane_node(&mut app, true, false, 400.0, 800.0);
        let mut handle = TerminalHandle::detached(20, 5);
        handle.advance(b"l1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6\r\nl7\r\nl8\r\nl9\r\nl10\r\n");
        app.world_mut()
            .entity_mut(pane)
            .insert((handle, Coalescer::default()));

        let mut window = Window {
            focused: true,
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.set_cursor_position(Some(Vec2::new(100.0, 100.0)));
        app.world_mut().spawn((window, PrimaryWindow));

        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseWheel>>()
            .write(make_wheel_event(MouseScrollUnit::Line, 2.0));

        // No TmuxClient anywhere in this world — proves the copy-mode scroll
        // does not depend on a tmux send.
        app.world_mut()
            .run_system_once(forward_wheel_to_tmux)
            .unwrap();

        let snapshot = app
            .world()
            .get::<TerminalHandle>(pane)
            .unwrap()
            .vi_indicator_snapshot();
        assert!(
            snapshot.scroll_offset > 0,
            "copy-mode wheel scroll did not move the local handle"
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
                    id: PaneId(1),
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

    #[derive(Resource, Default)]
    struct TmuxCaptured {
        quit: u32,
        select_pane: Vec<(Entity, PaneDirection)>,
        select_window: Vec<Entity>,
        detach: Vec<Entity>,
        forward: Vec<(Entity, Vec<String>)>,
    }

    fn capture_tmux_app_exit(
        mut exits: MessageReader<AppExit>,
        mut captured: ResMut<TmuxCaptured>,
    ) {
        captured.quit += exits.read().count() as u32;
    }

    fn meta_only() -> Modifiers {
        Modifiers {
            ctrl: false,
            shift: false,
            alt: false,
            meta: true,
        }
    }

    /// Builds an app running `apply_tmux_shortcuts` with a focused window and a
    /// single `ActivePane` tmux pane, capturing the action requests it triggers.
    fn tmux_dispatch_app(shortcuts: Shortcuts) -> (App, Entity) {
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .add_message::<AppExit>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ImeState>()
            .init_resource::<FocusedWebview>()
            .init_resource::<LeaderPhase>()
            .init_resource::<ResolvedCopyModeKeys>()
            .init_resource::<CopyPrompt>()
            .init_resource::<TmuxCaptured>()
            .insert_resource(shortcuts)
            .add_systems(Update, apply_tmux_shortcuts)
            .add_systems(Update, capture_tmux_app_exit.after(apply_tmux_shortcuts))
            .add_observer(|ev: On<SelectPaneRequest>, mut c: ResMut<TmuxCaptured>| {
                c.select_pane.push((ev.entity, ev.direction));
            })
            .add_observer(|ev: On<SelectWindowRequest>, mut c: ResMut<TmuxCaptured>| {
                c.select_window.push(ev.entity);
            })
            .add_observer(
                |ev: On<DetachSessionRequest>, mut c: ResMut<TmuxCaptured>| {
                    c.detach.push(ev.entity);
                },
            )
            .add_observer(
                |ev: On<ForwardPaneKeysRequest>, mut c: ResMut<TmuxCaptured>| {
                    c.forward.push((ev.entity, ev.names.clone()));
                },
            );
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        let pane = app
            .world_mut()
            .spawn((
                ActivePane,
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 80,
                        height: 24,
                        xoff: 0,
                        yoff: 0,
                    },
                },
            ))
            .id();
        (app, pane)
    }

    fn send_tmux_key(app: &mut App, key_code: KeyCode, logical: Key) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    #[test]
    fn leader_h_triggers_select_pane_left() {
        let (mut app, pane) = tmux_dispatch_app(test_shortcuts_with_repeat_prefix(
            KeyCode::KeyH,
            ShortcutAction::SelectPane(CfgPaneDirection::Left),
            Duration::ZERO,
        ));
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        send_tmux_key(&mut app, KeyCode::KeyH, Key::Character("h".into()));
        app.update();
        assert_eq!(
            app.world().resource::<TmuxCaptured>().select_pane,
            vec![(pane, PaneDirection::Left)],
            "a leader-scoped SelectPane(Left) must trigger SelectPaneRequest on the active pane"
        );
    }

    #[test]
    fn quit_writes_appexit() {
        let (mut app, _pane) = tmux_dispatch_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyQ,
            meta_only(),
            ShortcutAction::Quit,
        ));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        send_tmux_key(&mut app, KeyCode::KeyQ, Key::Character("q".into()));
        app.update();
        assert_eq!(
            app.world().resource::<TmuxCaptured>().quit,
            1,
            "the quit chord must write AppExit::Success"
        );
    }

    #[test]
    fn plain_keys_batch_into_one_forward_request() {
        let (mut app, pane) = tmux_dispatch_app(Shortcuts::default());
        send_tmux_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        send_tmux_key(&mut app, KeyCode::KeyB, Key::Character("b".into()));
        app.update();
        assert_eq!(
            app.world().resource::<TmuxCaptured>().forward,
            vec![(pane, vec!["a".to_string(), "b".to_string()])],
            "plain keys in one frame must batch into a single ForwardPaneKeysRequest"
        );
    }

    #[test]
    fn detach_triggers_detach_session_request() {
        use tmux_control_parser::SessionId;

        let (mut app, _pane) = tmux_dispatch_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyD,
            Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
                meta: false,
            },
            ShortcutAction::DetachSession,
        ));
        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(1),
                name: "main".into(),
            })
            .id();
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ControlLeft);
            keys.press(KeyCode::ShiftLeft);
        }
        send_tmux_key(&mut app, KeyCode::KeyD, Key::Character("d".into()));
        app.update();
        assert_eq!(
            app.world().resource::<TmuxCaptured>().detach,
            vec![session],
            "the detach chord must trigger DetachSessionRequest on the session"
        );
    }

    #[test]
    fn select_window_targets_indexed_window() {
        use tmux_control_parser::WindowId;

        let (mut app, _pane) = tmux_dispatch_app(test_shortcuts_with_direct_chord(
            KeyCode::Digit2,
            meta_only(),
            ShortcutAction::SelectWindow(2),
        ));
        app.world_mut().spawn(TmuxWindow {
            id: WindowId(1),
            index: 1,
            name: "one".into(),
        });
        let window_two = app
            .world_mut()
            .spawn(TmuxWindow {
                id: WindowId(2),
                index: 2,
                name: "two".into(),
            })
            .id();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        send_tmux_key(&mut app, KeyCode::Digit2, Key::Character("2".into()));
        app.update();
        assert_eq!(
            app.world().resource::<TmuxCaptured>().select_window,
            vec![window_two],
            "SelectWindow(2) must target the window whose display index is 2"
        );
    }
}
