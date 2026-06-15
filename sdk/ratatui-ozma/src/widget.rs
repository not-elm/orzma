//! The ratatui StatefulWidget that records placements.

use crate::session::FramePlacements;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, StatefulWidget, Widget};

/// A ratatui widget that mounts an ozmux webview at its area.
///
/// Blanks its cells (the webview composites under the text) and records its rect
/// into the frame the [`crate::OzmaBackend`] emits on the next draw. Optionally
/// paints a fallback under-layer (shown on non-macOS or before the page composites).
pub struct WebviewWidget<'a, W = WebviewDefaultPlaceholder> {
    handle: &'a str,
    fallback: W,
    focused: bool,
}

impl<'a> WebviewWidget<'a, WebviewDefaultPlaceholder> {
    /// Creates a widget for the given webview handle id.
    pub fn new(handle: &'a str) -> Self {
        Self {
            handle,
            fallback: WebviewDefaultPlaceholder,
            focused: false,
        }
    }
}

impl<'a, W> WebviewWidget<'a, W> {
    /// Sets a fallback widget painted into the cells under the webview.
    pub fn fallback<W2: Widget>(self, widget: W2) -> WebviewWidget<'a, W2> {
        WebviewWidget {
            handle: self.handle,
            fallback: widget,
            focused: self.focused,
        }
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

impl<W: Widget> StatefulWidget for WebviewWidget<'_, W> {
    type State = FramePlacements;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        Clear.render(area, buf);
        self.fallback.render(area, buf);
        state.record(self.handle.to_owned(), area);
        if self.focused {
            state.set_focused(self.handle.to_owned());
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
}
