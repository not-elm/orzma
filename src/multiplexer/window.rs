//! Multiplexer window (tab) domain: the window component, the active-window
//! marker, and the ECS wrapper around the pure layout tree.

use crate::multiplexer::layout::MultiplexerLayout;
use bevy::prelude::*;

/// A multiplexer window (tab). One is active at a time (see `ActiveMultiplexerWindow`).
#[derive(Component)]
pub(crate) struct MultiplexerWindow {
    /// Window-bar order and `select_window_N` target.
    pub index: u32,
    /// User-assigned name; `None` displays the active pane's `TerminalTitle`.
    pub name: Option<String>,
    /// The focused pane in this window, restored on switch.
    pub active_pane: Entity,
}

/// Marks the single active window whose `active_pane` drives keyboard focus.
#[derive(Component)]
pub(crate) struct ActiveMultiplexerWindow;

/// ECS wrapper around the Bevy-free layout tree, kept a newtype so
/// `layout.rs` has no Bevy dependency beyond the `Entity` id.
#[derive(Component)]
pub(crate) struct MultiplexerLayoutComp(pub MultiplexerLayout);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_component_roundtrips() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let pane = app.world_mut().spawn_empty().id();
        let win = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane,
                },
                ActiveMultiplexerWindow,
            ))
            .id();
        let w = app.world().entity(win).get::<MultiplexerWindow>().unwrap();
        assert_eq!(w.index, 0);
        assert_eq!(w.active_pane, pane);
        assert!(
            app.world()
                .entity(win)
                .contains::<ActiveMultiplexerWindow>()
        );
    }
}
