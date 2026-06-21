//! Webview pointer routing for the tmux mouse gesture system.
//!
//! Owns `TmuxWebviewPress`, `TmuxWebviewRouteParams`, and the systems that
//! forward left-button press/release and pointer motion to the inline CEF
//! child under the cursor.

use super::super::pane_hit::{phys_to_pane_local, tmux_pane_at_phys};
use super::effect::{TmuxMouseEffect, TmuxMouseEffects};
use super::{GestureState, TmuxGestureButtons, TmuxMouseGesture, cell_dims};
use crate::picker::SessionPicker;
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
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozmux_tmux::TmuxPane;

/// Tracks the CEF child that is currently pressed (a left press inside an
/// interactive inline rect was forwarded to it) so the matching release routes
/// to the same child even if the pointer drifted off-rect.
#[derive(Resource, Default)]
pub(super) struct TmuxWebviewPress(pub(super) Option<Entity>);

/// Inline-routing params for `tmux_webview_pointer`, bundled to stay within
/// Bevy's system-parameter limit. `focused_webview` / `browsers` are optional
/// so CEF-less tests construct the system (state effects still apply).
#[derive(SystemParam)]
pub(super) struct TmuxWebviewRouteParams<'w, 's> {
    pub(super) focused_webview: Option<ResMut<'w, FocusedWebview>>,
    pub(super) children: Query<'w, 's, &'static Children>,
    pub(super) webviews: Query<'w, 's, (&'static Webview, Has<NonInteractive>)>,
    pub(super) webview_parents: Query<'w, 's, &'static ChildOf, With<Webview>>,
    pub(super) overlay_rects: Query<'w, 's, &'static TerminalOverlays>,
    pub(super) browsers: Option<NonSend<'w, Browsers>>,
}

/// Releases an in-flight webview press to CEF (mouse-up at the last
/// cursor) and clears the marker. Called by `tmux_webview_pointer` on the
/// suppressed path (modal open / window unfocused) so the focused web page is
/// not left logically pressed with no matching mouse-up.
pub(super) fn release_webview_press(
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

/// Offers each frame's left-button events to the inline webview layer BEFORE
/// `tmux_gesture` (`.chain()`), handing off the non-consumed ones through
/// `TmuxGestureButtons`.
///
/// This system owns the single `MouseButtonInput` reader for the tmux pointer
/// pipeline and runs EVERY frame within `OzmuxActiveSet` (it is NOT gated by
/// `pointer_active`); it computes the suppressed/active decision in-body. On a
/// suppressed frame (no focused primary window, or a picker / copy-search prompt
/// owns input) it drains the reader and the buffer, resets the gesture to
/// `Idle`, and resolves an in-flight inline press: with no window it drops
/// `TmuxWebviewPress` WITHOUT a CEF mouse-up (there is no cursor/scale to place
/// it); with a window present it `release_webview_press`es so the focused page
/// is not left logically pressed with no matching mouse-up.
///
/// On an active frame `TmuxGestureButtons` is cleared at the start (invariant 7)
/// so non-consumed events never accumulate across frames. Non-`Left` events are
/// skipped (never buffered). Each `Left` event is routed through
/// `route_tmux_webview_left_click`; a consumed press additionally triggers
/// `SelectPane(host_pane)` so the keyboard/paste target follows the click
/// (invariant 3, `Pressed` only), and a non-consumed event is pushed into the
/// buffer for `tmux_gesture` to drain.
// NOTE: this system MUST run every frame and own the only `MouseButtonInput`
// reader. Gating it with `run_if(pointer_active)` would freeze its reader cursor
// while suppressed, so a press written during the last suppressed frame would be
// re-read once the pointer reactivates — leaking a stale press into the active
// pipeline (spurious select-pane / inline CEF click). The old single
// always-running arbiter reader could never resurface a suppressed-frame press.
pub(super) fn tmux_webview_pointer(
    mut commands: Commands,
    mut buffer: ResMut<TmuxGestureButtons>,
    mut webview_press: ResMut<TmuxWebviewPress>,
    mut gesture: ResMut<TmuxMouseGesture>,
    mut webview_route: TmuxWebviewRouteParams,
    mut buttons: MessageReader<MouseButtonInput>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    metrics: Res<TerminalCellMetricsResource>,
    picker: Res<SessionPicker>,
    copy_prompt: Res<CopyPrompt>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    buffer.0.clear();
    let Ok(window) = windows.single() else {
        buttons.clear();
        webview_press.0 = None;
        gesture.state = GestureState::Idle;
        return;
    };
    let scale = window.scale_factor();
    let (cell_w, cell_h) = cell_dims(&metrics);
    let cursor_phys = window.cursor_position().map(|c| c * scale);
    if !window.focused || picker.open || copy_prompt.open.is_some() {
        buttons.clear();
        gesture.state = GestureState::Idle;
        release_webview_press(
            &mut webview_press,
            &webview_route,
            &panes,
            cursor_phys,
            cell_w,
            cell_h,
            scale,
        );
        return;
    }
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

/// Forwards pointer motion over an interactive inline rect of a tmux pane to
/// the child's CEF browser (`send_mouse_move`, webview-local DIP), forwarding
/// whatever mouse buttons are held so the one system serves both hover and an
/// in-rect drag. `CursorMoved`-driven (one forward per frame, latest position), and
/// focus-gated inside `bevy_cef` so motion over an unfocused browser is
/// dropped browser-side. `Browsers` is optional so CEF-less tests construct it.
pub(super) fn forward_tmux_webview_mouse_moves(
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
    let (cell_w, cell_h) = cell_dims(&metrics);
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
