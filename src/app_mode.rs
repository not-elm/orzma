//! AppMode state enum and the Default-mode UI subtree lifecycle plugin.

use crate::ui::UiRoot;
use bevy::prelude::*;
use ozma_terminal::{KeyboardFocused, OzmaSpawnOptions, OzmaTerminalBundle, OzmaTerminalConfig};
use ozma_tty_engine::ControlModeWatch;
use ozma_webview::ControlPlaneHandle;

/// Application mode. `Default` is the default (single PTY, no tmux).
/// `Tmux` activates the tmux multiplexer backend.
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) enum AppMode {
    /// Single PTY terminal, Alacritty VT emulation, no tmux.
    #[default]
    Default,
    /// tmux backend, multiplexer layout.
    Tmux,
}

/// Root of the Default-mode UI subtree, mounted under `UiRoot`.
///
/// Adoption (`crate::mode::tmux::adopt`) despawns this container when it promotes the
/// Default shell to the tmux gateway, so `ensure_default_mode_ui` lazily spawns
/// a fresh Default shell on the next return to `AppMode::Default`.
#[derive(Component)]
pub(crate) struct DefaultModeUi;

/// Marker for the single Default-mode shell terminal entity. Persists across
/// `AppMode::Default` ↔ `AppMode::Tmux` round-trips when the Default shell
/// is not adopted as the tmux gateway.
#[derive(Component)]
struct DefaultShell;

/// Bevy plugin that ensures the Default-mode UI subtree (a single
/// `OzmaTerminal` under `DefaultModeUi`) exists while in `AppMode::Default`.
///
/// `ensure_default_mode_ui` runs once while in `AppMode::Default` to build the
/// subtree. Adoption despawns `DefaultModeUi` when it promotes the Default shell
/// to the tmux gateway; `ensure_default_mode_ui` spawns a fresh one on the next
/// return to `AppMode::Default`. `OzmaTerminalPlugin` must be added first (it
/// inserts the `OzmaTerminalConfig` this reads).
pub(crate) struct DefaultModePlugin;

impl Plugin for DefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            ensure_default_mode_ui
                .run_if(in_state(AppMode::Default).and(not(any_with_component::<DefaultModeUi>))),
        );
    }
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
    let mode_ui = commands
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
        .id();
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
            // RemovedComponents<OzmaTerminal> (never added on the error path),
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;
    use ozma_webview::TokenRegistry;
    use std::path::PathBuf;

    fn build_app(initial_mode: AppMode) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(initial_mode);
        app.insert_resource(OzmaTerminalConfig { shell: None });
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(DefaultModePlugin);
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
}
