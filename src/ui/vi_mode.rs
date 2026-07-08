//! Vi mode state. The vi cursor lives in alacritty
//! (`Term::vi_mode_cursor`) and the active selection lives in
//! `Term::selection`. This component is a pure marker — its presence
//! on a Surface entity means "vi mode is active". The v / V
//! toggle predicate reads `TerminalHandle::selection_type()` to
//! decide between "start new selection of kind X" and "clear existing".

use crate::input::focus::{KeyboardDisabled, MouseDisabled};
use bevy::app::{App, Plugin};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::observer::On;
use bevy::ecs::system::{Commands, Query};
use orzma_tty_engine::{Coalescer, TerminalHandle};

/// Bevy Plugin: registers the two observers. The `Clipboard` resource is
/// provided by `DefaultPlugins` (`bevy_clipboard::ClipboardPlugin`); orzma's
/// `crate::clipboard::ClipboardPlugin` adds the write-seam observer.
/// `ViModeState` is inserted/removed per-entity by the observers
/// themselves; no global system needed.
pub struct ViModePlugin;

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

/// Request to exit vi mode. The observer calls `TerminalHandle::exit_vi_mode`,
/// clears any selection, and removes `ViModeState`.
#[derive(EntityEvent, Debug)]
pub struct ExitViMode {
    /// The Surface entity to exit vi mode on.
    pub entity: Entity,
}

/// Observer for `EnterViModeActionEvent`. Inserts `ViModeState` on the
/// target entity and calls `TerminalHandle::enter_vi_mode`.
fn handle_enter_vi_mode_request(
    ev: On<EnterViModeActionEvent>,
    mut commands: Commands,
    mut q: Query<(&mut TerminalHandle, &mut Coalescer)>,
) {
    let Ok((mut handle, mut coalescer)) = q.get_mut(ev.entity) else {
        return;
    };
    // NOTE: must clear before entering vi mode — the v/V toggle predicate
    // (`resolve_selection_toggle`) reads `TerminalHandle::selection_type()` to
    // decide "start new selection" vs. "clear existing"; a leftover mouse-drag
    // selection would otherwise be misread as an already-started vi
    // selection, so the first post-entry `v` press would clear it instead of
    // starting a fresh one.
    handle.selection_clear(&mut coalescer);
    handle.enter_vi_mode(&mut coalescer);
    commands
        .entity(ev.entity)
        .insert((ViModeState, KeyboardDisabled, MouseDisabled));
}

/// Observer for `ExitViMode`. Removes `ViModeState`, clears any
/// selection, and calls `TerminalHandle::exit_vi_mode` (which snaps
/// the viewport to the live tail).
fn handle_exit_vi_mode(
    ev: On<ExitViMode>,
    mut commands: Commands,
    mut q: Query<(&mut TerminalHandle, &mut Coalescer)>,
) {
    let Ok((mut handle, mut coalescer)) = q.get_mut(ev.entity) else {
        return;
    };
    handle.selection_clear(&mut coalescer);
    handle.exit_vi_mode(&mut coalescer);
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
    use bevy::prelude::MinimalPlugins;
    use orzma_tty_engine::{SelectionType, SpawnOptions, TerminalBundle, TerminalHandle};

    fn spawn_terminal_entity(app: &mut App) -> Entity {
        let opts = SpawnOptions {
            cols: 10,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        app.world_mut().spawn(bundle).id()
    }

    #[test]
    fn enter_observer_inserts_vi_mode_state_and_does_not_create_selection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_vi_mode_request);

        let entity = spawn_terminal_entity(&mut app);

        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();

        assert!(app.world().get::<ViModeState>(entity).is_some());
        let h = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            h.selection_type().is_none(),
            "enter must not auto-create a selection",
        );
    }

    #[test]
    fn enter_observer_clears_a_pre_existing_selection() {
        // Regression: a leftover mouse-drag selection from before vi mode
        // was entered must not be misread by the v/V toggle predicate as an
        // already-started vi selection.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_vi_mode_request);

        let entity = spawn_terminal_entity(&mut app);
        {
            let mut e = app.world_mut().entity_mut(entity);
            let (mut h, mut coalescer) = (
                e.take::<TerminalHandle>().unwrap(),
                e.take::<Coalescer>().unwrap(),
            );
            h.selection_start(&mut coalescer, SelectionType::Simple);
            e.insert((h, coalescer));
        }

        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();

        let h = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            h.selection_type().is_none(),
            "entering vi mode must clear a pre-existing selection",
        );
    }

    #[test]
    fn enter_observer_disables_keyboard_and_mouse() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_vi_mode_request);
        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        assert!(app.world().get::<KeyboardDisabled>(entity).is_some());
        assert!(app.world().get::<MouseDisabled>(entity).is_some());
    }

    #[test]
    fn exit_observer_reenables_keyboard_and_mouse() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_vi_mode_request);
        app.add_observer(handle_exit_vi_mode);
        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        app.world_mut().trigger(ExitViMode { entity });
        app.update();
        assert!(app.world().get::<KeyboardDisabled>(entity).is_none());
        assert!(app.world().get::<MouseDisabled>(entity).is_none());
    }

    #[test]
    fn exit_observer_removes_vi_mode_state_and_clears_selection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_vi_mode_request);
        app.add_observer(handle_exit_vi_mode);

        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterViModeActionEvent { entity });
        app.update();
        {
            let mut e = app.world_mut().entity_mut(entity);
            let (mut h, mut coalescer) = (
                e.take::<TerminalHandle>().unwrap(),
                e.take::<Coalescer>().unwrap(),
            );
            h.selection_start(&mut coalescer, SelectionType::Simple);
            e.insert((h, coalescer));
        }

        app.world_mut().trigger(ExitViMode { entity });
        app.update();

        assert!(app.world().get::<ViModeState>(entity).is_none());
        let h = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            h.selection_type().is_none(),
            "exit must clear the selection"
        );
    }
}
