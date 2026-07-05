//! `OrzmaBootstrapPlugin` registers the `insert_initial_cursor_icon` Startup
//! system, which inserts the default arrow cursor on the primary window so
//! the hyperlink hover system can mutate it without first inserting it.

use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

/// Bevy Plugin that registers the `insert_initial_cursor_icon` system in the
/// `Startup` schedule.
pub struct OrzmaBootstrapPlugin;

impl Plugin for OrzmaBootstrapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, insert_initial_cursor_icon);
    }
}

/// Inserts an initial `CursorIcon::System(SystemCursorIcon::Default)`
/// (the arrow) on the primary window so the hover system in
/// `src/input/hyperlink.rs` can mutate the component without first
/// having to insert it. The arrow is the default for non-terminal
/// regions; the hover system narrows it to the I-beam over terminal text.
fn insert_initial_cursor_icon(
    mut commands: Commands,
    windows: Query<Entity, (With<PrimaryWindow>, Without<CursorIcon>)>,
) {
    for window in windows.iter() {
        commands
            .entity(window)
            .insert(CursorIcon::System(SystemCursorIcon::Default));
    }
}
