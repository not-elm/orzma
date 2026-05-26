//! Spawns the singleton UiRoot Node and a 2D Camera under PrimaryWindow.
//! Runs after bootstrap, but no longer depends on AttachedSession (which
//! now lives on session entities, not on the OS window).

use crate::ui::UiRoot;
use bevy::prelude::*;
use bevy::ui::IsDefaultUiCamera;
use bevy::window::PrimaryWindow;

/// Marker for the `Camera2d` entity that renders the primary GUI window.
#[derive(Component)]
pub(crate) struct WindowCamera;

/// Spawn the per-window `Camera2d` (tagged `IsDefaultUiCamera` so root
/// `Node`s without explicit `UiTargetCamera` resolve to it) and the
/// `UiRoot` Node entity.
pub(crate) fn setup_root_camera_and_ui_root(
    mut commands: Commands,
    primary: Query<Entity, With<PrimaryWindow>>,
) {
    let Ok(_window_entity) = primary.single() else {
        tracing::warn!(
            target: "ozmux_gui::ui",
            "setup_root_camera_and_ui_root: primary window missing",
        );
        return;
    };

    commands.spawn((
        Camera2d,
        // RenderTarget::Window(WindowRef::Entity(window_entity)),
        WindowCamera,
        IsDefaultUiCamera,
    ));

    let ui_root_entity = commands
        .spawn((
            Node {
                flex_direction: bevy::ui::FlexDirection::Column,
                width: bevy::ui::Val::Percent(100.0),
                height: bevy::ui::Val::Percent(100.0),
                ..default()
            },
            UiRoot,
            // NOTE: UiRoot does NOT carry StructuralNode — it must persist
            // across rebuilds. Its children carry StructuralNode and are
            // recycled by `rebuild_session_ui_on_data_change`.
        ))
        .id();

    commands.spawn((
        Node {
            flex_grow: 1.0,
            width: bevy::ui::Val::Percent(100.0),
            ..default()
        },
        crate::ui::SessionUiRoot,
        ChildOf(ui_root_entity),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::SessionUiRoot;
    use bevy::window::{PrimaryWindow, WindowResolution};

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        app.add_systems(Startup, setup_root_camera_and_ui_root);
        app.update();
        app
    }

    #[test]
    fn setup_spawns_session_ui_root_under_ui_root() {
        let mut app = build_app();
        let world = app.world_mut();
        let ui_root = world
            .query_filtered::<Entity, With<crate::ui::UiRoot>>()
            .single(world)
            .expect("UiRoot present");
        let session_ui_root = world
            .query_filtered::<Entity, With<SessionUiRoot>>()
            .single(world)
            .expect("SessionUiRoot present");
        let parent = world
            .get::<ChildOf>(session_ui_root)
            .expect("SessionUiRoot has ChildOf")
            .parent();
        assert_eq!(parent, ui_root, "SessionUiRoot must be a child of UiRoot");
    }
}
