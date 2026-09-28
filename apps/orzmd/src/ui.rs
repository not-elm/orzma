//! Native ratatui layer: places the webview over the whole pane.

use ratatui::Frame;
use ratatui_orzma::{FramePlacements, WebviewWidget};

/// Places the webview `instance_id` over the whole frame.
pub(crate) fn draw(frame: &mut Frame<'_>, placements: &mut FramePlacements, instance_id: &str) {
    frame.render_stateful_widget(WebviewWidget::new(instance_id), frame.area(), placements);
}
