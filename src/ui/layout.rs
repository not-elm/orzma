//! Egui layout helpers: `draw_cell_recursive` descends the `LayoutCellState`
//! tree of an `ozmux_multiplexer::Window` and renders each cell in egui's
//! immediate-mode API. `split_horizontal` / `split_vertical` divide a `Rect`
//! proportionally for split panes.

use crate::{theme::PANE_BORDER_PX, ui::egui_theme::palette};
use bevy_egui::egui;
use ozmux_multiplexer::{Cell, CellId, SplitOrientation, Window};

/// Egui draw of one Cell (recurses through Split into two `scope_builder`
/// child regions, leaves at Pane). The recursion shape mirrors Phase 2
/// `build_cell` but uses egui's `scope_builder` API instead of bevy_ui Nodes.
pub(crate) fn draw_cell_recursive(ui: &mut egui::Ui, window: &Window, cell_id: &CellId) {
    let Ok(cell) = window.cells.cell(cell_id) else {
        tracing::warn!(
            target: "ozmux_gui::layout",
            "cell {} missing from window {}",
            cell_id,
            window.id,
        );
        return;
    };

    match cell {
        Cell::Root(_) => {
            tracing::warn!(
                target: "ozmux_gui::layout",
                "unexpected nested Cell::Root at {}",
                cell_id,
            );
        }
        Cell::Pane(p) => draw_pane(ui, window, p),
        Cell::Split(s) => {
            let avail = ui.available_rect_before_wrap();
            let lhs_frac =
                ozmux_multiplexer::LayoutCellState::split_ratio(s.lhs_weight, s.rhs_weight);
            let (lhs_rect, rhs_rect) = match s.orientation {
                SplitOrientation::Horizontal => split_horizontal(avail, lhs_frac),
                SplitOrientation::Vertical => split_vertical(avail, lhs_frac),
            };

            ui.scope_builder(egui::UiBuilder::new().max_rect(lhs_rect), |ui| {
                draw_cell_recursive(ui, window, &s.lhs_cell);
            });
            ui.scope_builder(egui::UiBuilder::new().max_rect(rhs_rect), |ui| {
                draw_cell_recursive(ui, window, &s.rhs_cell);
            });
        }
    }
}

/// Split a `Rect` along the x-axis. `lhs_frac` ∈ [0.0, 1.0] is the
/// fraction of the rect's width allocated to the left half.
fn split_horizontal(rect: egui::Rect, lhs_frac: f32) -> (egui::Rect, egui::Rect) {
    let split_x = rect.left() + rect.width() * lhs_frac;
    (
        egui::Rect::from_min_max(rect.left_top(), egui::pos2(split_x, rect.bottom())),
        egui::Rect::from_min_max(egui::pos2(split_x, rect.top()), rect.right_bottom()),
    )
}

/// Split a `Rect` along the y-axis. `top_frac` ∈ [0.0, 1.0] is the
/// fraction of the rect's height allocated to the top half.
fn split_vertical(rect: egui::Rect, top_frac: f32) -> (egui::Rect, egui::Rect) {
    let split_y = rect.top() + rect.height() * top_frac;
    (
        egui::Rect::from_min_max(rect.left_top(), egui::pos2(rect.right(), split_y)),
        egui::Rect::from_min_max(egui::pos2(rect.left(), split_y), rect.right_bottom()),
    )
}

/// Egui draw of one `Cell::Pane` — pane chrome (border) + tab bar + active
/// activity placeholder.
fn draw_pane(ui: &mut egui::Ui, window: &Window, pane_cell: &ozmux_multiplexer::PaneCell) {
    let Some(pane) = window.panes.get(&pane_cell.pane) else {
        tracing::warn!(
            target: "ozmux_gui::layout",
            "pane {} referenced by cell missing",
            pane_cell.pane,
        );
        return;
    };
    let is_active_pane = pane_cell.pane == window.active_pane;

    egui::Frame::default()
        .stroke(egui::Stroke::new(PANE_BORDER_PX, palette().border))
        .show(ui, |ui| {
            ui.expand_to_include_rect(ui.max_rect());
            ui.vertical(|ui| {
                crate::ui::tab_bar::draw_pane_tab_bar(ui, pane, is_active_pane);
                if let Some(active) = pane
                    .activities
                    .iter()
                    .find(|a| a.id == pane.active_activity)
                {
                    crate::ui::activity::draw_activity_placeholder(ui, active);
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_horizontal_at_half_divides_evenly() {
        let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0));
        let (lhs, rhs) = split_horizontal(rect, 0.5);
        assert_eq!(lhs.min, egui::pos2(0.0, 0.0));
        assert_eq!(lhs.max, egui::pos2(50.0, 50.0));
        assert_eq!(rhs.min, egui::pos2(50.0, 0.0));
        assert_eq!(rhs.max, egui::pos2(100.0, 50.0));
    }

    #[test]
    fn split_vertical_at_third_divides_proportionally() {
        let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 90.0));
        let (top, bottom) = split_vertical(rect, 1.0 / 3.0);
        assert_eq!(top.min, egui::pos2(0.0, 0.0));
        assert_eq!(top.max.y, 30.0);
        assert_eq!(bottom.min.y, 30.0);
        assert_eq!(bottom.max, egui::pos2(100.0, 90.0));
    }
}
