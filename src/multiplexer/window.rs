//! Multiplexer window (tab) domain: the window component, the active-window
//! marker, and the ECS wrapper around the pure layout tree.

use crate::input::InputPhase;
use crate::input::focus::KeyboardFocused;
use crate::multiplexer::bootstrap::WindowContainer;
use crate::multiplexer::layout::{MultiplexerLayout, PaneRect};
use crate::multiplexer::pane::layout::PANE_GAP_PX;
use crate::multiplexer::request::SelectPaneRequest;
use bevy::prelude::*;
use bevy::ui::ComputedNode;

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

/// Registers the `SelectPaneRequest` message, `select_pane`, and
/// `sync_keyboard_focus_to_active_pane`.
///
/// Registers `SelectPaneRequest` here (not only in the shortcut-applier
/// plugin that writes it) so `select_pane`'s `on_message::<SelectPaneRequest>`
/// run condition has a `Messages<SelectPaneRequest>` resource to read even
/// when `WindowPlugin` is exercised without the input plugins (as bootstrap
/// tests do); `add_message` is idempotent, so this is a no-op when the
/// shortcut-applier plugin already registered it.
pub(in crate::multiplexer) struct WindowPlugin;

impl Plugin for WindowPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SelectPaneRequest>().add_systems(
            Update,
            (
                select_pane
                    .before(sync_keyboard_focus_to_active_pane)
                    .run_if(on_message::<SelectPaneRequest>),
                sync_keyboard_focus_to_active_pane
                    .before(InputPhase::FocusedKey)
                    .run_if(active_pane_changed),
            ),
        );
    }
}

/// Moves `KeyboardFocused` onto the active window's `active_pane`: removes it
/// from whatever pane currently holds it, then inserts it on the new target.
/// Gated by `active_pane_changed` so it only writes on a real change; ordered
/// `.before(InputPhase::FocusedKey)` so the resolved key effects' `focused`
/// reflects the active pane the same frame it changes.
///
/// `pub(crate)` (not private): `crate::input::mouse::button::multiplexer`'s
/// `focus_pane_on_click` names this function in a `.before(...)` ordering
/// constraint, so its own `active_pane` write lands before this sync runs —
/// otherwise the two systems' conflicting `MultiplexerWindow` access would be
/// ordered arbitrarily and focus could lag a frame behind a click.
pub(crate) fn sync_keyboard_focus_to_active_pane(
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

/// Moves the active window's `active_pane` to its neighbor in a
/// `SelectPaneRequest`'s direction. Un-zooms the window first so the
/// neighbor lookup runs against the full split layout rather than the
/// zoomed pane's full-area rect. `active_pane` is written only when the
/// computed neighbor differs from the current one, so a request at the
/// window edge (no neighbor in `dir`) is a no-op that never spuriously
/// trips `Changed<MultiplexerWindow>`.
///
/// Ordered `.before(sync_keyboard_focus_to_active_pane)` so a focus change
/// lands the same frame `active_pane` moves; gated
/// `.run_if(on_message::<SelectPaneRequest>)`.
fn select_pane(
    mut requests: MessageReader<SelectPaneRequest>,
    mut windows: Query<
        (Entity, &mut MultiplexerWindow, &mut MultiplexerLayoutComp),
        With<ActiveMultiplexerWindow>,
    >,
    containers: Query<(&WindowContainer, &ComputedNode)>,
) {
    let Ok((window, mut window_state, mut layout)) = windows.single_mut() else {
        return;
    };
    let Some((_, computed)) = containers.iter().find(|(c, _)| c.window == window) else {
        return;
    };
    let area = PaneRect {
        x: 0.0,
        y: 0.0,
        w: computed.size.x,
        h: computed.size.y,
    };
    for msg in requests.read() {
        if layout.0.zoomed().is_some() {
            layout.0.set_zoom(None);
        }
        if let Some(next) = layout
            .0
            .neighbor(window_state.active_pane, msg.dir, area, PANE_GAP_PX)
            && next != window_state.active_pane
        {
            window_state.active_pane = next;
        }
    }
}

/// Whether the active window's `MultiplexerWindow` (and so, approximately,
/// its `active_pane`) changed this frame.
fn active_pane_changed(
    windows: Query<(), (With<ActiveMultiplexerWindow>, Changed<MultiplexerWindow>)>,
) -> bool {
    !windows.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::layout::SplitAxis;
    use orzma_configs::shortcuts::PaneDirection;

    /// Spawns an active window with a 2-leaf vertical split (`pane_a` left,
    /// `pane_b` right, `active_pane` starting on `pane_a`), plus a
    /// `WindowContainer` carrying a `ComputedNode` of `area_size` so
    /// `select_pane` can resolve the workspace area. Returns
    /// `(window, pane_a, pane_b)`.
    fn spawn_two_pane_vertical_window(app: &mut App, area_size: Vec2) -> (Entity, Entity, Entity) {
        let world = app.world_mut();
        let pane_a = world.spawn_empty().id();
        let pane_b = world.spawn_empty().id();
        let mut layout = MultiplexerLayout::new(pane_a);
        layout.split(pane_a, pane_b, SplitAxis::Vertical);
        let window = world
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane_a,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(layout),
            ))
            .id();
        world.spawn((
            WindowContainer { window },
            ComputedNode {
                size: area_size,
                ..ComputedNode::DEFAULT
            },
        ));
        (window, pane_a, pane_b)
    }

    #[test]
    fn select_pane_right_moves_active_pane_to_right_neighbor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .add_systems(Update, select_pane);
        let (window, _pane_a, pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Right,
        });
        app.update();

        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_b,
            "SelectPaneRequest {{ Right }} from the left pane must move focus to its right neighbor"
        );
    }

    #[test]
    fn select_pane_left_moves_active_pane_to_left_neighbor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .add_systems(Update, select_pane);
        let (window, pane_a, pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));
        app.world_mut()
            .get_mut::<MultiplexerWindow>(window)
            .unwrap()
            .active_pane = pane_b;

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Left,
        });
        app.update();

        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_a,
            "SelectPaneRequest {{ Left }} from the right pane must move focus to its left neighbor"
        );
    }

    #[test]
    fn select_pane_with_no_neighbor_on_that_axis_is_noop() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .add_systems(Update, select_pane);
        let (window, pane_a, _pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Up,
        });
        app.update();
        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Down,
        });
        app.update();

        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_a,
            "a vertical split has no neighbor above/below; Up/Down must be a no-op"
        );
    }

    #[test]
    fn select_pane_unzooms_before_computing_neighbor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .add_systems(Update, select_pane);
        let (window, pane_a, pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));
        app.world_mut()
            .get_mut::<MultiplexerLayoutComp>(window)
            .unwrap()
            .0
            .set_zoom(Some(pane_a));

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Right,
        });
        app.update();

        assert_eq!(
            app.world()
                .get::<MultiplexerLayoutComp>(window)
                .unwrap()
                .0
                .zoomed(),
            None,
            "select_pane must clear zoom before moving focus"
        );
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_b,
            "after un-zooming, Right must move focus to the right neighbor"
        );
    }

    #[test]
    fn select_pane_edge_request_does_not_trigger_change_detection() {
        #[derive(Resource, Default)]
        struct RunCount(u32);

        fn probe(mut count: ResMut<RunCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<SelectPaneRequest>()
            .init_resource::<RunCount>()
            .add_systems(
                Update,
                (select_pane, probe.run_if(active_pane_changed)).chain(),
            );
        let (window, pane_a, _pane_b) =
            spawn_two_pane_vertical_window(&mut app, Vec2::new(800.0, 600.0));

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a freshly-inserted MultiplexerWindow counts as changed"
        );

        app.world_mut().write_message(SelectPaneRequest {
            dir: PaneDirection::Up,
        });
        app.update();

        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "an edge SelectPaneRequest with no neighbor must not spuriously trip Changed<MultiplexerWindow>"
        );
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_a
        );
    }

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
