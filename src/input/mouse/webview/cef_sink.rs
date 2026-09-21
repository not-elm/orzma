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
    /// frame before the CEF plugin has inserted `Browsers`.
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
