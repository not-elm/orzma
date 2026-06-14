//! The ratatui StatefulWidget that records placements.

use crate::session::FramePlacements;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear, StatefulWidget, Widget};

/// A ratatui widget that mounts an ozmux webview at its area.
///
/// Blanks its cells (the webview composites under the text) and records its rect
/// for the next [`crate::Ozma::flush`]. Optionally paints a fallback under-layer
/// (shown on non-macOS or before the page composites).
pub struct WebviewWidget<'a, W = Blank> {
    handle: &'a str,
    fallback: W,
}

impl<'a> WebviewWidget<'a, Blank> {
    /// Creates a widget for the given webview handle id.
    pub fn new(handle: &'a str) -> Self {
        Self { handle, fallback: Blank }
    }
}

impl<'a, W> WebviewWidget<'a, W> {
    /// Sets a fallback widget painted into the cells under the webview.
    pub fn fallback<W2: Widget>(self, widget: W2) -> WebviewWidget<'a, W2> {
        WebviewWidget {
            handle: self.handle,
            fallback: widget,
        }
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
    }
}

/// A no-op fallback widget (the default): renders nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct Blank;

impl Widget for Blank {
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
        let area = Rect { x: 1, y: 1, width: 6, height: 2 };
        let mut buf = Buffer::filled(Rect::new(0, 0, 10, 5), ratatui::buffer::Cell::new("Z"));
        let mut state = FramePlacements::default();

        WebviewWidget::new("view-x").render(area, &mut buf, &mut state);

        assert_eq!(state.placements_for_test().len(), 1);
        assert_eq!(state.placements_for_test()[0].handle, "view-x");
        assert_eq!(buf[(1, 1)].symbol(), " ");
    }

    #[test]
    fn fallback_is_painted() {
        let area = Rect { x: 0, y: 0, width: 5, height: 1 };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();

        WebviewWidget::new("v").fallback(Text::raw("hi")).render(area, &mut buf, &mut state);

        assert_eq!(buf[(0, 0)].symbol(), "h");
    }
}
