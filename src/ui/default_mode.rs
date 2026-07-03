//! Default-mode UI subtree: lazily (re)spawns the single `OzmaTerminal` shell
//! under `UiRoot` while in `AppMode::Default`.

use crate::app_mode::AppMode;
use crate::input::focus::KeyboardFocused;
use crate::session::default::spawn::{
    OzmaSpawnOptions, OzmaTerminalBundle, OzmaTerminalConfig, full_size_node,
};
use crate::ui::UiRoot;
use bevy::prelude::*;
use ozma_tty_engine::ControlModeWatch;
use ozma_webview::ControlPlaneHandle;

/// Root of the Default-mode UI subtree, mounted under `UiRoot`.
///
/// Adoption (`crate::session::tmux::adopt`) despawns this container when it promotes the
/// Default shell to the tmux gateway, so `ensure_default_mode_ui` lazily spawns
/// a fresh Default shell on the next return to `AppMode::Default`.
#[derive(Component)]
pub(crate) struct DefaultModeUi;

/// Bevy plugin that ensures the Default-mode UI subtree exists while in
/// `AppMode::Default`. Gated by the absence of `DefaultModeUi`.
pub(super) struct DefaultModeUiPlugin;

impl Plugin for DefaultModeUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            ensure_default_mode_ui
                .run_if(in_state(AppMode::Default).and(not(any_with_component::<DefaultModeUi>))),
        );
    }
}

/// Restores a released tmux gateway as the Default-mode shell.
///
/// Spawns a fresh `DefaultModeUi` container under `ui_root` and reparents
/// `shell` into it with keyboard focus and the full-size layout — adoption
/// overwrote the shell's `Node` with `Display::None` + defaults, so the full
/// node is re-inserted, not just `display` flipped back.
pub(crate) fn restore_default_shell(commands: &mut Commands, shell: Entity, ui_root: Entity) {
    let mode_ui = spawn_default_mode_container(commands, ui_root);
    commands.entity(shell).insert((
        full_size_node(),
        KeyboardFocused,
        DefaultShell,
        ChildOf(mode_ui),
    ));
}

fn ensure_default_mode_ui(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    ui_root: Query<Entity, With<UiRoot>>,
    config: Res<OzmaTerminalConfig>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let Ok(ui_root) = ui_root.single() else {
        return;
    };
    // NOTE: spawn the DefaultModeUi container before attempting the PTY spawn.
    // The run condition gates on `DefaultModeUi` being absent; if the PTY spawn
    // failed and we returned without the container, this Update system would
    // re-fire every frame — re-attempting the PTY and re-writing AppExit.
    // Spawning the container first makes a failure a single attempt.
    let mode_ui = spawn_default_mode_container(&mut commands, ui_root);
    let shell = commands.spawn_empty().id();
    let env = control
        .as_deref()
        .map(|c| c.surface_env(shell).to_vec())
        .unwrap_or_default();
    match OzmaTerminalBundle::spawn(OzmaSpawnOptions {
        shell: config.shell.clone(),
        env,
        ..default()
    }) {
        Ok(bundle) => {
            commands.entity(shell).insert((
                bundle,
                KeyboardFocused,
                ControlModeWatch::default(),
                DefaultShell,
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
            tracing::error!(?e, "failed to spawn ozma terminal");
            exit.write(AppExit::Success);
        }
    }
}

/// Marker for the single Default-mode shell terminal entity. Persists across
/// `AppMode::Default` ↔ `AppMode::Tmux` round-trips when the Default shell
/// is not adopted as the tmux gateway.
#[derive(Component)]
struct DefaultShell;

/// Spawns the `DefaultModeUi` container node under `ui_root` and returns it.
fn spawn_default_mode_container(commands: &mut Commands, ui_root: Entity) -> Entity {
    commands
        .spawn((
            Name::new("Default Mode UI"),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            DefaultModeUi,
            ChildOf(ui_root),
        ))
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_mode::AppMode;
    use bevy::state::app::StatesPlugin;
    use ozma_webview::TokenRegistry;
    use std::path::PathBuf;

    fn build_app(initial_mode: AppMode) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(initial_mode);
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins((
            crate::session::default::DefaultSessionPlugin { shell: None },
            DefaultModeUiPlugin,
        ));
        app
    }

    #[test]
    fn spawns_default_mode_ui_once() {
        let mut app = build_app(AppMode::Default);
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<DefaultModeUi>>();
        assert_eq!(q.iter(world).count(), 1, "exactly one DefaultModeUi");
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<DefaultModeUi>>();
        assert_eq!(
            q.iter(world).count(),
            1,
            "still exactly one DefaultModeUi after second update"
        );
    }

    #[test]
    fn default_shell_survives_mode_roundtrip() {
        let mut app = build_app(AppMode::Default);
        app.update();

        let shell_entity = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<DefaultShell>>()
                .single(world)
                .expect("DefaultShell spawned")
        };

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Tmux);
        app.update();

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Default);
        app.update();

        assert!(
            app.world_mut().get_entity(shell_entity).is_ok(),
            "DefaultShell entity survived Default → Tmux → Default round-trip"
        );

        let world = app.world_mut();
        let count = world
            .query_filtered::<(), With<DefaultShell>>()
            .iter(world)
            .count();
        assert_eq!(count, 1, "exactly one DefaultShell after round-trip");
    }

    #[test]
    fn default_shell_registers_a_resolvable_webview_token() {
        let mut app = build_app(AppMode::Default);
        let tokens = TokenRegistry::default();
        app.world_mut().insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/ctl.sock"),
            tokens: tokens.clone(),
        });
        app.update();

        let shell = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<DefaultShell>>()
                .single(world)
                .expect("DefaultShell spawned")
        };
        let token = format!("ozma:{}", shell.to_bits());
        assert_eq!(
            tokens.resolve(&token),
            Some(shell),
            "the default shell's $OZMA_TOKEN must resolve to its own surface entity"
        );
    }

    #[test]
    fn restore_default_shell_rebuilds_container_focus_and_layout() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let ui_root = app.world_mut().spawn((Node::default(), UiRoot)).id();
        let shell = app
            .world_mut()
            .spawn(Node {
                display: Display::None,
                ..default()
            })
            .id();

        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                restore_default_shell(&mut commands, shell, ui_root);
            })
            .unwrap();

        let world = app.world_mut();
        let container = world
            .query_filtered::<Entity, With<DefaultModeUi>>()
            .single(world)
            .expect("restore spawns exactly one DefaultModeUi container");
        let shell_ref = world.entity(shell);
        assert_eq!(
            shell_ref.get::<ChildOf>().map(|c| c.parent()),
            Some(container),
            "shell reparented under the fresh container"
        );
        assert!(
            shell_ref.get::<KeyboardFocused>().is_some(),
            "focus restored"
        );
        assert!(shell_ref.get::<DefaultShell>().is_some(), "marker present");
        let node = shell_ref.get::<Node>().expect("node restored");
        assert_eq!(node.position_type, PositionType::Absolute);
        assert_eq!(node.width, Val::Percent(100.0));
        assert_ne!(node.display, Display::None, "no longer hidden");
    }
}
