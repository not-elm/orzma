//! Tmux left-button gesture pipeline and its webview arbiter, co-located.
//!
//! `tmux_webview_pointer` is the gather stage: it owns the single
//! `MouseButtonInput` reader, runs every frame, offers each left-button event to
//! the inline CEF webview layer, triggers `SelectPane` for a consumed inline
//! click, and hands the non-consumed events to `tmux_gesture` through
//! `TmuxGestureButtons` (resetting `GestureState` to `Idle` on a suppressed
//! frame). `tmux_gesture` reads that buffer and calls the pure deciders
//! (`decide_press`, `decide_release`, `decide_continuation`) which return
//! `TmuxMouseEffect`s; `on_tmux_mouse_effects` (observer in `apply`) applies them:
//! `SelectPane`/`ResizePane` send tmux control-mode commands, and the copy-drag
//! variants trigger local `TerminalSelection*` events on the pane's own terminal
//! handle. `forward_tmux_webview_mouse_moves` forwards pointer motion over an
//! interactive inline rect to the child's CEF browser.

mod apply;
mod decide;
mod effect;

use crate::app_mode::TmuxActiveSet;
use crate::configs::OrzmaConfigsResource;
use crate::input::InputPhase;
use crate::input::mouse::cell_dims;
use crate::input::mouse::gesture::ClickTracker;
use crate::input::mouse::webview::{
    WebviewMoveDeps, WebviewPress, WebviewRouteParams, forward_webview_move_at,
    release_webview_press, route_webview_left_click, webview_pointer_frame,
};
use crate::input::tmux::pane_hit::tmux_pane_at_phys;
use crate::render::tmux::{DividerPixelRect, PackedTmuxLayout};
use crate::surface::geometry::{Side as CellSide, cell_at_pane};
use crate::ui::text_prompt::ActiveTextPrompt;
use crate::ui::vi_mode::ViModeState;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_cef_core::prelude::Browsers;
pub(crate) use decide::divider_at;
use decide::{
    ContinuationCtx, PressHit, ReleaseCtx, decide_continuation, decide_press, decide_release,
};
use effect::{MultiSelectKind, TmuxMouseEffect, TmuxMouseEffects};
use orzma_tmux::{ActiveWindow, PaneId, TmuxPane};
use orzma_tty_engine::{Column, Line, Point, SelectionType, Side};
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::prelude::TerminalOverlays;
use orzma_webview::{NonInteractive, Webview};
use std::time::Duration;
use tmux_control_parser::DividerAxis;

/// Bevy plugin that registers the tmux left-button gesture pipeline: the webview
/// pointer arbiter (gather), the gesture state machine (decide), and the apply
/// observer.
pub(super) struct MouseButtonTmuxPlugin;

impl Plugin for MouseButtonTmuxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxMouseGesture>()
            .init_resource::<TmuxGestureButtons>()
            .add_systems(
                Update,
                tmux_webview_pointer
                    .in_set(InputPhase::Dispatch)
                    .in_set(TmuxActiveSet),
            )
            .add_systems(
                Update,
                forward_tmux_webview_mouse_moves
                    .in_set(InputPhase::Hover)
                    .in_set(TmuxActiveSet),
            )
            .add_systems(
                Update,
                tmux_gesture
                    .run_if(pointer_active)
                    .after(tmux_webview_pointer)
                    .in_set(InputPhase::Dispatch)
                    .in_set(TmuxActiveSet),
            )
            .add_plugins(apply::ApplyPlugin);
    }
}

/// Offers each frame's left-button events to the inline webview layer BEFORE
/// `tmux_gesture` (which is ordered after it via `.after(tmux_webview_pointer)`),
/// handing off the non-consumed ones through `TmuxGestureButtons`.
///
/// This system owns the single `MouseButtonInput` reader for the tmux pointer
/// pipeline and runs EVERY frame within `TmuxActiveSet` (it is NOT gated by
/// `pointer_active`); it computes the suppressed/active decision in-body. On a
/// suppressed frame (no focused primary window, or a text prompt owns
/// input) it drains the reader and the buffer, resets the gesture to
/// `Idle`, and resolves an in-flight inline press: with no window it drops
/// `WebviewPress` WITHOUT a CEF mouse-up (there is no cursor/scale to place
/// it); with a window present it `release_webview_press`es so the focused page
/// is not left logically pressed with no matching mouse-up.
///
/// On an active frame `TmuxGestureButtons` is cleared at the start (invariant 7)
/// so non-consumed events never accumulate across frames. Non-`Left` events are
/// skipped (never buffered). Each `Left` event is routed through
/// `route_webview_left_click`; a consumed press additionally triggers
/// `SelectPane(host_pane)` so the keyboard/paste target follows the click
/// (invariant 3, `Pressed` only), and a non-consumed event is pushed into the
/// buffer for `tmux_gesture` to drain.
// NOTE: this system MUST run every frame and own the only `MouseButtonInput`
// reader. Gating it with `run_if(pointer_active)` would freeze its reader cursor
// while suppressed, so a press written during the last suppressed frame would be
// re-read once the pointer reactivates — leaking a stale press into the active
// pipeline (spurious select-pane / inline CEF click). The old single
// always-running arbiter reader could never resurface a suppressed-frame press.
fn tmux_webview_pointer(
    mut commands: Commands,
    mut buffer: ResMut<TmuxGestureButtons>,
    mut webview_press: ResMut<WebviewPress>,
    mut gesture: ResMut<TmuxMouseGesture>,
    mut webview_route: WebviewRouteParams,
    mut buttons: MessageReader<MouseButtonInput>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    metrics: Res<TerminalCellMetricsResource>,
    active_text_prompt: Res<ActiveTextPrompt>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    buffer.0.clear();
    let Ok(window) = windows.single() else {
        buttons.clear();
        webview_press.0 = None;
        gesture.state = GestureState::Idle;
        return;
    };
    let frame = webview_pointer_frame(window, &metrics);
    if !window.focused || active_text_prompt.0.is_some() {
        buttons.clear();
        gesture.state = GestureState::Idle;
        release_webview_press(
            &mut webview_press,
            &webview_route,
            frame.cursor_phys,
            frame.cell_w,
            frame.cell_h,
            frame.scale,
        );
        return;
    }
    for ev in buttons.read() {
        if ev.button != MouseButton::Left {
            continue;
        }
        if let Some(cursor_phys) = frame.cursor_phys
            && let Some((terminal, _pane_id, local_phys)) = tmux_pane_at_phys(&panes, cursor_phys)
        {
            let consumed = route_webview_left_click(
                &mut webview_press,
                &mut webview_route,
                terminal,
                local_phys,
                cursor_phys,
                ev.state,
                frame.cell_w,
                frame.cell_h,
                frame.scale,
            );
            if consumed {
                if ev.state == ButtonState::Pressed
                    && let Ok((_, pane, _, _)) = panes.get(terminal)
                {
                    commands.trigger(TmuxMouseEffects {
                        entity: terminal,
                        effects: vec![TmuxMouseEffect::SelectPane(pane.id)],
                    });
                }
                continue;
            }
        }
        buffer.0.push(*ev);
    }
}

/// Forwards pointer motion over an interactive inline rect of the tmux pane
/// under the cursor to the child's CEF browser via the shared
/// `forward_webview_move_at`. `CursorMoved`-driven (one forward per frame, latest
/// position), so the one system serves both hover and an in-rect drag. Skipped
/// while a text prompt owns input. `Browsers` is optional so CEF-less
/// tests construct it.
fn forward_tmux_webview_mouse_moves(
    mut cursor_msg: MessageReader<CursorMoved>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: Query<'_, '_, &'static Children>,
    webviews: Query<'_, '_, (&'static Webview, Has<NonInteractive>)>,
    overlay_rects: Query<'_, '_, &'static TerminalOverlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    active_text_prompt: Res<ActiveTextPrompt>,
    browsers: Option<NonSend<Browsers>>,
) {
    let Some(moved) = cursor_msg.read().last() else {
        return;
    };
    if active_text_prompt.0.is_some() {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let frame = webview_pointer_frame(window, &metrics);
    let cursor_phys = moved.position * frame.scale;
    let deps = WebviewMoveDeps {
        children: &children,
        webviews: &webviews,
        overlay_rects: &overlay_rects,
        browsers: browsers.as_deref(),
        pressed_buttons: &mouse_buttons,
    };
    forward_webview_move_at(
        &deps,
        |c| {
            tmux_pane_at_phys(&panes, c)
                .map(|(terminal, _pane_id, local_phys)| (terminal, local_phys))
        },
        cursor_phys,
        &frame,
    );
}

/// Hand-off buffer from `tmux_webview_pointer` to `tmux_gesture`: the
/// frame's left-button events the webview layer did NOT consume.
///
/// `tmux_webview_pointer` clears it at the start of every run and pushes each
/// non-consumed event; `tmux_gesture` drains it via `drain(..)` (keeps the
/// allocation for reuse). It holds only non-consumed `Left` events and never
/// accumulates across frames.
#[derive(Resource, Default)]
struct TmuxGestureButtons(Vec<MouseButtonInput>);

/// Run condition for the active tmux gesture pipeline: `true` iff a focused
/// primary window exists AND no modal (text prompt) owns input.
///
/// Gates `tmux_gesture` only (which reads the hand-off buffer, not
/// `MouseButtonInput`). `tmux_webview_pointer` is NOT gated by this — it owns the
/// single `MouseButtonInput` reader and runs every frame, computing the same
/// suppressed/active decision in-body. The focused-webview case is NOT a
/// suppressor here — the `tmux_webview_pointer` pre-step owns webview focus.
fn pointer_active(
    windows: Query<&Window, With<PrimaryWindow>>,
    active_text_prompt: Res<ActiveTextPrompt>,
) -> bool {
    windows.single().is_ok_and(|window| window.focused) && active_text_prompt.0.is_none()
}

/// Bundles the immutable vi-mode query read used by `tmux_gesture`.
#[derive(SystemParam)]
struct ViModeGate<'w, 's> {
    vi_modes: Query<'w, 's, (), With<ViModeState>>,
}

/// The current phase of a left-button gesture over a tmux pane.
#[derive(Default, Debug, PartialEq)]
enum GestureState {
    /// No button is held; `tmux_gesture` is waiting for the next press.
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
    /// A double/triple click resolved into a pending word/line selection at
    /// `cell`, completed on the next continuation frame on the pane's local
    /// terminal handle.
    PendingMultiSelect {
        pane: Entity,
        cell: Point,
        kind: MultiSelectKind,
    },
    /// Selecting text in a pane via tmux vi-mode (entered on drag-start).
    Selecting {
        pane: Entity,
        anchor: Point,
        begun: bool,
        last_target: Option<Point>,
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
struct TmuxMouseGesture {
    state: GestureState,
    click: ClickTracker,
}

/// Converts a `cell_at_pane` `(col, row)` result into the viewport-relative
/// `Point` the local `TerminalSelection*` events expect. Both are 0-indexed,
/// so no offset is needed.
fn point_from_cell((col, row): (u16, u16)) -> Point {
    Point::new(Line(row as i32), Column(col as usize))
}

/// Converts `cell_at_pane`'s pane-local half-cell [`CellSide`] into the
/// engine's [`Side`] the local `TerminalSelection*` events expect.
fn engine_side(side: CellSide) -> Side {
    match side {
        CellSide::Left => Side::Left,
        CellSide::Right => Side::Right,
    }
}

/// Interprets the non-consumed left-button events handed off by
/// `tmux_webview_pointer` into tmux `select-pane` / `resize-pane` commands, or
/// local `TerminalSelection*` events on the pane's own terminal handle.
///
/// On each `Pressed` event the cursor's physical position is hit-tested: a
/// press within a divider's grab zone (whose primary pane has geometry) enters
/// `Resizing`; otherwise the pane under the cursor is focused (`select-pane`)
/// and the state becomes `Pressed`. While `Pressed`, a pointer that drags past
/// `drag_threshold_px` transitions to `Selecting` when the pane is already in
/// vi mode (drag/selection for a pane NOT in vi mode is owned by
/// the local terminal path). Multi-click (≥2) on a pane in vi mode enters
/// `PendingMultiSelect`, which starts a word/line selection on the pane's
/// local terminal handle on the very next continuation frame — this is
/// local-only and needs no `TmuxClient`. Each frame while `Resizing` the
/// pointer's major-axis cell coordinate is mapped to an absolute target size
/// and sent as `resize-pane -x/-y` whenever the target changes. On
/// `Released` from `Selecting` a begun selection is copied to clipboard;
/// from `Resizing` that never dragged, the pane under the cursor is focused
/// as a fallback click.
///
/// Gated by `run_if(pointer_active)`: this system runs only when a focused
/// primary window exists and no modal (text prompt) owns input.
/// The suppressed path — clearing the buffer, resetting the gesture, and
/// releasing or dropping an in-flight inline press — is owned by
/// `tmux_webview_pointer`, which runs every frame upstream of this system.
///
/// The webview pre-step runs UPSTREAM in `tmux_webview_pointer` (ordered before
/// this system via `.after(tmux_webview_pointer)`): a press inside an interactive
/// inline rect focuses + forwards to
/// the child's CEF browser and is consumed (it never reaches this buffer, but its
/// host pane is still `select-pane`d); a press outside every rect drops inline
/// focus and is buffered here as a normal pane gesture.
fn tmux_gesture(
    mut commands: Commands,
    mut gesture: ResMut<TmuxMouseGesture>,
    mut buttons: ResMut<TmuxGestureButtons>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    packed_q: Query<&PackedTmuxLayout, With<ActiveWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    configs: Option<Res<OrzmaConfigsResource>>,
    vi_gate: ViModeGate,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = window.scale_factor();
    let (cell_w, cell_h) = cell_dims(&metrics);
    let cursor_phys = window.cursor_position().map(|c| c * scale);

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

    for ev in buttons.0.drain(..) {
        match ev.state {
            ButtonState::Pressed => {
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
                    &vi_gate,
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
        &vi_gate,
        cursor_phys,
        drag_threshold_phys,
        cell_w,
        cell_h,
    );
    effects.extend(decide_continuation(&mut gesture.state, ctx));

    if !effects.is_empty() {
        // NOTE: Entity::PLACEHOLDER is correct because on_tmux_mouse_effects is a global observer
        // (app.add_observer) that never reads ev.entity() — every effect variant carries its own
        // PaneId. Must be revisited if the observer becomes entity-scoped or starts reading ev.entity().
        commands.trigger(TmuxMouseEffects {
            entity: Entity::PLACEHOLDER,
            effects,
        });
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
    let (pane, pane_id, _) = tmux_pane_at_phys(panes, cursor_phys)?;
    Some(PressHit::Pane {
        pane,
        pane_id,
        origin_phys: cursor_phys,
        cursor_logical,
    })
}

/// Resolves the `ReleaseCtx` for the gesture's current (pre-release) state:
/// vi-mode + a `Pressed` multi-click's origin point, and the pane under the
/// cursor for a `Resizing`-click focus fallback.
fn release_ctx(
    state: &GestureState,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    vi_gate: &ViModeGate,
    cursor_phys: Option<Vec2>,
    cell_w: f32,
    cell_h: f32,
) -> ReleaseCtx {
    match *state {
        GestureState::Pressed {
            pane, origin_phys, ..
        } => {
            let vi_mode = vi_gate.vi_modes.get(pane).is_ok();
            let multi_cell = panes
                .get(pane)
                .ok()
                .and_then(|(_, p, node, transform)| {
                    let cols = p.dims.width as u16;
                    let rows = p.dims.height as u16;
                    cell_at_pane(node, transform, origin_phys, cell_w, cell_h, cols, rows)
                })
                .map(|(col, row, _)| point_from_cell((col, row)));
            ReleaseCtx {
                vi_mode,
                multi_cell,
                pane_under: None,
            }
        }
        GestureState::Resizing { .. } => ReleaseCtx {
            vi_mode: false,
            multi_cell: None,
            pane_under: cursor_phys.and_then(|c| tmux_pane_at_phys(panes, c).map(|(_, id, _)| id)),
        },
        _ => ReleaseCtx {
            vi_mode: false,
            multi_cell: None,
            pane_under: None,
        },
    }
}

/// Resolves the per-frame `ContinuationCtx` for the gesture's current state,
/// reading only the inputs the active arm needs (cursor + vi-mode + origin
/// anchor point for `Pressed`; live selecting point for `Selecting`; pointer
/// cell for `Resizing`). `side` is resolved from whichever point the active
/// arm reads (the press origin for `Pressed`, the live cursor for
/// `Selecting`); `ty` is always `Simple` for a plain drag.
fn continuation_ctx(
    state: &GestureState,
    panes: &Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    vi_gate: &ViModeGate,
    cursor_phys: Option<Vec2>,
    drag_threshold_phys: f32,
    cell_w: f32,
    cell_h: f32,
) -> ContinuationCtx {
    let mut ctx = ContinuationCtx {
        pane_alive: false,
        cursor_phys,
        drag_threshold_phys,
        vi_mode: false,
        anchor_point: None,
        selecting_point: None,
        side: Side::Left,
        ty: SelectionType::Simple,
        resize_pointer_cell: None,
    };
    match *state {
        GestureState::Pressed {
            pane, origin_phys, ..
        } => {
            ctx.vi_mode = vi_gate.vi_modes.get(pane).is_ok();
            if let Ok((_, p, node, transform)) = panes.get(pane) {
                ctx.pane_alive = true;
                let cols = p.dims.width as u16;
                let rows = p.dims.height as u16;
                if let Some((col, row, side)) =
                    cell_at_pane(node, transform, origin_phys, cell_w, cell_h, cols, rows)
                {
                    ctx.anchor_point = Some(point_from_cell((col, row)));
                    ctx.side = engine_side(side);
                }
            }
        }
        GestureState::Selecting { pane, .. } => {
            if let Ok((_, p, node, transform)) = panes.get(pane) {
                ctx.pane_alive = true;
                if let Some(cursor_phys) = cursor_phys {
                    let cols = p.dims.width as u16;
                    let rows = p.dims.height as u16;
                    if let Some((col, row, side)) =
                        cell_at_pane(node, transform, cursor_phys, cell_w, cell_h, cols, rows)
                    {
                        ctx.selecting_point = Some(point_from_cell((col, row)));
                        ctx.side = engine_side(side);
                    }
                }
            }
        }
        GestureState::PendingMultiSelect { pane, .. } => {
            ctx.pane_alive = panes.get(pane).is_ok();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::mouse::webview::WebviewPress;
    use crate::input::tmux::pane_hit::tmux_pane_at_phys;
    use bevy::input::ButtonState;
    use bevy::input::mouse::MouseButtonInput;
    use bevy_cef::prelude::FocusedWebview;
    use orzma_tty_renderer::CellMetrics;
    use orzma_tty_renderer::prelude::TerminalOverlays;
    use orzma_webview::{NonInteractive, Webview, webview_hit_at};

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

    fn set_modal_open(app: &mut App, open: bool) {
        let target = if open {
            Some(app.world_mut().spawn_empty().id())
        } else {
            None
        };
        app.world_mut().resource_mut::<ActiveTextPrompt>().0 = target;
    }

    #[test]
    fn left_press_without_cursor_stays_idle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.init_resource::<TmuxMouseGesture>();
        app.init_resource::<TmuxGestureButtons>();
        app.init_resource::<WebviewPress>();
        app.init_resource::<ActiveTextPrompt>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(test_metrics());
        app.add_systems(
            Update,
            (tmux_webview_pointer, tmux_gesture.run_if(pointer_active)).chain(),
        );
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

    fn make_gesture_webview_app() -> (App, Entity, Entity) {
        use bevy::window::WindowResolution;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.init_resource::<TmuxMouseGesture>();
        app.init_resource::<TmuxGestureButtons>();
        app.init_resource::<WebviewPress>();
        app.init_resource::<ActiveTextPrompt>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(test_metrics());
        app.add_systems(
            Update,
            (tmux_webview_pointer, tmux_gesture.run_if(pointer_active)).chain(),
        );
        app.add_plugins(apply::ApplyPlugin);

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
        let (mut app, _pane, child) = make_gesture_webview_app();
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

        let (mut app, _pane, child) = make_gesture_webview_app();
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

        let (mut app, _pane, _child) = make_gesture_webview_app();
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
        let (mut app, pane, child) = make_gesture_webview_app();
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

    #[test]
    fn modal_open_suppresses_webview_routing_and_gesture() {
        let (mut app, _pane, _child) = make_gesture_webview_app();
        set_modal_open(&mut app, true);
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "a press while a modal is open must not route to the webview layer"
        );
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "a press while a modal is open must not arm a gesture"
        );
        assert!(
            app.world().resource::<TmuxGestureButtons>().0.is_empty(),
            "a gated frame must buffer no events"
        );
    }

    #[test]
    fn closing_modal_resumes_pointer_handling() {
        let (mut app, pane, _child) = make_gesture_webview_app();
        set_modal_open(&mut app, true);
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "while the modal is open the suppressed drain keeps the gesture Idle"
        );

        set_modal_open(&mut app, false);
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert!(
            matches!(
                app.world().resource::<TmuxMouseGesture>().state,
                GestureState::Pressed { pane: p, .. } if p == pane
            ),
            "closing the modal must let the active path arm the gesture again"
        );
    }

    #[test]
    fn suppressed_frame_press_does_not_resurface_when_pointer_reactivates() {
        let (mut app, _pane, _child) = make_gesture_webview_app();
        set_modal_open(&mut app, true);
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "the suppressed frame must not arm a gesture"
        );

        set_modal_open(&mut app, false);
        app.update();
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "a press written during the suppressed frame must not resurface and arm a \
             gesture once the pointer pipeline reactivates"
        );
        assert!(
            app.world().resource::<TmuxGestureButtons>().0.is_empty(),
            "no stale event may reach the hand-off buffer on the reactivated frame"
        );
    }

    #[test]
    fn suppressed_frame_releases_in_flight_webview_press() {
        let (mut app, _pane, child) = make_gesture_webview_app();
        app.world_mut().resource_mut::<WebviewPress>().0 = Some(child);
        set_modal_open(&mut app, true);
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert_eq!(
            app.world().resource::<WebviewPress>().0,
            None,
            "a window-exists-but-suppressed frame must release the in-flight inline press"
        );
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle,
            "the suppressed drain resets the gesture to Idle"
        );
    }

    #[test]
    fn non_consumed_press_is_handed_off_and_buffer_drained() {
        let (mut app, pane, _child) = make_gesture_webview_app();
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_button(
            &mut app,
            bevy::input::mouse::MouseButton::Left,
            ButtonState::Pressed,
        );
        app.update();
        assert!(
            matches!(
                app.world().resource::<TmuxMouseGesture>().state,
                GestureState::Pressed { pane: p, .. } if p == pane
            ),
            "a non-consumed press must reach tmux_gesture through the buffer and arm the gesture"
        );
        assert!(
            app.world().resource::<TmuxGestureButtons>().0.is_empty(),
            "tmux_gesture must drain the hand-off buffer (invariant 7: no cross-frame accumulation)"
        );
    }
}
