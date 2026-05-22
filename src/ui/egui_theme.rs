//! Adapts the Phase 2 theme.rs palette to egui::Visuals.
//! theme.rs stays the source of truth (Bevy Color); this file is the only
//! place that knows about egui's color/visuals types.

use bevy::prelude::Color;
use bevy_egui::egui::{Color32, Stroke, Visuals};

use crate::theme;

/// Converts a Bevy `Color` to an egui `Color32` (sRGB, unmultiplied alpha).
pub(crate) fn to_egui(c: Color) -> Color32 {
    let srgba = c.to_srgba();
    Color32::from_rgba_unmultiplied(
        (srgba.red * 255.0).round() as u8,
        (srgba.green * 255.0).round() as u8,
        (srgba.blue * 255.0).round() as u8,
        (srgba.alpha * 255.0).round() as u8,
    )
}

/// Returns egui `Visuals` populated with the ozmux theme palette.
pub(crate) fn ozmux_visuals() -> Visuals {
    let mut v = Visuals::dark();

    v.panel_fill = to_egui(theme::BACKGROUND);
    v.window_fill = to_egui(theme::BACKGROUND);
    v.extreme_bg_color = to_egui(theme::PANEL);
    v.faint_bg_color = to_egui(theme::TAB_BAR_BG);

    v.widgets.noninteractive.bg_fill = to_egui(theme::TAB_BAR_BG);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, to_egui(theme::BORDER));
    v.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, to_egui(theme::MUTED));
    v.widgets.active.bg_fill = to_egui(theme::TAB_ACTIVE_BG);
    v.widgets.active.fg_stroke = Stroke::new(1.0, to_egui(theme::FOREGROUND));

    v.selection.bg_fill = to_egui(theme::ACCENT).gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(2.0, to_egui(theme::ACCENT));
    v.hyperlink_color = to_egui(theme::ACCENT);

    v.override_text_color = Some(to_egui(theme::FOREGROUND));

    v
}

/// Pre-converted egui-space palette derived from `crate::theme` constants.
/// Hot-path draw code reads from `palette()` to avoid per-frame f32→u8
/// conversion of the immutable theme constants.
pub(crate) struct EguiPalette {
    /// Status bar / panel background color.
    pub panel: Color32,
    /// Tab bar background color.
    pub tab_bar_bg: Color32,
    /// Active tab background color.
    pub tab_active_bg: Color32,
    /// Pane border color.
    pub border: Color32,
    /// Primary text color.
    pub foreground: Color32,
    /// Secondary / muted text color.
    pub muted: Color32,
    /// Active highlight color (accent).
    pub accent: Color32,
    /// Terminal activity placeholder background.
    pub activity_terminal: Color32,
    /// Browser activity placeholder background.
    pub activity_browser: Color32,
    /// Extension activity placeholder background.
    pub activity_extension: Color32,
}

static PALETTE: std::sync::OnceLock<EguiPalette> = std::sync::OnceLock::new();

/// Returns the lazily-initialized egui-space color palette.
pub(crate) fn palette() -> &'static EguiPalette {
    PALETTE.get_or_init(|| EguiPalette {
        panel: to_egui(theme::PANEL),
        tab_bar_bg: to_egui(theme::TAB_BAR_BG),
        tab_active_bg: to_egui(theme::TAB_ACTIVE_BG),
        border: to_egui(theme::BORDER),
        foreground: to_egui(theme::FOREGROUND),
        muted: to_egui(theme::MUTED),
        accent: to_egui(theme::ACCENT),
        activity_terminal: to_egui(theme::ACTIVITY_TERMINAL),
        activity_browser: to_egui(theme::ACTIVITY_BROWSER),
        activity_extension: to_egui(theme::ACTIVITY_EXTENSION),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_egui_converts_accent_to_expected_hex() {
        // theme::ACCENT = Color::srgb(0.302, 0.561, 0.851) → ~#4D8FD9
        let c = to_egui(theme::ACCENT);
        assert_eq!(c, Color32::from_rgb(77, 143, 217));
    }

    #[test]
    fn ozmux_visuals_uses_background_for_panel_fill() {
        let v = ozmux_visuals();
        assert_eq!(v.panel_fill, to_egui(theme::BACKGROUND));
    }

    #[test]
    fn ozmux_visuals_uses_accent_for_selection_stroke() {
        let v = ozmux_visuals();
        assert_eq!(v.selection.stroke.color, to_egui(theme::ACCENT));
    }
}
