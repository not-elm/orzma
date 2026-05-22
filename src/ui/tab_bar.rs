//! Egui tab bar rendering for panes: `draw_pane_tab_bar` renders one tab per
//! `Activity` in a row. `tab_colors` encapsulates the active/inactive color
//! logic.

use bevy_egui::egui;
use ozmux_multiplexer::{Activity, Pane};

/// Color set computed from a tab's active state and its pane's active state.
pub(crate) struct TabColors {
    /// Background color of the tab.
    pub bg: egui::Color32,
    /// Top indicator color (bright when both tab and pane are active).
    pub indicator: egui::Color32,
    /// Text color of the tab label.
    pub text: egui::Color32,
}

/// Compute the (background, top-indicator, text) color triple for a single tab.
/// The indicator is bright `ACCENT` only when both the tab and its pane are
/// active; an active tab inside an inactive pane gets the muted `BORDER`.
pub(crate) fn tab_colors(is_active: bool, is_active_pane: bool) -> TabColors {
    let p = crate::ui::egui_theme::palette();
    let bg = if is_active {
        p.tab_active_bg
    } else {
        egui::Color32::TRANSPARENT
    };
    let indicator = match (is_active, is_active_pane) {
        (true, true) => p.accent,
        (true, false) => p.border,
        (false, _) => egui::Color32::TRANSPARENT,
    };
    let text = if is_active { p.foreground } else { p.muted };
    TabColors {
        bg,
        indicator,
        text,
    }
}

/// Egui draw of the per-pane tab bar (one tab per Activity, laid out left-to-right).
pub(crate) fn draw_pane_tab_bar(ui: &mut egui::Ui, pane: &Pane, is_active_pane: bool) {
    egui::Frame::default()
        .fill(crate::ui::egui_theme::palette().tab_bar_bg)
        .inner_margin(egui::Margin::same(0))
        .show(ui, |ui| {
            // NOTE: `expand_to_include_rect(ui.max_rect())` is required —
            // egui::Frame sizes its outer_rect from `content_ui.min_rect()`,
            // and `ui.horizontal` only advances the parent by its child's
            // min_rect (not the child's max_rect, see egui Ui::scope_dyn).
            // Without this expansion the bar collapses to the tabs' natural
            // width, leaving the rest of the pane unfilled.
            ui.expand_to_include_rect(ui.max_rect());
            ui.horizontal(|ui| {
                for activity in &pane.activities {
                    let is_active = activity.id == pane.active_activity;
                    draw_tab(ui, activity, is_active, is_active_pane);
                }
            });
        });
}

fn draw_tab(
    ui: &mut egui::Ui,
    activity: &Activity,
    is_active: bool,
    is_active_pane: bool,
) -> egui::Response {
    let colors = tab_colors(is_active, is_active_pane);

    let inner = egui::Frame::NONE
        .fill(colors.bg)
        .corner_radius(egui::CornerRadius {
            nw: crate::theme::TAB_BORDER_RADIUS_PX as u8,
            ne: crate::theme::TAB_BORDER_RADIUS_PX as u8,
            sw: 0,
            se: 0,
        })
        .inner_margin(egui::Margin::symmetric(
            crate::theme::TAB_PADDING_X_PX as i8,
            4,
        ))
        .show(ui, |ui| {
            ui.colored_label(colors.text, &activity.name);
        });

    if colors.indicator != egui::Color32::TRANSPARENT {
        let rect = inner.response.rect;
        ui.painter().line_segment(
            [rect.left_top(), rect.right_top()],
            egui::Stroke::new(crate::theme::TAB_INDICATOR_PX, colors.indicator),
        );
    }

    inner.response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_colors_active_in_active_pane_uses_accent_indicator() {
        let c = tab_colors(true, true);
        assert_eq!(c.bg, crate::ui::egui_theme::palette().tab_active_bg);
        assert_eq!(c.indicator, crate::ui::egui_theme::palette().accent);
        assert_eq!(c.text, crate::ui::egui_theme::palette().foreground);
    }

    #[test]
    fn tab_colors_active_in_inactive_pane_uses_border_indicator() {
        let c = tab_colors(true, false);
        assert_eq!(c.bg, crate::ui::egui_theme::palette().tab_active_bg);
        assert_eq!(c.indicator, crate::ui::egui_theme::palette().border);
    }

    #[test]
    fn tab_colors_inactive_is_fully_transparent() {
        let c = tab_colors(false, true);
        assert_eq!(c.bg, egui::Color32::TRANSPARENT);
        assert_eq!(c.indicator, egui::Color32::TRANSPARENT);
        assert_eq!(c.text, crate::ui::egui_theme::palette().muted);
    }
}
