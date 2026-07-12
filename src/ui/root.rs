//! Spawns the singleton UiRoot Node and a 2D Camera under PrimaryWindow.
//! Runs after bootstrap. Each mode's UI subtree attaches under `UiRoot` while
//! that mode is active.

use crate::ui::UiRoot;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::ui::{IsDefaultUiCamera, Val};
use bevy::window::{PrimaryWindow, WindowRef};

/// Marker for the `Camera2d` entity that renders the primary GUI window.
#[derive(Component)]
pub struct WindowCamera;

pub struct OrzmaUiRootPlugin;

impl Plugin for OrzmaUiRootPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::BLACK))
            .add_systems(Startup, (spawn_camera, spawn_root_ui));
    }
}

fn spawn_camera(mut commands: Commands, primary: Query<Entity, With<PrimaryWindow>>) {
    let Ok(window_entity) = primary.single() else {
        tracing::error!(
            target: "orzma::ui",
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
    commands.spawn((
        Name::new("UI Root"),
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        UiRoot,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn setup_spawns_ui_root() {
        let mut app = build_app();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<UiRoot>>();
        assert_eq!(q.iter(world).count(), 1, "UiRoot present");
    }
}
