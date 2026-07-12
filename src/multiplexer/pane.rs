//! Multiplexer pane domain: the pane component and its cwd cache.

pub(crate) mod spawn;

use bevy::prelude::*;
use std::path::PathBuf;

/// A multiplexer pane: a terminal surface owned by a window.
#[derive(Component)]
pub(crate) struct MultiplexerPane {
    /// The window this pane belongs to.
    pub window: Entity,
}

/// The pane's last OSC-7 reported cwd, used to seed a sibling's cwd on split.
#[derive(Component, Default)]
pub(crate) struct PaneCwd(pub Option<PathBuf>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_component_roundtrips() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let window = app.world_mut().spawn_empty().id();
        let pane = app
            .world_mut()
            .spawn((MultiplexerPane { window }, PaneCwd::default()))
            .id();
        let p = app.world().entity(pane).get::<MultiplexerPane>().unwrap();
        assert_eq!(p.window, window);
        let cwd = app.world().entity(pane).get::<PaneCwd>().unwrap();
        assert_eq!(cwd.0, None);
    }
}
