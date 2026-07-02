//! `RenameWindowRequest` — opens the ozmux rename prompt pre-filled with the
//! target window's current name.

use crate::mode::tmux::rename_prompt::{RenamePrompt, RenameSubject};
use bevy::prelude::*;
use ozmux_tmux::TmuxWindow;

/// Opens the rename prompt for the tmux window owning `entity`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct RenameWindowRequest {
    /// The window entity to rename.
    #[event_target]
    pub entity: Entity,
}

/// Registers the `RenameWindowRequest` apply observer.
pub(super) struct RenameWindowPlugin;

impl Plugin for RenameWindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_rename_window);
    }
}

fn on_rename_window(
    ev: On<RenameWindowRequest>,
    mut commands: Commands,
    windows: Query<&TmuxWindow>,
) {
    let Ok(window) = windows.get(ev.entity) else {
        return;
    };
    commands.insert_resource(RenamePrompt::new(RenameSubject::Window {
        id: window.id,
        current_name: window.name.clone(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control_parser::WindowId;

    #[test]
    fn rename_window_opens_prompt() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_rename_window);
        let target = app
            .world_mut()
            .spawn(TmuxWindow {
                id: WindowId(2),
                index: 0,
                name: "editor".into(),
            })
            .id();
        app.world_mut()
            .trigger(RenameWindowRequest { entity: target });
        app.update();
        assert!(app.world().contains_resource::<RenamePrompt>());
    }
}
