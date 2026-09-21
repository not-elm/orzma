//! The single CEF pointer sink for inline webviews.

use bevy::ecs::system::SystemParam;
use bevy::input::mouse::MouseButton;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy_cef_core::prelude::Browsers;

/// The sink every inline-webview pointer call is routed through. Each call
/// is dropped while no sink is present.
#[derive(SystemParam)]
pub(in crate::input::mouse) struct CefMouse<'w> {
    sink: Option<NonSend<'w, Browsers>>,
}

impl CefMouse<'_> {
    /// Sets the CEF focus flag on `webview`.
    pub fn set_focus(&self, webview: &Entity, focused: bool) {
        if let Some(sink) = &self.sink {
            sink.set_focus(webview, focused);
        }
    }

    /// Sends a click at `position` in webview-local DIP. `mouse_up` selects
    /// the release phase over the press phase.
    pub fn send_mouse_click(
        &self,
        webview: &Entity,
        position: Vec2,
        button: PointerButton,
        mouse_up: bool,
    ) {
        if let Some(sink) = &self.sink {
            sink.send_mouse_click(webview, position, button, mouse_up);
        }
    }

    /// Sends the raw CEF wheel `delta` at `position` in webview-local DIP.
    pub fn send_mouse_wheel(&self, webview: &Entity, position: Vec2, delta: Vec2) {
        if let Some(sink) = &self.sink {
            sink.send_mouse_wheel(webview, position, delta);
        }
    }

    /// Sends pointer motion to `position` in webview-local DIP, carrying the
    /// held `buttons` so one call serves both hover and an in-rect drag.
    pub fn send_mouse_move<'a>(
        &self,
        webview: &Entity,
        buttons: impl IntoIterator<Item = &'a MouseButton>,
        position: Vec2,
        mouse_leave: bool,
    ) {
        if let Some(sink) = &self.sink {
            sink.send_mouse_move(webview, buttons, position, mouse_leave);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Asserts that no call panics while no sink resource is present.
    ///
    /// Case: the sink resource is absent, as in a headless test app or a
    /// frame before the CEF plugin has inserted its proxy.
    #[test]
    fn calls_without_a_sink_do_not_panic() {
        let mut app = App::new();
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut()
            .run_system_once(move |cef: CefMouse| {
                cef.set_focus(&webview, true);
                cef.send_mouse_click(&webview, Vec2::ZERO, PointerButton::Primary, false);
                cef.send_mouse_wheel(&webview, Vec2::ZERO, Vec2::ZERO);
                cef.send_mouse_move(&webview, [].iter(), Vec2::ZERO, false);
            })
            .expect("a one-shot system with only a CefMouse param runs");
    }
}

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;
    use async_channel::Receiver;
    use bevy::ecs::system::RunSystemOnce;
    use bevy_cef_core::prelude::CefCommand;

    fn app_with_sink() -> (App, Entity, Receiver<CefCommand>) {
        let (tx, rx) = async_channel::unbounded::<CefCommand>();
        let mut app = App::new();
        app.insert_resource(BrowsersProxy::new(tx));
        let webview = app.world_mut().spawn_empty().id();
        (app, webview, rx)
    }

    /// Asserts that a click is handed to the Windows command proxy as a
    /// `SendMouseClick` carrying the entity, position, button, and phase.
    ///
    /// Case: the user presses the left button on a link inside a mounted
    /// webview pane.
    #[test]
    fn a_click_reaches_the_windows_command_proxy() {
        let (mut app, webview, rx) = app_with_sink();
        app.world_mut()
            .run_system_once(move |cef: CefMouse| {
                cef.send_mouse_click(
                    &webview,
                    Vec2::new(12.0, 34.0),
                    PointerButton::Primary,
                    false,
                );
            })
            .expect("a one-shot system with only a CefMouse param runs");
        let command = rx.try_recv().expect("the proxy received one command");
        assert!(
            matches!(
                command,
                CefCommand::SendMouseClick {
                    webview: target,
                    position,
                    button: PointerButton::Primary,
                    mouse_up: false,
                } if target == webview && position == Vec2::new(12.0, 34.0)
            ),
            "the click is forwarded verbatim to the CEF UI thread"
        );
    }

    /// Asserts that pointer motion is handed to the proxy with every held
    /// button, rather than a truncated subset.
    ///
    /// Case: the user drags a selection inside a webview with the left
    /// button down.
    #[test]
    fn a_move_carries_every_held_button() {
        let (mut app, webview, rx) = app_with_sink();
        app.world_mut()
            .run_system_once(move |cef: CefMouse| {
                cef.send_mouse_move(
                    &webview,
                    [MouseButton::Left, MouseButton::Right].iter(),
                    Vec2::new(5.0, 6.0),
                    false,
                );
            })
            .expect("a one-shot system with only a CefMouse param runs");
        let command = rx.try_recv().expect("the proxy received one command");
        assert!(
            matches!(
                command,
                CefCommand::SendMouseMove { webview: target, ref buttons, position, mouse_leave: false }
                    if target == webview
                        && buttons.as_slice() == [MouseButton::Left, MouseButton::Right]
                        && position == Vec2::new(5.0, 6.0)
            ),
            "both held buttons survive the slice repack"
        );
    }

    /// Asserts that a focus request reaches the proxy as `SetFocus`.
    ///
    /// Case: a press lands inside an inline rect, so the page must take
    /// keyboard focus before the click is delivered.
    #[test]
    fn a_focus_request_reaches_the_windows_command_proxy() {
        let (mut app, webview, rx) = app_with_sink();
        app.world_mut()
            .run_system_once(move |cef: CefMouse| {
                cef.set_focus(&webview, true);
            })
            .expect("a one-shot system with only a CefMouse param runs");
        let command = rx.try_recv().expect("the proxy received one command");
        assert!(
            matches!(
                command,
                CefCommand::SetFocus { webview: target, focused: true } if target == webview
            ),
            "the focus request is forwarded to the CEF UI thread"
        );
    }

    /// Asserts that a wheel notch reaches the proxy with its raw delta.
    ///
    /// Case: the user scrolls the wheel over a focused webview showing a
    /// long document.
    #[test]
    fn a_wheel_notch_reaches_the_windows_command_proxy() {
        let (mut app, webview, rx) = app_with_sink();
        app.world_mut()
            .run_system_once(move |cef: CefMouse| {
                cef.send_mouse_wheel(&webview, Vec2::new(7.0, 8.0), Vec2::new(0.0, 120.0));
            })
            .expect("a one-shot system with only a CefMouse param runs");
        let command = rx.try_recv().expect("the proxy received one command");
        assert!(
            matches!(
                command,
                CefCommand::SendMouseWheel { webview: target, position, delta }
                    if target == webview
                        && position == Vec2::new(7.0, 8.0)
                        && delta == Vec2::new(0.0, 120.0)
            ),
            "the raw wheel delta is forwarded unscaled"
        );
    }
}
