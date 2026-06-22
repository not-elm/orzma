//! A modal overlay shown when the tmux backend reports an error.

use crate::app_mode::AppMode;
use bevy::app::{App, Plugin, PostUpdate, Startup};
use bevy::color::Color;
use bevy::ecs::component::Component;
use bevy::ecs::query::With;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::schedule::common_conditions::resource_exists_and_changed;
use bevy::ecs::system::{Commands, Query, Res};
use bevy::prelude::{OnExit, default, in_state};
use bevy::ui::widget::Text;
use bevy::ui::{
    AlignItems, BackgroundColor, Display, GlobalZIndex, JustifyContent, Node, PositionType, Val,
};
use ozmux_tmux::ConnectionState;

const TMUX_DIALOG_Z: i32 = 300;

/// Spawns and toggles the tmux error dialog overlay.
pub(crate) struct DialogPlugin;

impl Plugin for DialogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_tmux_dialog)
            .add_systems(OnExit(AppMode::Tmux), hide_tmux_dialog)
            .add_systems(
                PostUpdate,
                sync_tmux_dialog
                    .run_if(resource_exists_and_changed::<ConnectionState>)
                    .run_if(in_state(AppMode::Tmux)),
            );
    }
}

#[derive(Component)]
struct TmuxDialogBackdrop;

#[derive(Component)]
struct TmuxDialogText;

fn spawn_tmux_dialog(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::None,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            GlobalZIndex(TMUX_DIALOG_Z),
            TmuxDialogBackdrop,
        ))
        .with_children(|parent| {
            parent.spawn((Text::new("tmux unavailable"), TmuxDialogText));
        });
}

fn hide_tmux_dialog(mut backdrop: Query<&mut Node, With<TmuxDialogBackdrop>>) {
    if let Ok(mut node) = backdrop.single_mut() {
        node.display = Display::None;
    }
}

fn sync_tmux_dialog(
    mut backdrop: Query<&mut Node, With<TmuxDialogBackdrop>>,
    mut text: Query<&mut Text, With<TmuxDialogText>>,
    state: Res<ConnectionState>,
) {
    let Ok(mut node) = backdrop.single_mut() else {
        return;
    };
    match &*state {
        ConnectionState::Error { reason } => {
            node.display = Display::Flex;
            if let Ok(mut label) = text.single_mut() {
                label.0 = format!("tmux unavailable\n{reason}");
            }
        }
        ConnectionState::Detached => {
            node.display = Display::Flex;
            if let Ok(mut label) = text.single_mut() {
                label.0 = "Disconnected — run `tmux -CC` to start a tmux session".to_string();
            }
        }
        _ => node.display = Display::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::prelude::AppExtStates;

    #[test]
    fn dialog_shows_on_error_and_detached() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.insert_state(AppMode::Tmux);
        app.init_resource::<ConnectionState>();
        app.add_plugins(DialogPlugin);
        app.update();

        fn backdrop_display(app: &mut App) -> Display {
            let mut q = app
                .world_mut()
                .query_filtered::<&Node, With<TmuxDialogBackdrop>>();
            q.single(app.world()).unwrap().display
        }

        assert_eq!(backdrop_display(&mut app), Display::None);

        app.insert_resource(ConnectionState::Error {
            reason: "tmux: command not found".to_string(),
        });
        app.update();
        assert_eq!(backdrop_display(&mut app), Display::Flex);

        app.insert_resource(ConnectionState::Detached);
        app.update();
        assert_eq!(backdrop_display(&mut app), Display::Flex);

        app.insert_resource(ConnectionState::Attached);
        app.update();
        assert_eq!(backdrop_display(&mut app), Display::None);
    }

    #[test]
    fn dialog_hidden_on_exit_tmux() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin);
        app.insert_state(AppMode::Tmux);
        app.init_resource::<ConnectionState>();
        app.add_plugins(DialogPlugin);
        app.update();

        fn backdrop_display(app: &mut App) -> Display {
            let mut q = app
                .world_mut()
                .query_filtered::<&Node, With<TmuxDialogBackdrop>>();
            q.single(app.world()).unwrap().display
        }

        app.insert_resource(ConnectionState::Detached);
        app.update();
        assert_eq!(backdrop_display(&mut app), Display::Flex);

        app.insert_resource(bevy::prelude::NextState::Pending(AppMode::Default));
        app.update();
        assert_eq!(backdrop_display(&mut app), Display::None);
    }
}
