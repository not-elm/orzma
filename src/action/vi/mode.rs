//! Vi mode state: a pure marker component whose presence on a surface
//! entity means vi mode is active. Entering and exiting request a selection
//! clear and a `RequestTtyViMode` switch on the underlying tty.

use crate::input::focus::{KeyboardDisabled, TerminalMouseDisabled};
use bevy::app::{App, Plugin};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::observer::On;
use bevy::ecs::query::With;
use bevy::ecs::system::{Commands, Query, ResMut};
use bevy_cef::prelude::FocusedWebview;
use bevy_orzmux::prelude::{OrzmuxPane, RequestTtySelectionClear, RequestTtyViMode, ViModeSwitch};

/// Adds vi-mode enter and exit handling.
pub(super) struct ViModePlugin;

impl Plugin for ViModePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(handle_enter_vi_mode_request)
            .add_observer(handle_exit_vi_mode);
    }
}

/// Marker: presence on a Surface entity means "vi mode is active".
#[derive(Component, Debug, Default)]
pub struct ViModeState;

/// Request to enter vi mode on a specific Surface entity.
#[derive(EntityEvent, Debug)]
pub struct EnterViModeActionEvent {
    /// The Surface entity to enter vi mode on.
    pub entity: Entity,
}

/// Requests exiting vi mode on `entity`: clears the selection, switches the
/// tty out of vi mode, and removes `ViModeState`.
#[derive(EntityEvent, Debug)]
pub struct ExitViMode {
    /// The Surface entity to exit vi mode on.
    pub entity: Entity,
}

/// Inserts `ViModeState` on the target entity, releases any focused inline
/// webview, and requests a selection clear followed by the vi-mode enter
/// switch. An entity that carries no `OrzmuxPane` is left untouched.
fn handle_enter_vi_mode_request(
    ev: On<EnterViModeActionEvent>,
    mut commands: Commands,
    mut focused_webview: ResMut<FocusedWebview>,
    terminals: Query<(), With<OrzmuxPane>>,
) {
    if terminals.get(ev.entity).is_err() {
        return;
    }
    // NOTE: vi mode gates both the terminal and the webview mouse paths, so a
    // webview left focused here can no longer be dismissed by an off-rect
    // click and would keep swallowing the keyboard for the whole session.
    if focused_webview.0.is_some() {
        focused_webview.0 = None;
    }
    // NOTE: clear before entering vi mode — once a selection-reading
    // capability lands, the v/V toggle predicate must not misread a
    // leftover mouse-drag selection as an already-started vi selection.
    commands.trigger(RequestTtySelectionClear {
        terminal: ev.entity,
    });
    commands.trigger(RequestTtyViMode {
        terminal: ev.entity,
        switch: ViModeSwitch::Enter,
    });
    commands
        .entity(ev.entity)
        .insert((ViModeState, KeyboardDisabled, TerminalMouseDisabled));
}

/// Removes `ViModeState`, and requests a selection clear followed by the
/// vi-mode exit switch, which snaps the viewport to the live tail.
fn handle_exit_vi_mode(
    ev: On<ExitViMode>,
    mut commands: Commands,
    terminals: Query<(), With<OrzmuxPane>>,
) {
    if terminals.get(ev.entity).is_err() {
        return;
    }
    commands.trigger(RequestTtySelectionClear {
        terminal: ev.entity,
    });
    commands.trigger(RequestTtyViMode {
        terminal: ev.entity,
        switch: ViModeSwitch::Exit,
    });
    commands
        .entity(ev.entity)
        .remove::<ViModeState>()
        .remove::<KeyboardDisabled>()
        .remove::<TerminalMouseDisabled>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::ecs::resource::Resource;
    use bevy::ecs::system::ResMut;
    use bevy::prelude::{ChildOf, MinimalPlugins};
    use bevy_orzma_webview::Webview;
    use orzma_vt::prelude::InstanceId;
    use orzmux::prelude::PaneId;

    fn spawn_terminal_entity(app: &mut App) -> Entity {
        app.world_mut().spawn(OrzmuxPane(PaneId(1))).id()
    }

    #[derive(Debug, PartialEq)]
    enum SeenRequest {
        Clear(Entity),
        Switch(Entity, ViModeSwitch),
    }

    #[derive(Resource, Default)]
    struct SeenRequests(Vec<SeenRequest>);

    fn capture_requests(app: &mut App) {
        app.init_resource::<SeenRequests>()
            .add_observer(
                |ev: On<RequestTtySelectionClear>, mut seen: ResMut<SeenRequests>| {
                    seen.0.push(SeenRequest::Clear(ev.terminal));
                },
            )
            .add_observer(|ev: On<RequestTtyViMode>, mut seen: ResMut<SeenRequests>| {
                seen.0.push(SeenRequest::Switch(ev.terminal, ev.switch));
            });
    }

    /// Asserts that entering vi mode inserts `ViModeState` and requests a
    /// selection clear before the vi-mode-enter switch.
    ///
    /// Case: the user presses the vi-mode shortcut on a terminal that may
    /// carry a leftover mouse-drag selection from before entry.
    #[test]
    fn enter_observer_inserts_vi_mode_state_and_clears_selection_first() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_observer(handle_enter_vi_mode_request);
        capture_requests(&mut app);
        let entity = spawn_terminal_entity(&mut app);

        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();

        assert!(app.world().get::<ViModeState>(entity).is_some());
        assert_eq!(
            app.world().resource::<SeenRequests>().0,
            vec![
                SeenRequest::Clear(entity),
                SeenRequest::Switch(entity, ViModeSwitch::Enter),
            ]
        );
    }

    /// Asserts that entering vi mode on an entity without an `OrzmuxPane`
    /// neither inserts `ViModeState` nor fires any request.
    ///
    /// Case: a stray `EnterViModeActionEvent` aimed at an entity that never
    /// had an `OrzmuxPane`, or whose pane was already torn down.
    #[test]
    fn enter_request_on_a_bare_entity_is_a_no_op() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_observer(handle_enter_vi_mode_request);
        capture_requests(&mut app);
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();

        assert!(app.world().get::<ViModeState>(entity).is_none());
        assert!(app.world().resource::<SeenRequests>().0.is_empty());
    }

    /// Asserts that entering vi mode marks the entity `KeyboardDisabled`
    /// and `TerminalMouseDisabled`.
    ///
    /// Case: the user enters vi mode on the focused terminal.
    #[test]
    fn enter_observer_disables_keyboard_and_mouse() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_observer(handle_enter_vi_mode_request);
        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        assert!(app.world().get::<KeyboardDisabled>(entity).is_some());
        assert!(app.world().get::<TerminalMouseDisabled>(entity).is_some());
    }

    /// Asserts that entering vi mode clears `FocusedWebview`.
    ///
    /// Case: the user has clicked into a page mounted in a pane and then
    /// enters vi mode to scroll back through that pane's output.
    #[test]
    fn enter_observer_releases_a_focused_webview() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_observer(handle_enter_vi_mode_request);
        let entity = spawn_terminal_entity(&mut app);
        let child = app
            .world_mut()
            .spawn((
                ChildOf(entity),
                Webview {
                    handle: "h1".into(),
                    instance: InstanceId(1),
                    slot: 0,
                    rows: 10,
                    cols: 40,
                },
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "a page left focused in vi mode takes the keyboard with no mouse route back"
        );
    }

    /// Asserts that exiting vi mode removes `KeyboardDisabled` and
    /// `TerminalMouseDisabled` again.
    ///
    /// Case: the user leaves vi mode with `Esc`.
    #[test]
    fn exit_observer_reenables_keyboard_and_mouse() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_observer(handle_enter_vi_mode_request)
            .add_observer(handle_exit_vi_mode);
        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        app.world_mut().trigger(ExitViMode { entity });
        app.update();
        assert!(app.world().get::<KeyboardDisabled>(entity).is_none());
        assert!(app.world().get::<TerminalMouseDisabled>(entity).is_none());
    }

    /// Asserts that exiting vi mode removes `ViModeState` and requests a
    /// selection clear before the vi-mode-exit switch.
    ///
    /// Case: the user presses `Esc` to leave vi mode, possibly with an
    /// active vi selection.
    #[test]
    fn exit_observer_removes_vi_mode_state_and_clears_selection_first() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_observer(handle_enter_vi_mode_request)
            .add_observer(handle_exit_vi_mode);
        capture_requests(&mut app);

        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();

        app.world_mut().trigger(ExitViMode { entity });
        app.update();

        assert!(app.world().get::<ViModeState>(entity).is_none());
        assert_eq!(
            app.world().resource::<SeenRequests>().0,
            vec![
                SeenRequest::Clear(entity),
                SeenRequest::Switch(entity, ViModeSwitch::Enter),
                SeenRequest::Clear(entity),
                SeenRequest::Switch(entity, ViModeSwitch::Exit),
            ],
            "each mode switch is preceded by its own selection clear"
        );
    }
}
