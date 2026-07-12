//! Multiplexer window (tab) domain: the window component, the active-window
//! marker, and the ECS wrapper around the pure layout tree.

use crate::input::InputPhase;
use crate::input::focus::KeyboardFocused;
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

/// Registers `sync_keyboard_focus_to_active_pane`.
pub(in crate::multiplexer) struct WindowPlugin;

impl Plugin for WindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            sync_keyboard_focus_to_active_pane
                .before(InputPhase::FocusedKey)
                .run_if(active_pane_changed),
        );
    }
}

/// Whether the active window's `MultiplexerWindow` (and so, approximately,
/// its `active_pane`) changed this frame.
fn active_pane_changed(
    windows: Query<(), (With<ActiveMultiplexerWindow>, Changed<MultiplexerWindow>)>,
) -> bool {
    !windows.is_empty()
}

/// Moves `KeyboardFocused` onto the active window's `active_pane`: removes it
/// from whatever pane currently holds it, then inserts it on the new target.
/// Gated by `active_pane_changed` so it only writes on a real change; ordered
/// `.before(InputPhase::FocusedKey)` so the resolved key effects' `focused`
/// reflects the active pane the same frame it changes.
fn sync_keyboard_focus_to_active_pane(
    mut commands: Commands,
    active_windows: Query<&MultiplexerWindow, With<ActiveMultiplexerWindow>>,
    focused: Query<Entity, With<KeyboardFocused>>,
) {
    let Ok(window) = active_windows.single() else {
        return;
    };
    let target = window.active_pane;
    for current in focused.iter() {
        if current != target {
            commands.entity(current).remove::<KeyboardFocused>();
        }
    }
    if !focused.contains(target) {
        commands.entity(target).insert(KeyboardFocused);
    }
}

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

    #[test]
    fn active_pane_change_moves_keyboard_focus() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(
            Update,
            sync_keyboard_focus_to_active_pane.run_if(active_pane_changed),
        );

        let pane_a = app.world_mut().spawn(KeyboardFocused).id();
        let pane_b = app.world_mut().spawn_empty().id();
        let window = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane_a,
                },
                ActiveMultiplexerWindow,
            ))
            .id();

        app.update();
        assert!(
            app.world().entity(pane_a).contains::<KeyboardFocused>(),
            "the active window's active_pane already carries KeyboardFocused"
        );
        assert!(!app.world().entity(pane_b).contains::<KeyboardFocused>());

        app.world_mut()
            .entity_mut(window)
            .get_mut::<MultiplexerWindow>()
            .unwrap()
            .active_pane = pane_b;
        app.update();

        assert!(
            !app.world().entity(pane_a).contains::<KeyboardFocused>(),
            "the former active_pane loses focus"
        );
        assert!(
            app.world().entity(pane_b).contains::<KeyboardFocused>(),
            "the new active_pane gains focus"
        );
    }

    #[test]
    fn active_pane_changed_gates_on_real_change_only() {
        #[derive(Resource, Default)]
        struct RunCount(u32);

        fn probe(mut count: ResMut<RunCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<RunCount>();
        app.add_systems(Update, probe.run_if(active_pane_changed));

        let pane = app.world_mut().spawn_empty().id();
        let window = app
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

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a freshly-inserted MultiplexerWindow counts as changed"
        );

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "an untouched window must not re-trigger the gate"
        );

        app.world_mut()
            .entity_mut(window)
            .get_mut::<MultiplexerWindow>()
            .unwrap()
            .active_pane = pane;
        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            2,
            "mutating the window must re-trigger the gate"
        );
    }
}
