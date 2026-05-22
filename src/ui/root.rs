//! Per-GUI-window egui context setup: spawns Camera2d (targeting the window)
//! with `EguiContext` + `EguiMultipassSchedule` for every Window entity that
//! gains `AttachedSession`, and applies the ozmux palette to each new context.

use crate::multiplexer::AttachedSession;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::window::WindowRef;
use bevy_egui::{EguiContext, EguiMultipassSchedule, EguiPrimaryContextPass};

/// Marker for `Camera2d` entities that target exactly one GUI window. One per
/// `(Window, AttachedSession)` pair.
#[derive(Component, Debug)]
pub(crate) struct WindowCamera;

/// Filter type for the `new_attached` query in `setup_egui_for_window`.
type NewEguiWindowFilter = (With<Window>, Added<AttachedSession>);

/// Reacts to `Added<AttachedSession>` on `Window` entities. For each newly
/// attached window, spawns a `WindowCamera` (`Camera2d` targeting the window)
/// with `EguiContext` + `EguiMultipassSchedule` so the per-window egui draw
/// systems fire inside the `EguiPrimaryContextPass` schedule. Only the primary
/// (single) window is in scope for Phase 2→3; secondary windows would require
/// a distinct schedule label, which is deferred.
pub(crate) fn setup_egui_for_window(
    mut commands: Commands,
    new_attached: Query<Entity, NewEguiWindowFilter>,
) {
    for window_entity in &new_attached {
        commands.spawn((
            Camera2d,
            RenderTarget::Window(WindowRef::Entity(window_entity)),
            WindowCamera,
            EguiContext::default(),
            EguiMultipassSchedule::new(EguiPrimaryContextPass),
        ));
    }
}

/// Applies the ozmux egui palette to each newly added `EguiContext`. Runs in
/// `Update` so it catches the freshly-spawned context before its first draw pass.
pub(crate) fn apply_visuals_for_new_contexts(
    mut new_contexts: Query<&mut EguiContext, Added<EguiContext>>,
) {
    for mut ctx_component in &mut new_contexts {
        ctx_component
            .get_mut()
            .set_visuals(crate::ui::egui_theme::ozmux_visuals());
    }
}
