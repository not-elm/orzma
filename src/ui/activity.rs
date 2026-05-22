//! Activity placeholder rendering for egui: `draw_activity_placeholder` renders
//! a colored frame + label for each `Activity` kind. Phase 3+ replaces the
//! `ActivityKind::Terminal` branch with an `EguiBevyPaintCallback` for the
//! terminal grid renderer.

use bevy_egui::egui;
use ozmux_multiplexer::{ActivityId, ActivityKind};

/// Egui background color for the `Activity` placeholder, chosen by kind.
pub(crate) fn kind_color(kind: &ActivityKind) -> egui::Color32 {
    match kind {
        ActivityKind::Terminal => crate::ui::egui_theme::palette().activity_terminal,
        ActivityKind::Browser { .. } => crate::ui::egui_theme::palette().activity_browser,
        ActivityKind::Extension { .. } => crate::ui::egui_theme::palette().activity_extension,
    }
}

/// Egui immediate-mode draw of the activity placeholder. Phase 3+ replaces
/// the body of the `ActivityKind::Terminal` arm with `EguiBevyPaintCallback`
/// for the terminal grid renderer.
pub(crate) fn draw_activity_placeholder(ui: &mut egui::Ui, activity: &ozmux_multiplexer::Activity) {
    let bg = kind_color(&activity.kind);
    egui::Frame::default().fill(bg).show(ui, |ui| {
        ui.expand_to_include_rect(ui.max_rect());
        ui.centered_and_justified(|ui| {
            ui.horizontal(|ui| {
                ui.label(&activity.name);
                ui.label(short_id(&activity.id));
            });
        });
    });

    // TODO: Phase 3+ — replace the ActivityKind::Terminal branch with EguiBevyPaintCallback for GPU terminal grid rendering.
}

/// First 8 bytes of an `ActivityId`'s UUID string (UUID v4 is always 36 ASCII chars).
fn short_id(id: &ActivityId) -> &str {
    &AsRef::<str>::as_ref(id)[..8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_color_terminal_uses_activity_terminal_constant() {
        use ozmux_multiplexer::ActivityKind;
        assert_eq!(
            kind_color(&ActivityKind::Terminal),
            crate::ui::egui_theme::palette().activity_terminal
        );
    }

    #[test]
    fn kind_color_browser_uses_activity_browser_constant() {
        use ozmux_multiplexer::ActivityKind;
        let kind = ActivityKind::Browser {
            initial_url: None,
            profile: ozmux_multiplexer::BrowserProfile::default(),
        };
        assert_eq!(
            kind_color(&kind),
            crate::ui::egui_theme::palette().activity_browser
        );
    }
}
