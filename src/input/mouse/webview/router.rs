//! Webview pointer routing for the shell surface: forwards left
//! press/release and pointer motion to the inline CEF child under the
//! cursor.

use crate::input::InputPhase;
use crate::input::focus::MouseDisabled;
use crate::input::mouse::MousePhase;
use crate::input::mouse::cell_dims;
use crate::input::mouse::separator::GrabbedSeparator;
use crate::input::mouse::webview::{
    CefMouse, WebviewMoveDeps, WebviewPress, WebviewRouteParams, forward_webview_move_at,
    pressed_terminal, release_webview_press, route_webview_left_click, webview_pointer_frame,
    webview_wheel_delta, webview_wheel_target,
};
use crate::surface::OrzmaTerminal;
use crate::surface::geometry::phys_to_pane_local;
use crate::surface::geometry::topmost_surface_at;
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, ComputedStackIndex, UiGlobalTransform};
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_cef::prelude::FocusedWebview;
use bevy_orzma_tty_renderer::TerminalCellMetricsResource;
use bevy_orzma_tty_renderer::prelude::TerminalOverlays;
use bevy_orzma_webview::{NonInteractive, Webview};

/// Adds the webview pointer-forwarding systems for the shell surface.
pub(super) struct MouseWebviewRouterPlugin;

impl Plugin for MouseWebviewRouterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            route_webview_pointer
                .in_set(MousePhase::Dispatch)
                .run_if(not(any_with_component::<GrabbedSeparator>)),
        )
        .add_systems(
            Update,
            forward_webview_mouse_moves
                .in_set(InputPhase::Hover)
                .run_if(on_message::<CursorMoved>)
                .run_if(not(any_with_component::<GrabbedSeparator>)),
        )
        .add_systems(
            Update,
            forward_webview_wheel
                .in_set(InputPhase::Dispatch)
                .run_if(on_message::<MouseWheel>),
        );
    }
}

/// Every `OrzmaTerminal` surface the router hit-tests, carrying whether the
/// host has suppressed that terminal's mouse input.
type RouterSurfaces<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ComputedNode,
        &'static ComputedStackIndex,
        &'static UiGlobalTransform,
        Has<MouseDisabled>,
    ),
    With<OrzmaTerminal>,
>;

/// What the router found under the pointer.
enum SurfaceUnderCursor {
    /// No terminal surface lies under the pointer.
    None,
    /// The topmost surface has its mouse input suppressed.
    Suppressed,
    /// The topmost surface accepts pointer input, at this pane-local
    /// physical point.
    Open(Entity, Vec2),
}

impl SurfaceUnderCursor {
    /// Resolves the topmost `OrzmaTerminal` under `cursor_phys`.
    ///
    /// A suppressed surface reports `Suppressed` rather than dropping out of
    /// the hit test, so the pointer does not reach a surface below it.
    fn resolve(surfaces: &RouterSurfaces, cursor_phys: Vec2) -> Self {
        let candidates = surfaces
            .iter()
            .map(|(entity, node, stack, transform, _)| (entity, node, stack, transform));
        let Some(terminal) = topmost_surface_at(cursor_phys, candidates) else {
            return Self::None;
        };
        let Ok((_, node, _, transform, suppressed)) = surfaces.get(terminal) else {
            return Self::None;
        };
        if suppressed {
            return Self::Suppressed;
        }
        match phys_to_pane_local(node, transform, cursor_phys) {
            Some(local_phys) => Self::Open(terminal, local_phys),
            None => Self::None,
        }
    }
}

/// Forwards left press/release to the inline CEF child under the cursor
/// on the shell surface. A window-unfocused frame, and a frame on which
/// the terminal owning an in-flight press has its mouse input suppressed,
/// release that press so the focused page is not left logically pressed.
fn route_webview_pointer(
    mut webview_press: ResMut<WebviewPress>,
    mut webview_route: WebviewRouteParams,
    mut buttons: MessageReader<MouseButtonInput>,
    surfaces: RouterSurfaces,
    metrics: Res<TerminalCellMetricsResource>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = windows.single() else {
        buttons.clear();
        webview_press.0 = None;
        return;
    };
    let frame = webview_pointer_frame(window, &metrics);
    if !window.focused {
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
    if let Some(terminal) = pressed_terminal(&webview_press, &webview_route)
        && surfaces
            .get(terminal)
            .is_ok_and(|(_, _, _, _, suppressed)| suppressed)
    {
        release_webview_press(
            &mut webview_press,
            &webview_route,
            frame.cursor_phys,
            frame.cell_w,
            frame.cell_h,
            frame.scale,
        );
    }
    if let Some(cursor_phys) = frame.cursor_phys
        && matches!(
            SurfaceUnderCursor::resolve(&surfaces, cursor_phys),
            SurfaceUnderCursor::Suppressed
        )
    {
        buttons.clear();
        return;
    }
    for ev in buttons.read() {
        if ev.button != MouseButton::Left {
            continue;
        }
        let Some(cursor_phys) = frame.cursor_phys else {
            continue;
        };
        let SurfaceUnderCursor::Open(terminal, local_phys) =
            SurfaceUnderCursor::resolve(&surfaces, cursor_phys)
        else {
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

/// Forwards pointer motion over an interactive inline rect of the shell surface
/// to the child's CEF browser via the shared `forward_webview_move_at`.
fn forward_webview_mouse_moves(
    mut cursor_msg: MessageReader<CursorMoved>,
    surfaces: RouterSurfaces,
    children: Query<'_, '_, &'static Children>,
    webviews: Query<'_, '_, (&'static Webview, Has<NonInteractive>)>,
    overlay_rects: Query<'_, '_, &'static TerminalOverlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    cef: CefMouse,
) {
    let Some(moved) = cursor_msg.read().last() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let frame = webview_pointer_frame(window, &metrics);
    let cursor_phys = moved.position * frame.scale;
    let deps = WebviewMoveDeps {
        children: &children,
        webviews: &webviews,
        overlay_rects: &overlay_rects,
        cef: &cef,
        pressed_buttons: &mouse_buttons,
    };
    forward_webview_move_at(
        &deps,
        |c| match SurfaceUnderCursor::resolve(&surfaces, c) {
            SurfaceUnderCursor::Open(terminal, local_phys) => Some((terminal, local_phys)),
            SurfaceUnderCursor::None | SurfaceUnderCursor::Suppressed => None,
        },
        cursor_phys,
        &frame,
    );
}

/// Forwards the mouse wheel to the FOCUSED inline webview under the
/// cursor on the shell surface (raw CEF wheel, focus-gated). When no
/// focused webview is under the pointer the reader is drained and the
/// wheel cedes to `crate::input::mouse::wheel::dispatch_mouse_wheel`
/// (the terminal's wheel routing: mouse reports, alternate-scroll cursor
/// keys, or the scrollback) through its own reader; over the rect the
/// shell is `MouseClaimedByWebview`, so that dispatcher yields and only
/// the page scrolls.
fn forward_webview_wheel(
    mut wheel: MessageReader<MouseWheel>,
    focused_webview: Res<FocusedWebview>,
    webview_parents: Query<&ChildOf, With<Webview>>,
    surfaces: RouterSurfaces,
    children: Query<&Children>,
    webviews: Query<(&Webview, Has<NonInteractive>)>,
    overlay_rects: Query<&TerminalOverlays>,
    windows: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<TerminalCellMetricsResource>,
    cef: CefMouse,
) {
    let Ok(window) = windows.single() else {
        wheel.clear();
        return;
    };
    if !window.focused {
        wheel.clear();
        return;
    }
    let scale = window.scale_factor();
    let (cell_w, cell_h) = cell_dims(&metrics);
    let target = window.cursor_position().and_then(|c| {
        let cursor_phys = c * scale;
        let SurfaceUnderCursor::Open(terminal, local_phys) =
            SurfaceUnderCursor::resolve(&surfaces, cursor_phys)
        else {
            return None;
        };
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
    for ev in wheel.read() {
        cef.send_mouse_wheel(&child, dip, webview_wheel_delta(ev.unit, ev.x, ev.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::math::{DVec2, IVec4};
    use bevy::window::WindowResolution;
    use bevy_cef::prelude::FocusedWebview;
    use bevy_orzma_tty_renderer::CellMetrics;
    use orzma_vt::prelude::InstanceId;

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

    /// The shell surface at window center (400,300), size 800x600 → top-left (0,0),
    /// with one interactive inline rect rows 2..12, cols 3..43 (phys y 32..192,
    /// x 24..344 at the 8x16 px cell pitch). Returns `(app, shell, child)`.
    fn make_webview_app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.init_resource::<WebviewPress>();
        app.init_resource::<FocusedWebview>();
        app.insert_resource(test_metrics());
        app.add_systems(Update, route_webview_pointer);

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
                    handle: "webview".into(),
                    instance: InstanceId(1),
                    slot: 0,
                    rows: 10,
                    cols: 40,
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
        let (mut app, _shell, child) = make_webview_app();
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
        let (mut app, _shell, child) = make_webview_app();
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
        let (mut app, _shell, child) = make_webview_app();
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

    /// Asserts that a press inside an interactive inline rect hands CEF a
    /// focus request followed by a press-phase click, in that order.
    ///
    /// Case: the user clicks a link on a page mounted into a pane, on a
    /// platform where CEF is driven through its own UI thread.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_press_over_an_inline_rect_reaches_cef_focus_then_click() {
        use bevy_cef_core::prelude::{BrowsersProxy, CefCommand};

        let (mut app, _shell, child) = make_webview_app();
        let (tx, rx) = async_channel::unbounded::<CefCommand>();
        app.insert_resource(BrowsersProxy::new(tx));
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();

        let focus = rx.try_recv().expect("the press issued a focus request");
        assert!(
            matches!(
                focus,
                CefCommand::SetFocus { webview, focused: true } if webview == child
            ),
            "the focus request precedes the click so the first click is not swallowed"
        );
        let click = rx.try_recv().expect("the press issued a click");
        assert!(
            matches!(
                click,
                CefCommand::SendMouseClick {
                    webview,
                    button: PointerButton::Primary,
                    mouse_up: false,
                    ..
                } if webview == child
            ),
            "the press phase of the click reaches the focused child"
        );
    }

    /// Asserts that a press over an interactive rect on a suppressed
    /// terminal reaches neither the webview focus nor the press marker.
    ///
    /// Case: the user enters vi mode and then clicks a link on a page
    /// mounted in that pane.
    #[test]
    fn a_press_on_a_suppressed_terminal_does_not_reach_the_webview() {
        let (mut app, shell, _child) = make_webview_app();
        app.world_mut().entity_mut(shell).insert(MouseDisabled);
        set_cursor(&mut app, Vec2::new(40.0, 48.0));
        write_left(&mut app, ButtonState::Pressed);
        app.update();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "a suppressed terminal must not hand its press to the inline webview"
        );
        assert_eq!(
            app.world().resource::<WebviewPress>().0,
            None,
            "no in-flight press is recorded for a suppressed terminal"
        );
    }

    /// Asserts that suppression of the terminal owning an in-flight press
    /// releases that press, with no cursor anywhere on screen.
    ///
    /// Case: the user presses inside a page, drags the pointer off the
    /// window, and vi mode starts before the button comes back up.
    #[test]
    fn suppression_of_the_pressed_terminal_releases_the_press() {
        let (mut app, shell, child) = make_webview_app();
        app.world_mut().resource_mut::<WebviewPress>().0 = Some(child);
        app.world_mut().entity_mut(shell).insert(MouseDisabled);
        let win = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Window>(win)
            .unwrap()
            .set_physical_cursor_position(None);
        app.update();
        assert_eq!(
            app.world().resource::<WebviewPress>().0,
            None,
            "the press is released by its own terminal's suppression, not by what lies under the pointer"
        );
    }
}
