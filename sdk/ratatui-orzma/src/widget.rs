//! The ratatui StatefulWidget that records placements.

use crate::session::FramePlacements;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, StatefulWidget, Widget};

/// A ratatui widget that mounts an orzma webview at its area.
///
/// Blanks its cells (the webview composites under the text) and records its rect
/// into the frame the [`crate::OrzmaBackend`] emits on the next draw. Optionally
/// paints a fallback under-layer (shown on non-macOS or before the page composites).
pub struct WebviewWidget<W = WebviewDefaultPlaceholder> {
    handle: String,
    fallback: W,
    focused: bool,
    on_compositing_change: Option<Box<dyn Fn(bool) + 'static>>,
}

impl WebviewWidget<WebviewDefaultPlaceholder> {
    /// Creates a widget for the given webview handle id.
    pub fn new(handle: impl Into<String>) -> Self {
        Self {
            handle: handle.into(),
            fallback: WebviewDefaultPlaceholder,
            focused: false,
            on_compositing_change: None,
        }
    }
}

impl<W> WebviewWidget<W> {
    /// Sets a fallback widget painted into the cells under the webview.
    pub fn fallback<W2: Widget>(self, widget: W2) -> WebviewWidget<W2> {
        WebviewWidget {
            handle: self.handle,
            fallback: widget,
            focused: self.focused,
            on_compositing_change: self.on_compositing_change,
        }
    }

    /// Registers a callback invoked during [`StatefulWidget::render`] when a
    /// compositing-state change for this handle is pending in the frame state.
    ///
    /// The callback receives `true` when the webview starts compositing (the
    /// page is live and painting) and `false` when it stops.
    pub fn on_compositing_change(mut self, f: impl Fn(bool) + 'static) -> Self {
        self.on_compositing_change = Some(Box::new(f));
        self
    }

    /// Marks the widget focused, a hint for drawing a focus frame/title around
    /// the webview (the page content itself is composited by the host).
    ///
    /// Focusing a webview on the same frame it is first mounted may race the
    /// mount on the host (the focus op travels the control socket while the
    /// mount OSC travels the terminal output), so the focus op can be silently
    /// dropped; focus a webview on a frame after its first mount, or re-assert
    /// focus if needed.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether this widget is currently focused.
    pub fn is_focused(&self) -> bool {
        self.focused
    }
}

impl<W: Widget> StatefulWidget for WebviewWidget<W> {
    type State = FramePlacements;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        Clear.render(area, buf);
        self.fallback.render(area, buf);
        state.record(self.handle.clone(), area);
        if self.focused {
            state.set_focused(self.handle.clone());
        }
        if let Some(active) = state.take_compositing(&self.handle)
            && let Some(cb) = &self.on_compositing_change
        {
            cb(active);
        }
    }
}

/// A no-op fallback widget (the default): renders nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct WebviewDefaultPlaceholder;

impl Widget for WebviewDefaultPlaceholder {
    fn render(self, _area: Rect, _buf: &mut Buffer) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::FramePlacements;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::text::Text;
    use ratatui::widgets::StatefulWidget;

    #[test]
    fn records_placement_and_blanks_cells() {
        let area = Rect {
            x: 1,
            y: 1,
            width: 6,
            height: 2,
        };
        let mut buf = Buffer::filled(Rect::new(0, 0, 10, 5), ratatui::buffer::Cell::new("Z"));
        let mut state = FramePlacements::default();

        WebviewWidget::new("view-x").render(area, &mut buf, &mut state);

        assert_eq!(state.placements_for_test().len(), 1);
        assert_eq!(state.placements_for_test()[0].handle, "view-x");
        assert_eq!(buf[(1, 1)].symbol(), " ");
    }

    #[test]
    fn fallback_is_painted() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 5,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();

        WebviewWidget::new("v")
            .fallback(Text::raw("hi"))
            .render(area, &mut buf, &mut state);

        assert_eq!(buf[(0, 0)].symbol(), "h");
    }

    #[test]
    fn focused_widget_constructs() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();
        WebviewWidget::new("v")
            .focused(true)
            .render(area, &mut buf, &mut state);
        assert_eq!(state.placements_for_test().len(), 1);
    }

    #[test]
    fn focused_render_records_focused_handle() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();
        WebviewWidget::new("v")
            .focused(true)
            .render(area, &mut buf, &mut state);
        assert_eq!(state.focused_for_test(), Some("v"));
    }

    #[test]
    fn unfocused_render_records_no_focus() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();
        WebviewWidget::new("v").render(area, &mut buf, &mut state);
        assert_eq!(state.focused_for_test(), None);
    }

    #[test]
    fn on_compositing_change_fires_when_pending() {
        use std::cell::Cell;
        use std::rc::Rc;
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();
        state.pending_compositing.insert("v".into(), true);

        let fired = Rc::new(Cell::new(false));
        let fired_val = Rc::new(Cell::new(false));
        let fired2 = fired.clone();
        let fired_val2 = fired_val.clone();
        WebviewWidget::new("v")
            .on_compositing_change(move |active| {
                fired2.set(true);
                fired_val2.set(active);
            })
            .render(area, &mut buf, &mut state);

        assert!(fired.get(), "callback should have fired");
        assert!(fired_val.get(), "active should be true");
    }

    #[test]
    fn on_compositing_change_not_fired_when_absent() {
        use std::rc::Rc;
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();

        let fired = Rc::new(std::cell::Cell::new(false));
        let fired2 = fired.clone();
        WebviewWidget::new("v")
            .on_compositing_change(move |_| fired2.set(true))
            .render(area, &mut buf, &mut state);

        assert!(
            !fired.get(),
            "callback must not fire when no pending compositing"
        );
    }

    #[test]
    fn on_compositing_change_fires_false() {
        use std::cell::Cell;
        use std::rc::Rc;
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();
        state.pending_compositing.insert("v".into(), false);

        let fired_val = Rc::new(Cell::new(true));
        let fired_val2 = fired_val.clone();
        WebviewWidget::new("v")
            .on_compositing_change(move |active| {
                fired_val2.set(active);
            })
            .render(area, &mut buf, &mut state);

        assert!(!fired_val.get(), "active should be false");
    }

    #[test]
    fn on_compositing_change_consumed_from_state() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();
        state.pending_compositing.insert("v".into(), true);

        WebviewWidget::new("v")
            .on_compositing_change(move |_| {})
            .render(area, &mut buf, &mut state);

        assert!(
            state.pending_compositing_for_test().is_empty(),
            "pending entry should be consumed after render"
        );
    }

    #[test]
    fn on_compositing_change_survives_fallback_builder() {
        use ratatui::text::Text;
        use std::cell::Cell;
        use std::rc::Rc;
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();
        state.pending_compositing.insert("v".into(), true);

        let fired = Rc::new(Cell::new(false));
        let fired2 = fired.clone();
        WebviewWidget::new("v")
            .on_compositing_change(move |_| fired2.set(true))
            .fallback(Text::raw("x"))
            .render(area, &mut buf, &mut state);

        assert!(
            fired.get(),
            "callback should survive .fallback() builder call"
        );
    }
}
