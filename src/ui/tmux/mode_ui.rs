//! Mode-scoped UI for `AppMode::Tmux`: spawns the Tmux subtree under `UiRoot`
//! while in Tmux mode and relies on `DespawnOnExit` to remove it on exit.

use crate::app_mode::AppMode;
use crate::ui::UiRoot;
use crate::ui::tmux::window_bar::spawn_window_bar;
use bevy::prelude::*;
use ozma_tty_renderer::TerminalCellMetricsResource;

/// Root of the Tmux-mode UI subtree, mounted under `UiRoot` while in
/// `AppMode::Tmux`. Carries `DespawnOnExit(AppMode::Tmux)`, so leaving Tmux mode
/// removes the whole subtree.
#[derive(Component)]
struct TmuxModeUi;

/// Workspace container under `TmuxModeUi` where the tmux render layer parents
/// each window container. Spawned with the Tmux subtree; removed with it via
/// `DespawnOnExit`.
#[derive(Component)]
pub(crate) struct WorkspaceUiRoot;

/// Bevy plugin that ensures the Tmux-mode UI subtree exists while in Tmux mode.
pub(crate) struct TmuxModeUiPlugin;

impl Plugin for TmuxModeUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            ensure_tmux_mode_ui
                .run_if(in_state(AppMode::Tmux).and(not(any_with_component::<TmuxModeUi>))),
        );
    }
}

fn ensure_tmux_mode_ui(
    mut commands: Commands,
    ui_root: Query<Entity, With<UiRoot>>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
) {
    let Ok(ui_root) = ui_root.single() else {
        return;
    };
    let tmux_ui = commands
        .spawn((
            Name::new("Tmux Mode UI"),
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            DespawnOnExit(AppMode::Tmux),
            TmuxModeUi,
            ChildOf(ui_root),
        ))
        .id();

    commands.spawn((
        Name::new("Workspace UI Root"),
        Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        WorkspaceUiRoot,
        ChildOf(tmux_ui),
    ));

    spawn_window_bar(&mut commands, tmux_ui, metrics.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tmux::window_bar::WindowBarRoot;
    use bevy::state::app::StatesPlugin;

    fn build_app() -> App {
        use ozma_tty_renderer::CellMetrics;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Tmux);
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        });
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(TmuxModeUiPlugin);
        app
    }

    #[test]
    fn workspace_ui_root_is_child_of_tmux_mode_ui() {
        let mut app = build_app();
        app.update();
        let world = app.world_mut();
        let tmux_ui = world
            .query_filtered::<Entity, With<TmuxModeUi>>()
            .single(world)
            .expect("TmuxModeUi present");
        let parent = world
            .query_filtered::<&ChildOf, With<WorkspaceUiRoot>>()
            .single(world)
            .expect("WorkspaceUiRoot present")
            .parent();
        assert_eq!(parent, tmux_ui, "WorkspaceUiRoot under TmuxModeUi");
    }

    #[test]
    fn spawns_tmux_mode_ui_under_ui_root_in_tmux() {
        let mut app = build_app();
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<TmuxModeUi>>();
        assert_eq!(q.iter(world).count(), 1, "exactly one TmuxModeUi");
    }

    #[test]
    fn despawns_tmux_mode_ui_on_exit_to_default() {
        let mut app = build_app();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Default);
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<TmuxModeUi>>();
        assert_eq!(q.iter(world).count(), 0, "TmuxModeUi removed on exit");
    }

    #[test]
    fn tmux_subtree_includes_window_bar() {
        let mut app = build_app();
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<WindowBarRoot>>();
        assert_eq!(q.iter(world).count(), 1, "window bar mounts in Tmux mode");
    }

    #[test]
    fn leaving_tmux_removes_window_bar() {
        let mut app = build_app();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Default);
        app.update();
        let world = app.world_mut();
        let mut bar = world.query_filtered::<(), With<WindowBarRoot>>();
        let mut ws = world.query_filtered::<(), With<WorkspaceUiRoot>>();
        assert_eq!(bar.iter(world).count(), 0, "window bar removed on exit");
        assert_eq!(ws.iter(world).count(), 0, "workspace removed on exit");
    }
}
