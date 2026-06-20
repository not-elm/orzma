//! Mouse gesture arbiter for the tmux backend.
//!
//! Owns a single left-button state machine (`TmuxMouseGesture`) that reads raw
//! `MouseButtonInput` messages and issues `select-pane` on a focused press. When
//! the pane is in copy mode, a press that drags past `drag_threshold_px` enters
//! `Selecting` and multi-click (≥2) enters `PendingMultiSelect`; both relay the
//! tmux copy-mode path with pane-targeted `send-keys -X` commands. Text selection,
//! word/line copy, and hyperlink hover/open for a pane NOT in copy mode are owned
//! by `ozma_terminal`'s shared mouse systems, not here.
//! Divider-drag-to-resize is also here: a press within `divider_grab_tolerance_px`
//! of a divider line enters `Resizing` state; the pointer's major-axis cell
//! coordinate maps to an absolute target size sent as `resize-pane -x/-y`.

mod apply;
mod decide;
mod effect;

use super::copy_mode::{CopyModeSnapshot, cell_at_pane};
use super::pane_hit::{phys_to_pane_local, tmux_pane_at_phys};
use super::render::{DividerPixelRect, PackedTmuxLayout};
use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::picker::SessionPicker;
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::CopyPrompt;
use crate::webview::mount::{Webview, webview_hit_at, webview_local_dip};
use crate::webview::osc::NonInteractive;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
pub(crate) use decide::divider_at;
use decide::{
    ClickTracker, ContinuationCtx, PressHit, ReleaseCtx, decide_continuation, decide_press,
    decide_release,
};
use effect::{MultiSelectKind, TmuxMouseEffect, TmuxMouseEffects};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozmux_tmux::{ActiveWindow, PaneId, TmuxPane};
use std::time::Duration;
use tmux_control_parser::DividerAxis;

/// Bevy plugin that registers the tmux mouse gesture arbiter.
pub(crate) struct MousePlugin;

/// Tracks the CEF child that is currently pressed (a left press inside an
/// interactive inline rect was forwarded to it) so the matching release routes
/// to the same child even if the pointer drifted off-rect.
#[derive(Resource, Default)]
pub(super) struct TmuxWebviewPress(pub(super) Option<Entity>);

impl Plugin for MousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxMouseGesture>()
            .init_resource::<TmuxWebviewPress>()
            .add_systems(
                Update,
                arbiter
                    .in_set(InputPhase::Dispatch)
                    .in_set(super::OzmuxActiveSet),
            )
            .add_systems(
                Update,
                forward_tmux_webview_mouse_moves
                    .in_set(InputPhase::Hover)
                    .in_set(super::OzmuxActiveSet),
            )
            .add_observer(apply::on_tmux_mouse_effects);
    }
}

/// Modal-input gate: the resources whose presence means another surface owns
/// input and the arbiter must drain events without mutating tmux. The
/// focused-webview case is NOT gated here — the inline click pre-step
/// (`route_tmux_webview_left_click`) owns webview focus instead.
#[derive(SystemParam)]
struct ModalGate<'w> {
    picker: Res<'w, SessionPicker>,
    copy_prompt: Res<'w, CopyPrompt>,
}

/// Bundles the two immutable copy-mode query reads used by the gesture arbiter.
#[derive(SystemParam)]
struct CopyModeGate<'w, 's> {
    copy_modes: Query<'w, 's, (), With<CopyModeState>>,
    snapshots: Query<'w, 's, &'static CopyModeSnapshot>,
}

/// The current phase of a left-button gesture over a tmux pane.
#[derive(Default, Debug, PartialEq)]
enum GestureState {
    /// No button is held; the arbiter is waiting for the next press.
    #[default]
    Idle,
    /// Left button is held; `pane`/`pane_id` is the pane that received the press
    /// and `origin_phys` is the physical-pixel cursor position at press time.
    /// Becomes `Selecting` once the pointer drags past `drag_threshold_px`.
    Pressed {
        pane: Entity,
        pane_id: PaneId,
        origin_phys: Vec2,
        click_count: u8,
    },
    /// A double/triple click awaiting its copy-mode snapshot before positioning
    /// the copy cursor and selecting a word/line.
    PendingMultiSelect {
        pane: Entity,
        pane_id: PaneId,
        cell: (u16, u16),
        kind: MultiSelectKind,
    },
    /// Selecting text in a pane via tmux copy-mode (entered on drag-start).
    Selecting {
        pane: Entity,
        pane_id: PaneId,
        anchor: (u16, u16),
        begun: bool,
        last_target: Option<(u16, u16)>,
    },
    /// Dragging a divider to resize its primary pane.
    Resizing {
        divider: DividerPixelRect,
        /// The primary pane's fixed near edge (xoff for vertical, yoff for horizontal), cells.
        near: i32,
        /// Last absolute size (cells) we issued a resize for.
        last_sent: u32,
        /// Whether any `resize-pane` was actually sent (i.e. the pointer
        /// dragged). A press that never drags is a click: on release it falls
        /// back to focusing the pane under the cursor, because the grab zone
        /// overlaps the adjacent pane bodies.
        resized: bool,
    },
}

/// Tracks the current left-button gesture over a tmux pane.
#[derive(Resource, Default)]
pub(crate) struct TmuxMouseGesture {
    state: GestureState,
    click: ClickTracker,
}

/// Returns the `(Entity, PaneId)` of the first `TmuxPane` whose `ComputedNode`
/// contains `cursor_phys` (physical px), or `None` when no pane covers the point.
fn pane_under_cursor(
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    cursor_phys: Vec2,
) -> Option<(Entity, PaneId)> {
    panes
        .iter()
        .find(|(_, _, node, transform)| node.contains_point(**transform, cursor_phys))
        .map(|(entity, pane, _, _)| (entity, pane.id))
}

/// Interprets raw left-button messages into tmux `select-pane`, `resize-pane`,
/// or selection commands.
///
/// On each `Pressed` event the cursor's physical position is hit-tested: a
/// press within a divider's grab zone (whose primary pane has geometry) enters
/// `Resizing`; otherwise the pane under the cursor is focused (`select-pane`)
/// and the state becomes `Pressed`. While `Pressed`, a pointer that drags past
/// `drag_threshold_px` transitions to `Selecting` when the pane is already in
/// copy mode (drag/selection for a pane NOT in copy mode is owned by
/// `ozma_terminal`). Multi-click (≥2) on a pane in copy mode enters
/// `PendingMultiSelect` to wait for a copy-mode snapshot, then selects a
/// word/line via copy-mode commands. Each frame while `Resizing` the pointer's
/// major-axis cell coordinate is mapped to an absolute target size and sent as
/// `resize-pane -x/-y` whenever the target changes. On `Released` from
/// `Selecting` a begun selection is copied to clipboard; from `Resizing` that
/// never dragged, the pane under the cursor is focused as a fallback click. When
/// the primary window is not focused, or a modal (picker / copy-search prompt)
/// owns input, queued events are drained and the state is reset.
///
/// Each left press/release is first offered to the webview layer
/// (`route_tmux_webview_left_click`): a press inside an interactive inline rect
/// focuses + forwards to the child's CEF browser and never reaches the tmux
/// gesture pipeline; a press outside every rect drops inline focus and falls
/// through to the normal pane gesture.
fn arbiter(
    mut commands: Commands,
    mut gesture: ResMut<TmuxMouseGesture>,
    mut webview_press: ResMut<TmuxWebviewPress>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut webview_route: TmuxWebviewRouteParams,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    packed_q: Query<&PackedTmuxLayout, With<ActiveWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    configs: Option<Res<OzmuxConfigsResource>>,
    modals: ModalGate,
    copy_gate: CopyModeGate,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let ModalGate {
        picker,
        copy_prompt,
    } = &modals;
    let Ok(window) = windows.single() else {
        buttons.clear();
        // NOTE: no window means no cursor/scale to synthesize the CEF mouse-up,
        // so just drop any in-flight inline press — leaving it set would let a
        // later release act on a stale child.
        webview_press.0 = None;
        gesture.state = GestureState::Idle;
        return;
    };
    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cursor_phys = window.cursor_position().map(|c| c * scale);
    if !window.focused {
        buttons.clear();
        release_webview_press(
            &mut webview_press,
            &webview_route,
            &panes,
            cursor_phys,
            cell_w,
            cell_h,
            scale,
        );
        gesture.state = GestureState::Idle;
        return;
    }
    // NOTE: a gesture behind a picker / copy-search prompt must not mutate
    // tmux. The focused-webview case is NOT drained here — the inline click
    // pre-step below owns focus (in-rect press keeps it, off-rect press
    // releases it and drives tmux). An in-flight inline press IS released so
    // the focused page does not stay logically pressed (no matching mouse-up).
    if picker.open || copy_prompt.open.is_some() {
        buttons.clear();
        release_webview_press(
            &mut webview_press,
            &webview_route,
            &panes,
            cursor_phys,
            cell_w,
            cell_h,
            scale,
        );
        gesture.state = GestureState::Idle;
        return;
    }

    let (grab_tol_logical, drag_threshold_logical, dbl_click_ms, click_drift) = configs
        .as_deref()
        .map(|c| {
            (
                c.mouse.divider_grab_tolerance_px,
                c.mouse.drag_threshold_px,
                c.mouse.double_click_timeout_ms,
                c.mouse.click_drift_px,
            )
        })
        .unwrap_or((4.0, 4.0, 400, 8.0));
    let drag_threshold_phys = drag_threshold_logical * scale;
    let dbl_click = (Duration::from_millis(dbl_click_ms as u64), click_drift);

    let packed_dividers: &[DividerPixelRect] = packed_q
        .single()
        .map(|p| p.dividers.as_slice())
        .unwrap_or(&[]);

    let mut effects: Vec<TmuxMouseEffect> = Vec::new();

    for ev in buttons.read() {
        if ev.button != MouseButton::Left {
            continue;
        }
        if let Some(cursor_phys) = cursor_phys
            && let Some((terminal, _pane_id, local_phys)) = tmux_pane_at_phys(&panes, cursor_phys)
        {
            let consumed = route_tmux_webview_left_click(
                &mut webview_press,
                &mut webview_route,
                &panes,
                terminal,
                local_phys,
                cursor_phys,
                ev.state,
                cell_w,
                cell_h,
                scale,
            );
            if consumed {
                // NOTE: a press that focused a webview must also make its
                // host pane the tmux-active pane. ActivePane is the keyboard/paste
                // target, so it has to follow the pane the user clicked into —
                // without this, after focus is released keystrokes route to the
                // previously-active pane.
                if ev.state == ButtonState::Pressed
                    && let Ok((_, pane, _, _)) = panes.get(terminal)
                {
                    effects.push(TmuxMouseEffect::SelectPane(pane.id));
                }
                continue;
            }
        }
        match ev.state {
            ButtonState::Pressed => {
                // A press with no cursor cannot hit-test; skip it without
                // disturbing the gesture (matches the pre-refactor `continue`).
                let Some(cursor_phys) = cursor_phys else {
                    continue;
                };
                let Some(hit) = press_hit(
                    &panes,
                    packed_dividers,
                    cursor_phys,
                    scale,
                    grab_tol_logical,
                ) else {
                    continue;
                };
                let TmuxMouseGesture { state, click, .. } = &mut *gesture;
                effects.extend(decide_press(state, click, hit, time.elapsed(), dbl_click));
            }
            ButtonState::Released => {
                let ctx = release_ctx(
                    &gesture.state,
                    &panes,
                    &copy_gate,
                    cursor_phys,
                    cell_w,
                    cell_h,
                );
                effects.extend(decide_release(&mut gesture.state, ctx));
            }
        }
    }

    let ctx = continuation_ctx(
        &gesture.state,
        &panes,
        &copy_gate,
        cursor_phys,
        drag_threshold_phys,
        cell_w,
        cell_h,
    );
    let entity = gesture_pane_entity(&gesture.state);
    effects.extend(decide_continuation(&mut gesture.state, ctx));

    if !effects.is_empty() {
        let entity = entity
            .or_else(|| effect_target_entity(&panes, &effects))
            .unwrap_or(Entity::PLACEHOLDER);
        commands.trigger(TmuxMouseEffects { entity, effects });
    }
}

/// Hit-tests a left press at `cursor_phys` into a `PressHit`: a divider grab
/// (resolved to its primary pane's near edge + current size) takes priority over
/// a pane-body focus. A divider whose primary pane has no projected geometry yet
/// falls through to a pane focus rather than a bogus (0) resize baseline; `None`
/// means the press landed on neither.
fn press_hit(
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    dividers: &[DividerPixelRect],
    cursor_phys: Vec2,
    scale: f32,
    grab_tol_logical: f32,
) -> Option<PressHit> {
    let cursor_logical = cursor_phys / scale;
    let resize = divider_at(dividers, cursor_logical, grab_tol_logical).and_then(|d| {
        panes
            .iter()
            .find(|(_, p, _, _)| p.id == d.primary)
            .map(|(_, p, _, _)| match d.axis {
                DividerAxis::Vertical => (d, p.dims.xoff, p.dims.width),
                DividerAxis::Horizontal => (d, p.dims.yoff, p.dims.height),
            })
    });
    if let Some((divider, near, last_sent)) = resize {
        return Some(PressHit::Divider {
            divider,
            near,
            last_sent,
        });
    }
    let (pane, pane_id) = pane_under_cursor(panes, cursor_phys)?;
    Some(PressHit::Pane {
        pane,
        pane_id,
        origin_phys: cursor_phys,
        cursor_logical,
    })
}

/// Resolves the `ReleaseCtx` for the gesture's current (pre-release) state:
/// copy-mode + a `Pressed` multi-click's origin cell, and the pane under the
/// cursor for a `Resizing`-click focus fallback.
fn release_ctx(
    state: &GestureState,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    copy_gate: &CopyModeGate,
    cursor_phys: Option<Vec2>,
    cell_w: f32,
    cell_h: f32,
) -> ReleaseCtx {
    match *state {
        GestureState::Pressed {
            pane, origin_phys, ..
        } => {
            let copy_mode = copy_gate.copy_modes.get(pane).is_ok();
            let multi_cell = panes.get(pane).ok().and_then(|(_, p, node, transform)| {
                let cols = p.dims.width as u16;
                let rows = p.dims.height as u16;
                cell_at_pane(node, transform, origin_phys, cell_w, cell_h, cols, rows)
            });
            ReleaseCtx {
                copy_mode,
                multi_cell,
                pane_under: None,
            }
        }
        GestureState::Resizing { .. } => ReleaseCtx {
            copy_mode: false,
            multi_cell: None,
            pane_under: cursor_phys.and_then(|c| pane_under_cursor(panes, c).map(|(_, id)| id)),
        },
        _ => ReleaseCtx {
            copy_mode: false,
            multi_cell: None,
            pane_under: None,
        },
    }
}

/// Resolves the per-frame `ContinuationCtx` for the gesture's current state,
/// reading only the inputs the active arm needs (cursor + copy-mode + origin
/// anchor for `Pressed`; snapshot + live cell for `Selecting`; snapshot for
/// `PendingMultiSelect`; pointer cell for `Resizing`).
fn continuation_ctx(
    state: &GestureState,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    copy_gate: &CopyModeGate,
    cursor_phys: Option<Vec2>,
    drag_threshold_phys: f32,
    cell_w: f32,
    cell_h: f32,
) -> ContinuationCtx {
    let mut ctx = ContinuationCtx {
        pane_alive: false,
        cursor_phys,
        drag_threshold_phys,
        copy_mode: false,
        anchor_cell: None,
        snapshot_cursor: None,
        selecting_cell: None,
        resize_pointer_cell: None,
    };
    match *state {
        GestureState::Pressed {
            pane, origin_phys, ..
        } => {
            ctx.copy_mode = copy_gate.copy_modes.get(pane).is_ok();
            if let Ok((_, p, node, transform)) = panes.get(pane) {
                ctx.pane_alive = true;
                let cols = p.dims.width as u16;
                let rows = p.dims.height as u16;
                ctx.anchor_cell =
                    cell_at_pane(node, transform, origin_phys, cell_w, cell_h, cols, rows);
            }
        }
        GestureState::Selecting { pane, .. } => {
            if let Ok((_, p, node, transform)) = panes.get(pane) {
                ctx.pane_alive = true;
                ctx.snapshot_cursor = copy_gate
                    .snapshots
                    .get(pane)
                    .map(|s| (s.0.cursor_x, s.0.cursor_y))
                    .ok();
                if let Some(cursor_phys) = cursor_phys {
                    let cols = p.dims.width as u16;
                    let rows = p.dims.height as u16;
                    ctx.selecting_cell =
                        cell_at_pane(node, transform, cursor_phys, cell_w, cell_h, cols, rows);
                }
            }
        }
        GestureState::PendingMultiSelect { pane, .. } => {
            if panes.get(pane).is_ok() {
                ctx.pane_alive = true;
                ctx.snapshot_cursor = copy_gate
                    .snapshots
                    .get(pane)
                    .map(|s| (s.0.cursor_x, s.0.cursor_y))
                    .ok();
            }
        }
        GestureState::Resizing { divider, .. } => {
            ctx.pane_alive = true;
            ctx.resize_pointer_cell = cursor_phys.map(|c| match divider.axis {
                DividerAxis::Vertical => (c.x / cell_w).floor() as i32,
                DividerAxis::Horizontal => (c.y / cell_h).floor() as i32,
            });
        }
        GestureState::Idle => {}
    }
    ctx
}

/// The pane `Entity` carried by the active gesture state, used as the
/// `TmuxMouseEffects` event target. `Resizing`/`Idle` carry no pane entity.
fn gesture_pane_entity(state: &GestureState) -> Option<Entity> {
    match *state {
        GestureState::Pressed { pane, .. }
        | GestureState::PendingMultiSelect { pane, .. }
        | GestureState::Selecting { pane, .. } => Some(pane),
        GestureState::Resizing { .. } | GestureState::Idle => None,
    }
}

/// The first live pane `Entity` whose `PaneId` is targeted by an effect, used as
/// a `TmuxMouseEffects` target fallback when the gesture state carries none
/// (e.g. a `Resizing` resize or a consumed-press `SelectPane`).
fn effect_target_entity(
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    effects: &[TmuxMouseEffect],
) -> Option<Entity> {
    let target = effects.iter().find_map(effect_pane_id)?;
    panes
        .iter()
        .find(|(_, p, _, _)| p.id == target)
        .map(|(e, _, _, _)| e)
}

/// The `PaneId` an effect targets.
fn effect_pane_id(effect: &TmuxMouseEffect) -> Option<PaneId> {
    match *effect {
        TmuxMouseEffect::SelectPane(id)
        | TmuxMouseEffect::ResizePane { primary: id, .. }
        | TmuxMouseEffect::BeginCopyDrag { pane: id, .. }
        | TmuxMouseEffect::ExtendCopyDrag { pane: id, .. }
        | TmuxMouseEffect::MultiSelect { pane: id, .. }
        | TmuxMouseEffect::CopySelection { pane: id } => Some(id),
    }
}

/// Inline-routing params for the arbiter, bundled to stay within Bevy's
/// system-parameter limit. `focused_webview` / `browsers` are optional so
/// CEF-less tests construct the system (state effects still apply).
#[derive(SystemParam)]
struct TmuxWebviewRouteParams<'w, 's> {
    focused_webview: Option<ResMut<'w, FocusedWebview>>,
    children: Query<'w, 's, &'static Children>,
    webviews: Query<'w, 's, (&'static Webview, Has<NonInteractive>)>,
    webview_parents: Query<'w, 's, &'static ChildOf, With<Webview>>,
    overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    browsers: Option<NonSend<'w, Browsers>>,
}

/// Releases an in-flight webview press to CEF (mouse-up at the last
/// cursor) and clears the marker. Called when an arbiter guard drains the
/// queued release (modal open / window unfocused) so the focused web page is
/// not left logically pressed with no matching mouse-up.
fn release_webview_press(
    webview_press: &mut TmuxWebviewPress,
    route: &TmuxWebviewRouteParams,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    cursor_phys: Option<Vec2>,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) {
    let Some(child) = webview_press.0.take() else {
        return;
    };
    if let Some(cursor_phys) = cursor_phys
        && let Some(browsers) = route.browsers.as_deref()
        && let Some(dip) = tmux_webview_release_dip(
            route,
            panes,
            child,
            cursor_phys,
            cell_w_phys,
            cell_h_phys,
            scale,
        )
    {
        browsers.send_mouse_click(&child, dip, PointerButton::Primary, true);
    }
}

/// Routes a left press/release through the webview layer, returning
/// `true` when the event was consumed and must NOT reach the tmux gesture
/// pipeline. A press inside an
/// interactive rect sets `FocusedWebview`, issues the UNGATED `set_focus`
/// BEFORE the gated `send_mouse_click` (CEF drops clicks to a browser with no
/// `focused_frame()`, so the first click would otherwise be swallowed),
/// forwards the press in DIP, and records the in-flight press; a press outside
/// every rect clears an inline `FocusedWebview` and returns `false`. Release
/// forwards the click-up to the recorded child (drift-tolerant) and clears.
#[expect(
    clippy::too_many_arguments,
    reason = "inline routing needs the webview press state, route params, and pointer geometry"
)]
fn route_tmux_webview_left_click(
    webview_press: &mut TmuxWebviewPress,
    route: &mut TmuxWebviewRouteParams,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    terminal: Entity,
    local_phys: Vec2,
    cursor_phys: Vec2,
    button_state: ButtonState,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> bool {
    match button_state {
        ButtonState::Pressed => {
            webview_press.0 = None;
            let hit = route.overlay_rects.get(terminal).ok().and_then(|overlays| {
                webview_hit_at(
                    &route.children,
                    &route.webviews,
                    overlays,
                    terminal,
                    local_phys,
                    cell_w_phys,
                    cell_h_phys,
                    scale,
                )
            });
            let Some(hit) = hit else {
                if let Some(focused) = route.focused_webview.as_deref_mut()
                    && focused
                        .0
                        .is_some_and(|current| route.webview_parents.contains(current))
                {
                    focused.0 = None;
                }
                return false;
            };
            if let Some(focused) = route.focused_webview.as_deref_mut()
                && focused.0 != Some(hit.child)
            {
                focused.0 = Some(hit.child);
            }
            if let Some(browsers) = route.browsers.as_deref() {
                browsers.set_focus(&hit.child, true);
                browsers.send_mouse_click(&hit.child, hit.local_dip, PointerButton::Primary, false);
            }
            webview_press.0 = Some(hit.child);
            true
        }
        ButtonState::Released => {
            let Some(child) = webview_press.0.take() else {
                return false;
            };
            if let Some(browsers) = route.browsers.as_deref()
                && let Some(dip) = tmux_webview_release_dip(
                    route,
                    panes,
                    child,
                    cursor_phys,
                    cell_w_phys,
                    cell_h_phys,
                    scale,
                )
            {
                browsers.send_mouse_click(&child, dip, PointerButton::Primary, true);
            }
            true
        }
    }
}

/// Webview-local DIP for a release on `child`, WITHOUT containment (a pointer
/// that drifted off the rect still produces a release position). `None` when
/// the child/terminal/rect chain is gone.
fn tmux_webview_release_dip(
    route: &TmuxWebviewRouteParams,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    child: Entity,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    scale: f32,
) -> Option<Vec2> {
    let terminal = route.webview_parents.get(child).ok()?.parent();
    let (_, _, node, transform) = panes.get(terminal).ok()?;
    let local_phys = phys_to_pane_local(node, transform, cursor_phys)?;
    let (view, _) = route.webviews.get(child).ok()?;
    webview_local_dip(
        route.overlay_rects.get(terminal).ok()?,
        view.slot,
        local_phys,
        cell_w_phys,
        cell_h_phys,
        scale,
    )
}

/// Forwards pointer motion over an interactive inline rect of a tmux pane to
/// the child's CEF browser (`send_mouse_move`, webview-local DIP), forwarding
/// whatever mouse buttons are held so the one system serves both hover and an
/// in-rect drag. `CursorMoved`-driven (one forward per frame, latest position), and
/// focus-gated inside `bevy_cef` so motion over an unfocused browser is
/// dropped browser-side. `Browsers` is optional so CEF-less tests construct it.
fn forward_tmux_webview_mouse_moves(
    mut cursor_msg: MessageReader<CursorMoved>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: Query<&Children>,
    webviews: Query<(&Webview, Has<NonInteractive>)>,
    overlay_rects: Query<&TerminalOverlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    picker: Res<SessionPicker>,
    copy_prompt: Res<CopyPrompt>,
    browsers: Option<NonSend<Browsers>>,
) {
    let Some(moved) = cursor_msg.read().last() else {
        return;
    };
    if picker.open || copy_prompt.open.is_some() {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = window.scale_factor();
    let cursor_phys = moved.position * scale;
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some((terminal, _pane_id, local_phys)) = tmux_pane_at_phys(&panes, cursor_phys) else {
        return;
    };
    let Ok(overlays) = overlay_rects.get(terminal) else {
        return;
    };
    let Some(hit) = webview_hit_at(
        &children, &webviews, overlays, terminal, local_phys, cell_w, cell_h, scale,
    ) else {
        return;
    };
    if let Some(browsers) = browsers.as_ref() {
        browsers.send_mouse_move(
            &hit.child,
            mouse_buttons.get_pressed(),
            hit.local_dip,
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::mouse::MouseButtonInput;
    use ozma_tty_renderer::CellMetrics;
    use ozmux_tmux::{CopyModeQueries, TmuxConnection};

    #[test]
    fn gesture_state_default_is_idle() {
        assert_eq!(GestureState::default(), GestureState::Idle);
    }

    fn test_metrics() -> TerminalCellMetricsResource {
        TerminalCellMetricsResource {
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
        }
    }

    #[test]
    fn left_press_without_cursor_stays_idle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.init_resource::<TmuxMouseGesture>();
        app.init_resource::<TmuxWebviewPress>();
        app.init_resource::<CopyModeQueries>();
        app.init_resource::<SessionPicker>();
        app.init_resource::<CopyPrompt>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(test_metrics());
        app.add_systems(Update, arbiter);
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: bevy::input::mouse::MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            });
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle
        );
    }

    fn make_arbiter_webview_app() -> (App, Entity, Entity) {
        use bevy::window::WindowResolution;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.init_resource::<TmuxMouseGesture>();
        app.init_resource::<TmuxWebviewPress>();
        app.init_resource::<CopyModeQueries>();
        app.init_resource::<SessionPicker>();
        app.init_resource::<CopyPrompt>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(test_metrics());
        app.add_systems(Update, arbiter);
        app.add_observer(apply::on_tmux_mouse_effects);

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

    fn set_cursor(app: &mut App, phys: Vec2) {
        use bevy::math::DVec2;

        let win = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Window>(win)
            .unwrap()
            .set_physical_cursor_position(Some(DVec2::new(phys.x as f64, phys.y as f64)));
    }

    fn write_button(app: &mut App, button: bevy::input::mouse::MouseButton, state: ButtonState) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button,
                state,
                window: Entity::PLACEHOLDER,
            });
    }

    #[test]
    fn webview_press_focuses_child_and_consumes() {
        let (mut app, _pane, child) = make_arbiter_webview_app();
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "a press inside an interactive inline rect must focus that child"
        );
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "a consumed inline press must NOT arm a Pressed/Selecting gesture"
        );
    }

    #[test]
    fn move_resolves_inline_child_over_rect() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut app, _pane, child) = make_arbiter_webview_app();
        let hit = app
            .world_mut()
            .run_system_once(
                move |panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
                      children: Query<&Children>,
                      webviews: Query<(&Webview, Has<NonInteractive>)>,
                      overlays: Query<&TerminalOverlays>| {
                    let (terminal, _pane_id, local) =
                        tmux_pane_at_phys(&panes, Vec2::new(40.0, 48.0)).unwrap();
                    webview_hit_at(
                        &children,
                        &webviews,
                        overlays.get(terminal).unwrap(),
                        terminal,
                        local,
                        8.0,
                        16.0,
                        1.0,
                    )
                    .map(|h| h.child)
                },
            )
            .unwrap();
        assert_eq!(
            hit,
            Some(child),
            "pointer over the rect must resolve the inline child"
        );
    }

    #[test]
    fn move_resolves_nothing_off_rect() {
        use bevy::ecs::system::RunSystemOnce;

        let (mut app, _pane, _child) = make_arbiter_webview_app();
        let hit = app
            .world_mut()
            .run_system_once(
                |panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
                 children: Query<&Children>,
                 webviews: Query<(&Webview, Has<NonInteractive>)>,
                 overlays: Query<&TerminalOverlays>| {
                    let (terminal, _pane_id, local) =
                        tmux_pane_at_phys(&panes, Vec2::new(400.0, 400.0)).unwrap();
                    webview_hit_at(
                        &children,
                        &webviews,
                        overlays.get(terminal).unwrap(),
                        terminal,
                        local,
                        8.0,
                        16.0,
                        1.0,
                    )
                    .map(|h| h.child)
                },
            )
            .unwrap();
        assert_eq!(
            hit, None,
            "pointer over terminal text must resolve no inline child"
        );
    }

    #[test]
    fn inline_off_rect_press_releases_focus_and_falls_through() {
        let (mut app, pane, child) = make_arbiter_webview_app();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "an off-rect press must release inline focus"
        );
        assert!(
            matches!(
                app.world().resource::<TmuxMouseGesture>().state,
                GestureState::Pressed { pane: p, .. } if p == pane
            ),
            "an off-rect press must fall through to the normal pane gesture"
        );
    }
}
