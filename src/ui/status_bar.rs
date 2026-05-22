//! Egui status bar rendering: `draw_status_bar` renders the session name and
//! a list of window chips into a `TopBottomPanel::bottom`. The active window's
//! chip is filled with ACCENT background.

use std::collections::HashMap;

use bevy_egui::egui;
use ozmux_multiplexer::{Session, Window, WindowId};

/// Egui draw of the status bar (rendered into a `TopBottomPanel::bottom`).
/// Same content as Phase 2: session name + window chips, with the active
/// window chip filled in ACCENT.
pub(crate) fn draw_status_bar(
    ui: &mut egui::Ui,
    session: &Session,
    active_wid: &WindowId,
    windows: &HashMap<WindowId, Window>,
) {
    let p = crate::ui::egui_theme::palette();
    egui::Frame::default()
        .fill(p.panel)
        .inner_margin(egui::Margin::symmetric(
            crate::theme::ELEMENT_PADDING_PX as i8,
            0,
        ))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.colored_label(p.foreground, &session.name);

                ui.add_space(crate::theme::ELEMENT_PADDING_PX);

                for wid in &session.linked_windows {
                    let fallback;
                    let label: &str = match windows.get(wid) {
                        Some(w) => w.name.as_str(),
                        None => {
                            fallback = wid.to_string();
                            &fallback
                        }
                    };
                    let chip_bg = if wid == active_wid {
                        p.accent
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    egui::Frame::default()
                        .fill(chip_bg)
                        .inner_margin(egui::Margin::symmetric(
                            crate::theme::ELEMENT_PADDING_PX as i8,
                            0,
                        ))
                        .show(ui, |ui| {
                            ui.colored_label(p.foreground, label);
                        });
                }
            });
        });
}
