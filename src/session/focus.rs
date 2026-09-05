//! Active-pane focus mirror: the backend owns the active pane, and this
//! module mirrors `MuxActivePaneChanged` into the host's `KeyboardFocused`
//! marker so keyboard dispatch keeps following it.

use crate::input::focus::KeyboardFocused;
use bevy::prelude::*;
use bevy_orzma_mux::prelude::MuxActivePaneChanged;

/// Registers the active-pane focus mirror observer.
pub(super) struct FocusPlugin;

impl Plugin for FocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_active_pane_changed);
    }
}

fn on_active_pane_changed(ev: On<MuxActivePaneChanged>, mut commands: Commands) {
    if let Some(previous) = ev.previous {
        commands.entity(previous).remove::<KeyboardFocused>();
    }
    if let Some(current) = ev.current {
        commands.entity(current).insert(KeyboardFocused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that an active-pane change moves `KeyboardFocused` from
    /// the previous pane to the current one.
    ///
    /// Case: the user splits a pane and the backend hands focus to the
    /// new one.
    #[test]
    fn active_pane_change_moves_keyboard_focus() {
        let mut app = App::new();
        app.add_observer(on_active_pane_changed);
        let previous = app.world_mut().spawn(KeyboardFocused).id();
        let current = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(MuxActivePaneChanged {
            previous: Some(previous),
            current: Some(current),
        });
        app.world_mut().flush();

        assert!(!app.world().entity(previous).contains::<KeyboardFocused>());
        assert!(app.world().entity(current).contains::<KeyboardFocused>());
    }

    /// Asserts that a `None` previous or current is a no-op on that side.
    ///
    /// Case: the very first pane opens (`previous: None`) or the last
    /// pane closes (`current: None`).
    #[test]
    fn a_none_side_is_left_untouched() {
        let mut app = App::new();
        app.add_observer(on_active_pane_changed);
        let current = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(MuxActivePaneChanged {
            previous: None,
            current: Some(current),
        });
        app.world_mut().flush();
        assert!(app.world().entity(current).contains::<KeyboardFocused>());

        app.world_mut().trigger(MuxActivePaneChanged {
            previous: Some(current),
            current: None,
        });
        app.world_mut().flush();
        assert!(!app.world().entity(current).contains::<KeyboardFocused>());
    }
}
