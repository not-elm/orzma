//! Webview pointer routing for the tmux mouse gesture system.
//!
//! Resolves the inline CEF child under the cursor on the tmux pane the pointer
//! is over (`tmux_pane_at_phys`) and delegates the CEF forwarding + focus to the
//! mode-agnostic core in `crate::input::mouse::webview`. Owns the tmux-specific glue:
//! the `TmuxGestureButtons` hand-off of non-consumed events to `tmux_gesture`,
//! and the `SelectPane` trigger so the keyboard/paste target follows a consumed
//! inline click.

use super::super::pane_hit::tmux_pane_at_phys;
use super::effect::{TmuxMouseEffect, TmuxMouseEffects};
use super::{GestureState, TmuxGestureButtons, TmuxMouseGesture, cell_dims};
use crate::app_mode::TmuxActiveSet;
use crate::input::InputPhase;
use crate::input::mouse::webview::{
    WebviewPress, WebviewRouteParams, forward_webview_move, release_webview_press,
    route_webview_left_click,
};
use crate::ui::copy_search::CopyPrompt;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_cef_core::prelude::Browsers;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::prelude::TerminalOverlays;
use ozma_webview::{NonInteractive, Webview};
use ozmux_tmux::TmuxPane;

/// Plugin that registers the tmux webview pointer-routing systems and the shared
/// `WebviewPress` resource.
pub(super) struct WebviewPointerPlugin;

impl Plugin for WebviewPointerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WebviewPress>()
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
            );
    }
}

/// Offers each frame's left-button events to the inline webview layer BEFORE
/// `tmux_gesture` (which is ordered after it via `.after(tmux_webview_pointer)`),
/// handing off the non-consumed ones through `TmuxGestureButtons`.
///
/// This system owns the single `MouseButtonInput` reader for the tmux pointer
/// pipeline and runs EVERY frame within `TmuxActiveSet` (it is NOT gated by
/// `pointer_active`); it computes the suppressed/active decision in-body. On a
/// suppressed frame (no focused primary window, or a copy-search prompt owns
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
pub(super) fn tmux_webview_pointer(
    mut commands: Commands,
    mut buffer: ResMut<TmuxGestureButtons>,
    mut webview_press: ResMut<WebviewPress>,
    mut gesture: ResMut<TmuxMouseGesture>,
    mut webview_route: WebviewRouteParams,
    mut buttons: MessageReader<MouseButtonInput>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    metrics: Res<TerminalCellMetricsResource>,
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
    if !window.focused || copy_prompt.open.is_some() {
        buttons.clear();
        gesture.state = GestureState::Idle;
        release_webview_press(
            &mut webview_press,
            &webview_route,
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
            let consumed = route_webview_left_click(
                &mut webview_press,
                &mut webview_route,
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

/// Forwards pointer motion over an interactive inline rect of the tmux pane
/// under the cursor to the child's CEF browser via the shared
/// `forward_webview_move`. `CursorMoved`-driven (one forward per frame, latest
/// position), so the one system serves both hover and an in-rect drag. Skipped
/// while a copy-search prompt owns input. `Browsers` is optional so CEF-less
/// tests construct it.
fn forward_tmux_webview_mouse_moves(
    mut cursor_msg: MessageReader<CursorMoved>,
    panes: Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    children: Query<&Children>,
    webviews: Query<(&Webview, Has<NonInteractive>)>,
    overlay_rects: Query<&TerminalOverlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    copy_prompt: Res<CopyPrompt>,
    browsers: Option<NonSend<Browsers>>,
) {
    let Some(moved) = cursor_msg.read().last() else {
        return;
    };
    if copy_prompt.open.is_some() {
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
    forward_webview_move(
        &children,
        &webviews,
        &overlay_rects,
        browsers.as_deref(),
        &mouse_buttons,
        terminal,
        local_phys,
        cell_w,
        cell_h,
        scale,
    );
}
