//! Default-mode (`AppMode::Default`) webview pointer routing: forwards left
//! press/release and pointer motion to the inline CEF child under the cursor on
//! the single Default shell surface, via the mode-agnostic core in
//! `crate::input::mouse::webview`. The tmux equivalent is `crate::input::mouse::button::tmux`;
//! this is the single-surface analogue (no pane arbitration / gesture hand-off).
//!
//! The pointer system runs EVERY frame in `AppMode::Default` (not message-gated)
//! so an in-flight press is released when input is suppressed (window unfocused /
//! modal), never leaving CEF logically pressed. Double-handling with the
//! terminal's `dispatch_mouse_buttons` is avoided by the `MouseDisabled`
//! rect-claim gate in `crate::input::default_mode::maintain_input_gates`: over an
//! interactive rect the shell is `MouseDisabled` (dispatch yields, the webview
//! gets the click); off-rect the press clears webview focus here and falls
//! through to the terminal.

use crate::app_mode::AppMode;
use crate::input::InputPhase;
use crate::input::mouse::cell_dims;
use crate::input::mouse::webview::{
    WebviewMoveDeps, WebviewPress, WebviewRouteParams, forward_webview_move_at,
    release_webview_press, route_webview_left_click, webview_pointer_frame, webview_wheel_delta,
    webview_wheel_target,
};
use crate::surface::OrzmaTerminal;
use crate::surface::geometry::phys_to_pane_local;
use crate::surface::geometry::topmost_surface_at;
use crate::ui::copy_search::CopyPrompt;
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_cef::prelude::FocusedWebview;
use bevy_cef_core::prelude::Browsers;
use orzma_tty_renderer::TerminalCellMetricsResource;
use orzma_tty_renderer::prelude::TerminalOverlays;
use orzma_webview::{NonInteractive, Webview};

/// Registers the Default-mode webview pointer systems. The shared
/// `WebviewPress` resource is owned by the parent `MouseWebviewPlugin`.
pub(super) struct MouseWebviewDefaultModePlugin;

impl Plugin for MouseWebviewDefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            default_webview_pointer
                .in_set(InputPhase::Dispatch)
                .run_if(in_state(AppMode::Default)),
        )
        .add_systems(
            Update,
            forward_default_webview_mouse_moves
                .in_set(InputPhase::Hover)
                .run_if(in_state(AppMode::Default).and(on_message::<CursorMoved>)),
        )
        .add_systems(
            Update,
            forward_default_webview_wheel
                .in_set(InputPhase::Dispatch)
                .run_if(in_state(AppMode::Default).and(on_message::<MouseWheel>)),
        );
    }
}

/// Forwards left press/release to the inline CEF child under the cursor on the
/// Default shell. Runs every frame in `AppMode::Default`: a suppressed frame
/// (window unfocused / copy-search prompt) drains the reader and releases an
/// in-flight press so the focused page is not left logically pressed.
fn default_webview_pointer(
    mut webview_press: ResMut<WebviewPress>,
    mut webview_route: WebviewRouteParams,
    mut buttons: MessageReader<MouseButtonInput>,
    surfaces: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<OrzmaTerminal>>,
    metrics: Res<TerminalCellMetricsResource>,
    copy_prompt: Res<CopyPrompt>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        buttons.clear();
        webview_press.0 = None;
        return;
    };
    let frame = webview_pointer_frame(window, &metrics);
    if !window.focused || copy_prompt.open.is_some() {
        buttons.clear();
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
        let Some(cursor_phys) = frame.cursor_phys else {
            continue;
        };
        let Some(terminal) = topmost_surface_at(cursor_phys, surfaces.iter()) else {
            continue;
        };
        let Ok((_, node, transform)) = surfaces.get(terminal) else {
            continue;
        };
        let Some(local_phys) = phys_to_pane_local(node, transform, cursor_phys) else {
            continue;
        };
        route_webview_left_click(
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
    }
}

/// Forwards pointer motion over an interactive inline rect of the Default shell
/// to the child's CEF browser via the shared `forward_webview_move_at`. Skipped
/// while a copy-search prompt owns input.
fn forward_default_webview_mouse_moves(
    mut cursor_msg: MessageReader<CursorMoved>,
    surfaces: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<OrzmaTerminal>>,
    children: Query<'_, '_, &'static Children>,
    webviews: Query<'_, '_, (&'static Webview, Has<NonInteractive>)>,
    overlay_rects: Query<'_, '_, &'static TerminalOverlays>,
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
            let t = topmost_surface_at(c, surfaces.iter())?;
            let (_, node, transform) = surfaces.get(t).ok()?;
            Some((t, phys_to_pane_local(node, transform, c)?))
        },
        cursor_phys,
        &frame,
    );
}

/// Forwards the mouse wheel to the FOCUSED inline webview under the cursor on the
/// Default shell (raw CEF wheel, focus-gated). When no focused webview is under
/// the pointer the reader is drained and the wheel cedes to
/// `crate::input::mouse::wheel::dispatch_mouse_wheel` (terminal scrollback) through its own
/// reader; over the rect the shell is `MouseDisabled` (rect-claim gate), so that
/// dispatcher yields and only the page scrolls. Gated to wheel frames.
fn forward_default_webview_wheel(
    mut wheel: MessageReader<MouseWheel>,
    focused_webview: Res<FocusedWebview>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    surfaces: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<OrzmaTerminal>>,
    children: Query<&Children>,
    webviews: Query<(&Webview, Has<NonInteractive>)>,
    overlay_rects: Query<&TerminalOverlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    copy_prompt: Res<CopyPrompt>,
    browsers: Option<NonSend<Browsers>>,
) {
    let Ok(window) = windows.single() else {
        wheel.clear();
        return;
    };
    if !window.focused || copy_prompt.open.is_some() {
        wheel.clear();
        return;
    }
    let scale = window.scale_factor();
    let (cell_w, cell_h) = cell_dims(&metrics);
    let target = window.cursor_position().and_then(|c| {
        let cursor_phys = c * scale;
        let terminal = topmost_surface_at(cursor_phys, surfaces.iter())?;
        let (_, node, transform) = surfaces.get(terminal).ok()?;
        let local_phys = phys_to_pane_local(node, transform, cursor_phys)?;
        webview_wheel_target(
            &focused_webview,
            &webview_parents,
            &children,
            &webviews,
            &overlay_rects,
            terminal,
            local_phys,
            cell_w,
            cell_h,
            scale,
        )
    });
    let Some((child, dip)) = target else {
        wheel.clear();
        return;
    };
    let Some(browsers) = browsers.as_deref() else {
        wheel.clear();
        return;
    };
    for ev in wheel.read() {
        browsers.send_mouse_wheel(&child, dip, webview_wheel_delta(ev.unit, ev.x, ev.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::math::{DVec2, IVec4};
    use bevy::window::WindowResolution;
    use bevy_cef::prelude::FocusedWebview;
    use orzma_tty_renderer::CellMetrics;

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

    /// Default shell at window center (400,300), size 800x600 → top-left (0,0),
    /// with one interactive inline rect rows 2..12, cols 3..43 (phys y 32..192,
    /// x 24..344 at the 8x16 px cell pitch). Returns `(app, shell, child)`.
    fn make_default_webview_app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.init_resource::<WebviewPress>();
        app.init_resource::<CopyPrompt>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(test_metrics());
        app.add_systems(Update, default_webview_pointer);

        let mut overlays = TerminalOverlays::default();
        overlays.rects[0] = IVec4::new(2, 3, 10, 40);
        let shell = app
            .world_mut()
            .spawn((
                OrzmaTerminal,
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
                ChildOf(shell),
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
        (app, shell, child)
    }

    fn set_cursor(app: &mut App, phys: Vec2) {
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

    fn write_left(app: &mut App, state: ButtonState) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: MouseButton::Left,
                state,
                window: Entity::PLACEHOLDER,
            });
    }

    #[test]
    fn default_press_over_inline_rect_focuses_child() {
        let (mut app, _shell, child) = make_default_webview_app();
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "a press inside the inline rect focuses the CEF child (so the link receives the click)"
        );
        assert_eq!(
            app.world().resource::<WebviewPress>().0,
            Some(child),
            "the press is recorded so the matching release routes to the same child"
        );
    }

    #[test]
    fn default_off_rect_press_clears_focus_and_records_no_press() {
        let (mut app, _shell, child) = make_default_webview_app();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);
        set_cursor(&mut app, Vec2::new(400.0, 400.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "an off-rect press clears inline webview focus so the click falls through to the terminal"
        );
        assert_eq!(
            app.world().resource::<WebviewPress>().0,
            None,
            "an off-rect press records no in-flight webview press"
        );
    }

    #[test]
    fn default_suppressed_frame_releases_in_flight_press() {
        let (mut app, _shell, child) = make_default_webview_app();
        app.world_mut().resource_mut::<WebviewPress>().0 = Some(child);
        let win = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut().get_mut::<Window>(win).unwrap().focused = false;
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        app.update();
        assert_eq!(
            app.world().resource::<WebviewPress>().0,
            None,
            "a window-exists-but-suppressed frame releases the in-flight inline press so CEF is not left pressed"
        );
    }
}
