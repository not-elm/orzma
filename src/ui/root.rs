//! Spawns the singleton UiRoot Node and a 2D Camera under PrimaryWindow.
//! Runs after bootstrap, but no longer depends on AttachedSession (which
//! now lives on session entities, not on the OS window).

use crate::ui::{SessionUiRoot, UiRoot};
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::ui::{FlexDirection, IsDefaultUiCamera, Val};
use bevy::window::{PrimaryWindow, WindowRef};

/// Marker for the `Camera2d` entity that renders the primary GUI window.
#[derive(Component)]
pub struct WindowCamera;

pub struct OzmuxUiRootPlugin;

impl Plugin for OzmuxUiRootPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (spawn_camera, spawn_root_ui));
    }
}

fn spawn_camera(mut commands: Commands, primary: Query<Entity, With<PrimaryWindow>>) {
    let Ok(window_entity) = primary.single() else {
        tracing::error!(
            target: "ozmux_gui::ui",
            "setup_root_camera_and_ui_root: primary window missing",
        );
        return;
    };

    commands.spawn((
        Camera2d,
        RenderTarget::Window(WindowRef::Entity(window_entity)),
        WindowCamera,
        IsDefaultUiCamera,
    ));
}

fn spawn_root_ui(mut commands: Commands) {
    let ui_root_entity = commands
        .spawn((
            Name::new("UI Root"),
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            UiRoot,
        ))
        .id();

    commands.spawn((
        Name::new("Session UI Root"),
        Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        SessionUiRoot,
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
        app.add_systems(Startup, spawn_root_ui);
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
