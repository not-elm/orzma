//! AppMode state enum and the Default-mode UI subtree lifecycle plugin.

use crate::ui::UiRoot;
use bevy::prelude::*;
use bevy::ui::Val;
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
/// `run_if(in_state(Default).and(no_default_mode_ui))` so it spawns once `UiRoot`
/// exists (boot-safe: `Update` always runs after `Startup`) and re-fires on
/// re-entry. Teardown is `DespawnOnExit`. `OzmaTerminalPlugin` must be added
/// first (it inserts the `OzmaTerminalConfig` this reads).
pub(crate) struct DefaultModePlugin;

impl Plugin for DefaultModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            ensure_default_mode_ui.run_if(in_state(AppMode::Default).and(no_default_mode_ui)),
        );
    }
}

fn no_default_mode_ui(roots: Query<(), With<DefaultModeUi>>) -> bool {
    roots.is_empty()
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
    let bundle = match OzmaTerminalBundle::spawn(OzmaSpawnOptions {
        shell: config.shell.clone(),
        ..default()
    }) {
        Ok(bundle) => bundle,
        Err(e) => {
            tracing::error!(?e, "failed to spawn ozma terminal");
            exit.write(AppExit::Success);
            return;
        }
    };
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
    commands.spawn((bundle, KeyboardFocused, ChildOf(mode_ui)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn no_default_mode_ui_is_true_until_one_exists() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        assert!(app.world_mut().run_system_once(no_default_mode_ui).unwrap());
        app.world_mut().spawn(DefaultModeUi);
        assert!(!app.world_mut().run_system_once(no_default_mode_ui).unwrap());
    }
}
