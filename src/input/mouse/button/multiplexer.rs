//! Click-to-focus-pane: the one behavior the generic mouse-button dispatcher
//! does not already provide. Wheel-to-pane-under-cursor
//! (`crate::input::mouse::wheel`) and app-report/selection forwarding
//! (`crate::input::mouse::button`) already route through the topmost pane
//! surface; nothing moves the active window's `active_pane` on a click until
//! `focus_pane_on_click` does so here.

use crate::input::InputPhase;
use crate::input::mouse::{TerminalSurfaces, hit_candidates};
use crate::multiplexer::pane::MultiplexerPane;
use crate::multiplexer::window::{
    ActiveMultiplexerWindow, MultiplexerWindow, sync_keyboard_focus_to_active_pane,
};
use crate::surface::geometry::topmost_surface_at;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// Registers `focus_pane_on_click`, plus an idempotent
/// `add_message::<MouseButtonInput>()` so the system has a `Messages<T>`
/// resource to read when this plugin is exercised in isolation (mirroring
/// `ui::multiplexer::divider_handle::DividerHandlePlugin`).
pub(super) struct FocusPaneOnClickPlugin;

impl Plugin for FocusPaneOnClickPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<MouseButtonInput>().add_systems(
            Update,
            focus_pane_on_click
                .in_set(InputPhase::Dispatch)
                .before(sync_keyboard_focus_to_active_pane)
                .run_if(on_message::<MouseButtonInput>),
        );
    }
}

/// On a `MouseButton::Left` press, moves the active window's `active_pane` to
/// the `MultiplexerPane` under the cursor, when the hit entity is a
/// multiplexer pane and differs from the current `active_pane`.
///
/// Reuses the generic dispatcher's hit-test kernel
/// (`topmost_surface_at` + `hit_candidates`) rather than reinventing it, and
/// resolves the physical cursor the same way `wheel.rs`'s
/// `resolve_wheel_target` does. Writes `MultiplexerWindow` only on a real
/// change (an ordinary conditional `&mut` write, no `bypass_change_detection`
/// needed), so `Changed<MultiplexerWindow>` stays honest and still drives
/// `sync_keyboard_focus_to_active_pane` (and, in the `ui` crate module,
/// `sync_inactive_pane_style`). Ordered `.before(sync_keyboard_focus_to_active_pane)`
/// (and so before `InputPhase::FocusedKey`) so the focus move lands the same
/// frame; gated `.run_if(on_message::<MouseButtonInput>)`.
fn focus_pane_on_click(
    mut active_windows: Query<&mut MultiplexerWindow, With<ActiveMultiplexerWindow>>,
    mut buttons: MessageReader<MouseButtonInput>,
    terminals: TerminalSurfaces,
    panes: Query<(), With<MultiplexerPane>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(mut active_window) = active_windows.single_mut() else {
        buttons.clear();
        return;
    };
    let cursor_phys = windows
        .single()
        .ok()
        .filter(|window| window.focused)
        .and_then(|window| window.cursor_position().map(|c| c * window.scale_factor()));
    let Some(cursor_phys) = cursor_phys else {
        buttons.clear();
        return;
    };
    for ev in buttons.read() {
        if ev.button != MouseButton::Left || ev.state != ButtonState::Pressed {
            continue;
        }
        let Some(hit) = topmost_surface_at(cursor_phys, hit_candidates(&terminals)) else {
            continue;
        };
        if panes.contains(hit) && hit != active_window.active_pane {
            active_window.active_pane = hit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::mouse::test_support::set_phys_cursor;
    use crate::surface::OrzmaTerminal;
    use bevy::ui::{ComputedNode, ComputedStackIndex, UiGlobalTransform};
    use orzma_tty_engine::TerminalHandle;
    use orzma_tty_renderer::schema::TerminalGrid;

    /// A bare test `App` with `MinimalPlugins`, the `MouseButtonInput`
    /// message, and a focused 800x600 primary window — no systems registered
    /// yet, so callers can compose their own `focus_pane_on_click` wiring
    /// (see `click_on_active_pane_is_noop_for_change_detection`, which chains
    /// a probe after it).
    fn new_test_app() -> App {
        use bevy::window::WindowResolution;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<MouseButtonInput>();
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    fn make_focus_app() -> App {
        let mut app = new_test_app();
        app.add_systems(
            Update,
            focus_pane_on_click.run_if(on_message::<MouseButtonInput>),
        );
        app
    }

    /// Spawns a `MultiplexerPane` + `OrzmaTerminal` surface at the given
    /// window-space center `cx`, half-width (400x600) of the 800x600 test
    /// window, so a cursor in its half hit-tests to it.
    fn spawn_pane_surface(app: &mut App, window: Entity, cx: f32) -> Entity {
        app.world_mut()
            .spawn((
                OrzmaTerminal,
                MultiplexerPane { window },
                TerminalHandle::detached(100, 37),
                ComputedNode {
                    size: Vec2::new(400.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                ComputedStackIndex(0),
                UiGlobalTransform::from_xy(cx, 300.0),
                TerminalGrid {
                    cols: 100,
                    rows: 37,
                    ..default()
                },
            ))
            .id()
    }

    fn spawn_active_window_with_two_panes(app: &mut App) -> (Entity, Entity, Entity) {
        let window = app.world_mut().spawn_empty().id();
        let pane_a = spawn_pane_surface(app, window, 200.0);
        let pane_b = spawn_pane_surface(app, window, 600.0);
        app.world_mut().entity_mut(window).insert((
            MultiplexerWindow {
                index: 0,
                name: None,
                active_pane: pane_a,
            },
            ActiveMultiplexerWindow,
        ));
        (window, pane_a, pane_b)
    }

    fn write_left_press(app: &mut App) {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            });
    }

    #[test]
    fn click_focuses_pane_under_cursor() {
        let mut app = make_focus_app();
        let (window, pane_a, pane_b) = spawn_active_window_with_two_panes(&mut app);

        set_phys_cursor(&mut app, Vec2::new(600.0, 300.0));
        write_left_press(&mut app);
        app.update();
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_b,
            "a Left press over pane B must move active_pane to B"
        );

        set_phys_cursor(&mut app, Vec2::new(200.0, 300.0));
        write_left_press(&mut app);
        app.update();
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_a,
            "a Left press over pane A must move active_pane back to A"
        );
    }

    #[test]
    fn click_on_active_pane_is_noop_for_change_detection() {
        #[derive(Resource, Default)]
        struct RunCount(u32);

        fn probe(mut count: ResMut<RunCount>) {
            count.0 += 1;
        }

        fn window_changed(
            windows: Query<(), (With<ActiveMultiplexerWindow>, Changed<MultiplexerWindow>)>,
        ) -> bool {
            !windows.is_empty()
        }

        let mut app = new_test_app();
        app.init_resource::<RunCount>().add_systems(
            Update,
            (
                focus_pane_on_click.run_if(on_message::<MouseButtonInput>),
                probe.run_if(window_changed),
            )
                .chain(),
        );
        let (_window, _pane_a, _pane_b) = spawn_active_window_with_two_panes(&mut app);

        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "a freshly-inserted MultiplexerWindow counts as changed"
        );

        set_phys_cursor(&mut app, Vec2::new(200.0, 300.0));
        write_left_press(&mut app);
        app.update();
        assert_eq!(
            app.world().resource::<RunCount>().0,
            1,
            "clicking the already-active pane must not spuriously trip Changed<MultiplexerWindow>"
        );
    }
}
