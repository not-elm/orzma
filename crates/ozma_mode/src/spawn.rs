//! Terminal spawn and despawn for Ozma mode.

use bevy::prelude::*;

/// Marker component identifying the single Ozma terminal entity.
#[derive(Component)]
pub(crate) struct OzmaTerminal;

/// Spawns the Ozma PTY terminal on mode entry.
pub(crate) fn spawn_terminal(_commands: Commands) {
    // TODO: implement in Task 3
}

/// Despawns the Ozma terminal on mode exit.
pub(crate) fn despawn_terminal(
    mut commands: Commands,
    terminal_q: Query<Entity, With<OzmaTerminal>>,
) {
    for entity in terminal_q.iter() {
        commands.entity(entity).despawn();
    }
}
