//! Per-GUI-window Camera + UiRoot setup. Spawned once at Startup AFTER
//! `bootstrap` has attached `AttachedSession` to the primary Window.

use crate::multiplexer::AttachedSession;
use crate::ui::UiRoot;
use bevy::prelude::*;
use bevy::ui::IsDefaultUiCamera;
use bevy::window::PrimaryWindow;

/// Marker for the `Camera2d` entity that renders the primary GUI window.
#[derive(Component)]
pub(crate) struct WindowCamera;

/// Spawn the per-window `Camera2d` (tagged `IsDefaultUiCamera` so root
/// `Node`s without explicit `UiTargetCamera` resolve to it) and the
/// `UiRoot` Node entity. Reads `AttachedSession` to confirm bootstrap
/// ran first; logs a warn and returns early if missing (system-ordering
/// safety net — the Plugin orders `.after(bootstrap)` so this is rare).
pub(crate) fn setup_root_camera_and_ui_root(
    mut commands: Commands,
    primary: Query<Entity, (With<PrimaryWindow>, With<AttachedSession>)>,
) {
    let Ok(_window_entity) = primary.single() else {
        tracing::warn!(
            target: "ozmux_gui::ui",
            "setup_root_camera_and_ui_root: primary window without AttachedSession — bootstrap order?",
        );
        return;
    };

    commands.spawn((
        Camera2d,
        // RenderTarget::Window(WindowRef::Entity(window_entity)),
        WindowCamera,
        IsDefaultUiCamera,
    ));

    commands.spawn((
        Node {
            flex_direction: bevy::ui::FlexDirection::Column,
            width: bevy::ui::Val::Percent(100.0),
            height: bevy::ui::Val::Percent(100.0),
            ..default()
        },
        UiRoot,
        // NOTE: UiRoot does NOT carry StructuralNode — it must persist
        // across rebuilds. Its children carry StructuralNode and are
        // recycled by `rebuild_structure_on_change`.
    ));
}
