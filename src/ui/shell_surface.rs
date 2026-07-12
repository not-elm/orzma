//! Shell-surface subtree: lazily (re)spawns the single `OrzmaTerminal` shell
//! under `UiRoot`. Owns the surface entity's UI-side lifecycle;
//! `crate::session` owns the PTY-side policy (spawn config, layout, exit).

use crate::input::focus::KeyboardFocused;
use crate::session::spawn::{OrzmaSpawnOptions, OrzmaTerminalBundle, OrzmaTerminalConfig};
use crate::ui::UiRoot;
use bevy::prelude::*;
use orzma_webview::ControlPlaneHandle;

/// Root of the shell-surface subtree, mounted under `UiRoot`.
#[derive(Component)]
pub(crate) struct ShellSurfaceUi;

/// Bevy plugin that ensures the shell-surface subtree exists. Gated by the
/// absence of `ShellSurfaceUi`.
pub(super) struct ShellSurfacePlugin;

impl Plugin for ShellSurfacePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            ensure_shell_surface_ui.run_if(not(any_with_component::<ShellSurfaceUi>)),
        );
    }
}

fn ensure_shell_surface_ui(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    ui_root: Query<Entity, With<UiRoot>>,
    config: Res<OrzmaTerminalConfig>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let Ok(ui_root) = ui_root.single() else {
        return;
    };
    // NOTE: spawn the ShellSurfaceUi container before attempting the PTY spawn.
    // The run condition gates on `ShellSurfaceUi` being absent; if the PTY spawn
    // failed and we returned without the container, this Update system would
    // re-fire every frame — re-attempting the PTY and re-writing AppExit.
    // Spawning the container first makes a failure a single attempt.
    let mode_ui = spawn_shell_surface_container(&mut commands, ui_root);
    let shell = commands.spawn_empty().id();
    let env = control
        .as_deref()
        .map(|c| c.surface_env(shell).to_vec())
        .unwrap_or_default();
    match OrzmaTerminalBundle::spawn(OrzmaSpawnOptions {
        shell: config.shell.clone(),
        env,
        ..default()
    }) {
        Ok(bundle) => {
            commands.entity(shell).insert((
                bundle,
                KeyboardFocused,
                ShellTerminal,
                ChildOf(mode_ui),
            ));
            // NOTE: bind the token only after a successful spawn. gc keys on
            // RemovedComponents<TerminalHandle> (never added on the error path),
            // so a pre-spawn bind would leak the token if the spawn failed.
            if let Some(c) = control.as_deref() {
                c.bind_surface(shell);
            }
        }
        Err(e) => {
            commands.entity(shell).despawn();
            tracing::error!(?e, "failed to spawn orzma terminal");
            exit.write(AppExit::Success);
        }
    }
}

/// Marker for the single shell terminal entity.
#[derive(Component)]
struct ShellTerminal;

/// Spawns the `ShellSurfaceUi` container node under `ui_root` and returns it.
fn spawn_shell_surface_container(commands: &mut Commands, ui_root: Entity) -> Entity {
    commands
        .spawn((
            Name::new("Shell Surface UI"),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            ShellSurfaceUi,
            ChildOf(ui_root),
        ))
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_webview::TokenRegistry;
    use std::path::PathBuf;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins((
            crate::session::SessionPlugin { shell: None },
            ShellSurfacePlugin,
        ));
        app
    }

    #[test]
    fn spawns_shell_surface_ui_once() {
        let mut app = build_app();
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<ShellSurfaceUi>>();
        assert_eq!(q.iter(world).count(), 1, "exactly one ShellSurfaceUi");
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<ShellSurfaceUi>>();
        assert_eq!(
            q.iter(world).count(),
            1,
            "still exactly one ShellSurfaceUi after second update"
        );
    }

    #[test]
    fn shell_terminal_registers_a_resolvable_webview_token() {
        let mut app = build_app();
        let tokens = TokenRegistry::default();
        app.world_mut().insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/ctl.sock"),
            tokens: tokens.clone(),
        });
        app.update();

        let shell = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<ShellTerminal>>()
                .single(world)
                .expect("ShellTerminal spawned")
        };
        let token = format!("orzma:{}", shell.to_bits());
        assert_eq!(
            tokens.resolve(&token),
            Some(shell),
            "the shell terminal's $ORZMA_TOKEN must resolve to its own surface entity"
        );
    }
}
