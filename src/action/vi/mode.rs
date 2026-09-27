//! Vi mode state: a pure marker component whose presence on a surface
//! entity means vi mode is active. Entering and exiting request a
//! `RequestTtyViMode` switch on the underlying tty, which also drops the
//! selection.

use crate::input::focus::KeyboardDisabled;
use bevy::app::{App, Plugin};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::observer::On;
use bevy::ecs::query::With;
use bevy::ecs::system::{Commands, Query, Res};
use bevy_cef::prelude::FocusedWebview;
use bevy_orzma_webview::RequestWebviewFocus;
use bevy_orzmux::prelude::{OrzmuxPane, RequestTtyViMode, ViModeSwitch};

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

/// Requests exiting vi mode on `entity`: switches the tty out of vi mode
/// and removes `ViModeState`.
#[derive(EntityEvent, Debug)]
pub struct ExitViMode {
    /// The Surface entity to exit vi mode on.
    pub entity: Entity,
}

/// Inserts `ViModeState` on the target entity, asks for any focused inline
/// webview to be released, and requests the vi-mode enter switch. An entity
/// that carries no `OrzmuxPane` is left untouched.
fn handle_enter_vi_mode_request(
    ev: On<EnterViModeActionEvent>,
    mut commands: Commands,
    focused_webview: Res<FocusedWebview>,
    terminals: Query<(), With<OrzmuxPane>>,
) {
    if terminals.get(ev.entity).is_err() {
        return;
    }
    // NOTE: vi mode gates the webview mouse path, so a webview left focused
    // here can no longer be dismissed by an off-rect click and would keep
    // swallowing the keyboard for the whole session.
    if focused_webview.0.is_some() {
        commands.trigger(RequestWebviewFocus::new(None));
    }
    commands.trigger(RequestTtyViMode {
        terminal: ev.entity,
        switch: ViModeSwitch::Enter,
    });
    commands
        .entity(ev.entity)
        .insert((ViModeState, KeyboardDisabled));
}

/// Removes `ViModeState` and requests the vi-mode exit switch, which drops
/// the selection and snaps the viewport to the live tail.
fn handle_exit_vi_mode(
    ev: On<ExitViMode>,
    mut commands: Commands,
    terminals: Query<(), With<OrzmuxPane>>,
) {
    if terminals.get(ev.entity).is_err() {
        return;
    }
    commands.trigger(RequestTtyViMode {
        terminal: ev.entity,
        switch: ViModeSwitch::Exit,
    });
    commands
        .entity(ev.entity)
        .remove::<ViModeState>()
        .remove::<KeyboardDisabled>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::focus::TerminalMouseDisabled;
    use bevy::app::App;
    use bevy::ecs::resource::Resource;
    use bevy::ecs::system::ResMut;
    use bevy::prelude::{ChildOf, MinimalPlugins};
    use bevy_orzma_webview::Webview;
    use bevy_orzmux::prelude::RequestTtySelectionClear;
    use orzma_vt::prelude::InstanceId;
    use orzma_webview_host::prelude::MountId;
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

    /// Asserts that entering vi mode inserts `ViModeState` and requests only
    /// the vi-mode-enter switch.
    ///
    /// Case: the user presses the vi-mode shortcut on a terminal.
    #[test]
    fn enter_observer_inserts_vi_mode_state_and_requests_the_switch() {
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
            vec![SeenRequest::Switch(entity, ViModeSwitch::Enter)]
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

    /// Asserts that entering vi mode disables the keyboard but leaves the
    /// terminal's mouse input enabled.
    ///
    /// Case: the user enters vi mode and then drags to select text.
    #[test]
    fn enter_observer_disables_only_the_keyboard() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .add_observer(handle_enter_vi_mode_request);
        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        assert!(app.world().get::<KeyboardDisabled>(entity).is_some());
        assert!(app.world().get::<TerminalMouseDisabled>(entity).is_none());
    }

    /// Asserts that entering vi mode asks for a focused webview to be
    /// released.
    ///
    /// Case: the user has clicked into a page mounted in a pane and then
    /// enters vi mode to scroll back through that pane's output.
    #[test]
    fn enter_observer_requests_the_release_of_a_focused_webview() {
        #[derive(Resource, Default)]
        struct Requested(Vec<Option<Entity>>);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<FocusedWebview>()
            .init_resource::<Requested>()
            .add_observer(handle_enter_vi_mode_request)
            .add_observer(
                |ev: On<RequestWebviewFocus>, mut requested: ResMut<Requested>| {
                    requested.0.push(ev.target());
                },
            );
        let entity = spawn_terminal_entity(&mut app);
        let child = app
            .world_mut()
            .spawn((
                ChildOf(entity),
                Webview::new("h1".into(), InstanceId(1), MountId::new(1), 0, 10, 40),
            ))
            .id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(child);

        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();

        assert_eq!(app.world().resource::<Requested>().0, vec![None]);
    }

    /// Asserts that exiting vi mode removes `KeyboardDisabled` again.
    ///
    /// Case: the user leaves vi mode with `Esc`.
    #[test]
    fn exit_observer_reenables_the_keyboard() {
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
    }

    /// Asserts that exiting vi mode removes `ViModeState` and requests only
    /// the vi-mode-exit switch.
    ///
    /// Case: the user presses `Esc` to leave vi mode.
    #[test]
    fn exit_observer_removes_vi_mode_state_and_requests_the_switch() {
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
                SeenRequest::Switch(entity, ViModeSwitch::Enter),
                SeenRequest::Switch(entity, ViModeSwitch::Exit),
            ]
        );
    }
}
