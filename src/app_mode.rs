//! AppMode state enum and the Default-mode UI subtree lifecycle plugin.

use crate::ui::UiRoot;
use bevy::prelude::*;
use ozma_terminal::{KeyboardFocused, OzmaSpawnOptions, OzmaTerminalBundle, OzmaTerminalConfig};
use ozma_tty_engine::ControlModeWatch;

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

/// Root of the Default-mode UI subtree, mounted under `UiRoot`. Persists across
/// mode switches; visibility is toggled by `toggle_default_mode_ui_visibility`
/// rather than despawned.
#[derive(Component)]
struct DefaultModeUi;

/// Marker for the single Default-mode shell terminal entity. Persists across
/// `AppMode::Default` ↔ `AppMode::Tmux` round-trips; the subtree is hidden
/// in `AppMode::Tmux` and shown again on re-entry.
#[derive(Component)]
struct DefaultShell;

/// Bevy plugin that ensures the Default-mode UI subtree (a single
/// `OzmaTerminal` under `DefaultModeUi`) exists while in `AppMode::Default`.
///
/// `ensure_default_mode_ui` runs once (`not(any_with_component::<DefaultModeUi>)`)
/// to build the subtree; it is never despawned. `toggle_default_mode_ui_visibility`
/// shows it in `AppMode::Default` and hides it in `AppMode::Tmux`.
/// `OzmaTerminalPlugin` must be added first (it inserts the `OzmaTerminalConfig`
/// this reads).
pub(crate) struct DefaultModePlugin;

impl Plugin for DefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            ensure_default_mode_ui.run_if(not(any_with_component::<DefaultModeUi>)),
        )
        .add_systems(
            Update,
            toggle_default_mode_ui_visibility.run_if(state_changed::<AppMode>),
        );
    }
}

fn ensure_default_mode_ui(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    ui_root: Query<Entity, With<UiRoot>>,
    config: Res<OzmaTerminalConfig>,
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
    match OzmaTerminalBundle::spawn(OzmaSpawnOptions {
        shell: config.shell.clone(),
        ..default()
    }) {
        Ok(bundle) => {
            commands.spawn((
                bundle,
                KeyboardFocused,
                ControlModeWatch::default(),
                DefaultShell,
                ChildOf(mode_ui),
            ));
        }
        Err(e) => {
            tracing::error!(?e, "failed to spawn ozma terminal");
            exit.write(AppExit::Success);
        }
    }
}

/// Shows `DefaultModeUi` in `AppMode::Default` and hides it in `AppMode::Tmux`.
/// Gated by `state_changed::<AppMode>` so it only runs on transitions.
/// Mutates the `Node.display` field only when the value differs.
fn toggle_default_mode_ui_visibility(
    mode: Res<State<AppMode>>,
    mut mode_ui: Query<&mut Node, With<DefaultModeUi>>,
) {
    let want = match **mode {
        AppMode::Default => Display::Flex,
        AppMode::Tmux => Display::None,
    };
    let Ok(mut node) = mode_ui.single_mut() else {
        return;
    };
    if node.display != want {
        node.display = want;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;

    fn build_app(initial_mode: AppMode) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(initial_mode);
        // Provide OzmaTerminalConfig so ensure_default_mode_ui can read it.
        app.insert_resource(OzmaTerminalConfig { shell: None });
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(DefaultModePlugin);
        app
    }

    #[test]
    fn spawns_default_mode_ui_once() {
        let mut app = build_app(AppMode::Default);
        // First update: ensure_default_mode_ui fires.
        app.update();
        let world = app.world_mut();
        let mut q = world.query_filtered::<(), With<DefaultModeUi>>();
        assert_eq!(q.iter(world).count(), 1, "exactly one DefaultModeUi");
        // Second update: run condition blocks re-spawn.
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

        // Record the DefaultShell entity.
        let shell_entity = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<DefaultShell>>()
                .single(world)
                .expect("DefaultShell spawned")
        };

        // Transition Default → Tmux.
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Tmux);
        app.update();

        // Transition Tmux → Default.
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Default);
        app.update();

        // The entity must still exist (not despawned).
        assert!(
            app.world_mut().get_entity(shell_entity).is_ok(),
            "DefaultShell entity survived Default → Tmux → Default round-trip"
        );

        // Only one DefaultShell must exist.
        let world = app.world_mut();
        let count = world
            .query_filtered::<(), With<DefaultShell>>()
            .iter(world)
            .count();
        assert_eq!(count, 1, "exactly one DefaultShell after round-trip");
    }

    #[test]
    fn default_mode_ui_hidden_in_tmux_mode() {
        let mut app = build_app(AppMode::Default);
        app.update();

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Tmux);
        app.update();

        let world = app.world_mut();
        let node = world
            .query_filtered::<&Node, With<DefaultModeUi>>()
            .single(world)
            .expect("DefaultModeUi present");
        assert_eq!(
            node.display,
            Display::None,
            "DefaultModeUi hidden in Tmux mode"
        );
    }

    #[test]
    fn default_mode_ui_visible_in_default_mode() {
        let mut app = build_app(AppMode::Default);
        app.update();

        // Transition to Tmux then back to Default.
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Tmux);
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Default);
        app.update();

        let world = app.world_mut();
        let node = world
            .query_filtered::<&Node, With<DefaultModeUi>>()
            .single(world)
            .expect("DefaultModeUi present");
        assert_eq!(
            node.display,
            Display::Flex,
            "DefaultModeUi visible in Default mode"
        );
    }
}
