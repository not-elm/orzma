//! Vi mode state. This component is a pure marker — its presence on a
//! Surface entity means "vi mode is active". Entering and exiting request a
//! selection clear and a `RequestTtyViMode` switch on the underlying tty;
//! where the vi cursor and the active selection actually live is an
//! implementation detail of whatever `Vt` capability eventually backs them.

use crate::input::focus::{KeyboardDisabled, MouseDisabled};
use bevy::app::{App, Plugin};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::observer::On;
use bevy::ecs::query::With;
use bevy::ecs::system::{Commands, Query};
use bevy_orzma_mux::prelude::{MuxPane, RequestTtySelectionClear, RequestTtyViMode, ViModeSwitch};

/// Bevy Plugin: registers the two observers. The `Clipboard` resource is
/// provided by `DefaultPlugins` (`bevy_clipboard::ClipboardPlugin`); orzma's
/// `crate::action::clipboard` copy plugin adds the write-seam observer.
/// `ViModeState` is inserted/removed per-entity by the observers
/// themselves; no global system needed.
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

/// Request to exit vi mode. The observer requests a selection clear and a
/// `RequestTtyViMode { switch: Exit }`, and removes `ViModeState`.
#[derive(EntityEvent, Debug)]
pub struct ExitViMode {
    /// The Surface entity to exit vi mode on.
    pub entity: Entity,
}

/// Observer for `EnterViModeActionEvent`. Inserts `ViModeState` on the
/// target entity and requests a selection clear followed by the vi-mode
/// enter switch.
fn handle_enter_vi_mode_request(
    ev: On<EnterViModeActionEvent>,
    mut commands: Commands,
    terminals: Query<(), With<MuxPane>>,
) {
    if terminals.get(ev.entity).is_err() {
        return;
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
        .insert((ViModeState, KeyboardDisabled, MouseDisabled));
}

/// Observer for `ExitViMode`. Removes `ViModeState`, and requests a
/// selection clear followed by the vi-mode exit switch (which snaps the
/// viewport to the live tail).
fn handle_exit_vi_mode(
    ev: On<ExitViMode>,
    mut commands: Commands,
    terminals: Query<(), With<MuxPane>>,
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
        .remove::<MouseDisabled>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::ecs::resource::Resource;
    use bevy::ecs::system::ResMut;
    use bevy::prelude::MinimalPlugins;
    use orzma_mux::prelude::PaneId;

    fn spawn_terminal_entity(app: &mut App) -> Entity {
        app.world_mut().spawn(MuxPane(PaneId(1))).id()
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

    /// Asserts that entering vi mode on an entity without a `MuxPane`
    /// neither inserts `ViModeState` nor fires any request.
    ///
    /// Case: a stray `EnterViModeActionEvent` aimed at an entity that never
    /// had a `MuxPane`, or whose pane was already torn down.
    #[test]
    fn enter_request_on_a_bare_entity_is_a_no_op() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_observer(handle_enter_vi_mode_request);
        capture_requests(&mut app);
        let entity = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();

        assert!(app.world().get::<ViModeState>(entity).is_none());
        assert!(app.world().resource::<SeenRequests>().0.is_empty());
    }

    /// Asserts that entering vi mode marks the entity `KeyboardDisabled`
    /// and `MouseDisabled`.
    ///
    /// Case: the user enters vi mode on the focused terminal.
    #[test]
    fn enter_observer_disables_keyboard_and_mouse() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_observer(handle_enter_vi_mode_request);
        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        assert!(app.world().get::<KeyboardDisabled>(entity).is_some());
        assert!(app.world().get::<MouseDisabled>(entity).is_some());
    }

    /// Asserts that exiting vi mode removes `KeyboardDisabled` and
    /// `MouseDisabled` again.
    ///
    /// Case: the user leaves vi mode with `Esc`.
    #[test]
    fn exit_observer_reenables_keyboard_and_mouse() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_observer(handle_enter_vi_mode_request)
            .add_observer(handle_exit_vi_mode);
        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        app.world_mut().trigger(ExitViMode { entity });
        app.update();
        assert!(app.world().get::<KeyboardDisabled>(entity).is_none());
        assert!(app.world().get::<MouseDisabled>(entity).is_none());
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
