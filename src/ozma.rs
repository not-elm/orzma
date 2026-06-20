//! AppMode state enum and the Ozma single-terminal lifecycle plugin.

use bevy::prelude::*;
use ozma_terminal::{
    KeyboardFocused, OzmaSpawnOptions, OzmaTerminal, OzmaTerminalBundle, OzmaTerminalConfig,
};

/// Application mode. `Ozma` is the default (single PTY, no tmux).
/// `Ozmux` activates the tmux multiplexer backend.
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) enum AppMode {
    /// Single PTY terminal, Alacritty VT emulation, no tmux.
    #[default]
    Ozma,
    /// tmux backend, multiplexer layout.
    Ozmux,
}

/// Bevy plugin that registers the `AppMode::Ozma` spawn/despawn lifecycle.
///
/// Spawns one `OzmaTerminal` entity (marked `KeyboardFocused`, the keyboard
/// target) on `OnEnter(AppMode::Ozma)` and despawns it on `OnExit(AppMode::Ozma)`. Requires `AppMode` to be
/// inserted via `App::insert_state` before this plugin runs, and
/// `OzmaTerminalPlugin` must be added first (it inserts `OzmaTerminalConfig`
/// that `spawn_terminal` reads).
pub(crate) struct OzmaModePlugin;

impl Plugin for OzmaModePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppMode::Ozma), spawn_terminal)
            .add_systems(OnExit(AppMode::Ozma), despawn_terminal);
    }
}

fn spawn_terminal(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    config: Res<OzmaTerminalConfig>,
) {
    match OzmaTerminalBundle::spawn(OzmaSpawnOptions {
        shell: config.shell.clone(),
        ..default()
    }) {
        Ok(bundle) => {
            commands.spawn((bundle, KeyboardFocused));
        }
        Err(e) => {
            tracing::error!(?e, "failed to spawn ozma terminal");
            exit.write(AppExit::Success);
        }
    }
}

fn despawn_terminal(mut commands: Commands, terminals: Query<Entity, With<OzmaTerminal>>) {
    for entity in terminals.iter() {
        commands.entity(entity).despawn();
    }
}
