//! `OzmuxUiPlugin` wires the egui-based UI: per-window Camera + EguiContext
//! setup, one-shot Visuals application, and the immediate-mode draw system
//! registered in `EguiPrimaryContextPass` so it runs inside the egui frame
//! for the primary window.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::{EguiContext, EguiPlugin, EguiPrimaryContextPass};

use crate::multiplexer::{AttachedSession, Multiplexer};
use crate::ui::root::{WindowCamera, apply_visuals_for_new_contexts, setup_egui_for_window};

pub(crate) mod activity;
pub(crate) mod egui_theme;
pub(crate) mod layout;
pub(crate) mod root;
pub(crate) mod status_bar;
pub(crate) mod tab_bar;

/// Bevy Plugin for the Phase 2.5 egui-based UI shell.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default());
        app.add_systems(
            Update,
            (setup_egui_for_window, apply_visuals_for_new_contexts).chain(),
        );
        app.add_systems(
            EguiPrimaryContextPass,
            draw_ui_in_primary.after(crate::input::dispatch_focused_key),
        );
    }
}

fn draw_ui_in_primary(
    mut cameras: Query<&mut EguiContext, With<WindowCamera>>,
    mux: Res<Multiplexer>,
    attached: Query<&AttachedSession, With<Window>>,
) {
    // NOTE: `single_mut()` panics if more than one WindowCamera entity exists
    //   (Phase 4+ multi-window adds a per-window draw system instead of
    //   extending this one).
    let Ok(mut ctx_component) = cameras.single_mut() else {
        return;
    };
    let Ok(attached) = attached.single() else {
        return;
    };
    let ctx = ctx_component.get_mut();

    let Ok(session) = mux.sessions.get(&attached.0) else {
        return;
    };
    let Some(active_wid) = session.active_window.as_ref() else {
        return;
    };
    let Some(window) = mux.windows.get(active_wid) else {
        return;
    };

    egui::TopBottomPanel::bottom("ozmux_status").show(ctx, |ui| {
        status_bar::draw_status_bar(ui, session, active_wid, &mux.windows);
    });

    egui::CentralPanel::default().show(ctx, |ui| {
        if let Ok(ozmux_multiplexer::Cell::Root(root)) = window.cells.cell(&window.root_cell) {
            layout::draw_cell_recursive(ui, window, &root.child);
        }
    });
}
