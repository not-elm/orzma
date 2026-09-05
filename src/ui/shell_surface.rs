//! Shell-surface subtree: the clipping container every pane lives under,
//! the one-shot root spawn, and the spawn-failure handler.

use crate::session::spawn::PaneSpawnRequest;
use crate::ui::UiRoot;
use bevy::prelude::*;
use bevy_orzma_mux::prelude::{MuxPane, MuxPaneSpawnFailed, PaneGeometry};
use bevy_orzma_webview::ControlPlaneHandle;
use orzma_mux::prelude::NewPaneAt;

/// Root of the shell-surface subtree, mounted under `UiRoot`. Clips its
/// children so a layout wider than the window overflows invisibly.
#[derive(Component)]
pub(crate) struct ShellSurfaceUi;

pub(super) struct ShellSurfacePlugin;

impl Plugin for ShellSurfacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RootRequested>()
            .add_systems(
                Update,
                (
                    ensure_shell_surface_ui.run_if(not(any_with_component::<ShellSurfaceUi>)),
                    request_root_pane
                        .run_if(any_with_component::<ShellSurfaceUi>)
                        .run_if(resource_exists::<PaneGeometry>)
                        .run_if(root_not_requested),
                ),
            )
            .add_observer(on_spawn_failed);
    }
}

/// Marks that the root pane was requested once.
#[derive(Resource, Default)]
struct RootRequested(bool);

fn root_not_requested(requested: Res<RootRequested>) -> bool {
    !requested.0
}

fn ensure_shell_surface_ui(mut commands: Commands, ui_root: Query<Entity, With<UiRoot>>) {
    let Ok(ui_root) = ui_root.single() else {
        return;
    };
    commands.spawn((
        Name::new("Shell Surface UI"),
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            overflow: Overflow::clip(),
            ..default()
        },
        ShellSurfaceUi,
        ChildOf(ui_root),
    ));
}

/// Asks for the first pane once the geometry has been sent (D17: the
/// backend refuses `NewPane` before its first `Resize`).
fn request_root_pane(mut commands: Commands, mut requested: ResMut<RootRequested>) {
    requested.0 = true;
    commands.trigger(PaneSpawnRequest {
        at: NewPaneAt::Root,
    });
}

/// Unbinds the token and despawns the pending entity; a failed root
/// spawn (no pane at all) exits.
fn on_spawn_failed(
    ev: On<MuxPaneSpawnFailed>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    control: Option<Res<ControlPlaneHandle>>,
    panes: Query<(), With<MuxPane>>,
) {
    if let Some(control) = control.as_deref() {
        control.tokens.remove_entity(ev.entity);
    }
    commands.entity(ev.entity).despawn();
    tracing::error!(error = %ev.error, "pane spawn failed");
    if panes.is_empty() {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::message::Messages;
    use orzma_tty::CellPixels;

    fn app_with_ui_root() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(ShellSurfacePlugin);
        app
    }

    /// Asserts that the container spawns exactly once, with clipping
    /// enabled so an oversized layout never bleeds outside the window.
    ///
    /// Case: the app starts, and the system keeps running on later
    /// frames once the container already exists.
    #[test]
    fn spawns_shell_surface_ui_once() {
        let mut app = app_with_ui_root();
        app.update();
        {
            let world = app.world_mut();
            let mut q = world.query_filtered::<&Node, With<ShellSurfaceUi>>();
            let nodes: Vec<&Node> = q.iter(world).collect();
            assert_eq!(nodes.len(), 1, "exactly one ShellSurfaceUi");
            assert_eq!(nodes[0].overflow, Overflow::clip());
        }
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<ShellSurfaceUi>>();
        assert_eq!(
            q.iter(world).count(),
            1,
            "still exactly one ShellSurfaceUi after second update"
        );
    }

    /// Asserts that the root pane is requested only once geometry
    /// exists, and only once even across many frames.
    ///
    /// Case: the app starts before font metrics load (no
    /// `PaneGeometry` yet), then the geometry arrives.
    #[test]
    fn root_pane_is_requested_once_geometry_exists_and_only_once() {
        #[derive(Resource, Default)]
        struct Spawns(u32);

        let mut app = app_with_ui_root();
        app.init_resource::<Spawns>()
            .add_observer(|_ev: On<PaneSpawnRequest>, mut spawns: ResMut<Spawns>| spawns.0 += 1);
        app.update();
        assert_eq!(
            app.world().resource::<Spawns>().0,
            0,
            "no geometry yet, so no spawn request"
        );

        app.world_mut().insert_resource(PaneGeometry {
            cell_px: CellPixels {
                width: 8,
                height: 16,
            },
            scale_factor: 1.0,
        });
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<Spawns>().0,
            1,
            "exactly one spawn request once geometry exists"
        );
    }

    /// Asserts that a spawn failure unbinds the token, despawns the
    /// pending entity, and exits the app when no pane is left.
    ///
    /// Case: the only pane (the root) fails to spawn its shell.
    #[test]
    fn root_spawn_failure_unbinds_and_exits() {
        let mut app = app_with_ui_root();
        app.add_message::<AppExit>();
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(MuxPaneSpawnFailed {
            entity,
            error: "no space".into(),
        });
        app.update();

        assert!(app.world().get_entity(entity).is_err());
        let mut messages = app.world_mut().resource_mut::<Messages<AppExit>>();
        assert_eq!(
            messages.drain().count(),
            1,
            "the last pane's spawn failure exits"
        );
    }
}
