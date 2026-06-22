//! AppMode state enum and the Default-mode UI subtree lifecycle plugin.

use crate::ui::UiRoot;
use bevy::prelude::*;
use ozma_terminal::{KeyboardFocused, OzmaSpawnOptions, OzmaTerminalBundle, OzmaTerminalConfig};

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

/// Root of the Default-mode UI subtree, mounted under `UiRoot` while in
/// `AppMode::Default`. Carries `DespawnOnExit(AppMode::Default)`, so leaving
/// Default mode removes the subtree (including its child terminal).
#[derive(Component)]
struct DefaultModeUi;

/// Bevy plugin that ensures the Default-mode UI subtree (a single
/// `OzmaTerminal` under `DefaultModeUi`) exists while in `AppMode::Default`.
///
/// The subtree is built by `ensure_default_mode_ui`, gated
/// `run_if(in_state(AppMode::Default).and(not(any_with_component::<DefaultModeUi>)))`:
/// it runs in `Update` (always after `Startup` spawns `UiRoot`) and re-fires on
/// re-entry once the previous subtree is gone. Teardown is `DespawnOnExit`.
/// `OzmaTerminalPlugin` must be added first (it inserts the `OzmaTerminalConfig`
/// this reads).
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
            DespawnOnExit(AppMode::Default),
            DefaultModeUi,
            ChildOf(ui_root),
        ))
        .id();
    match OzmaTerminalBundle::spawn(OzmaSpawnOptions {
        shell: config.shell.clone(),
        ..default()
    }) {
        Ok(bundle) => {
            commands.spawn((bundle, KeyboardFocused, ChildOf(mode_ui)));
        }
        Err(e) => {
            tracing::error!(?e, "failed to spawn ozma terminal");
            exit.write(AppExit::Success);
        }
    }
}
