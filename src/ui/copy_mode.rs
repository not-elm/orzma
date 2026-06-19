//! Copy mode state. The vi cursor lives in alacritty
//! (`Term::vi_mode_cursor`) and the active selection lives in
//! `Term::selection`. This component is a pure marker — its presence
//! on a Surface entity means "copy mode is active". The v / V
//! toggle predicate reads `TerminalHandle::selection_type()` to
//! decide between "start new selection of kind X" and "clear existing".

use bevy::app::{App, Plugin};
use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::event::EntityEvent;
use bevy::ecs::observer::On;
use bevy::ecs::system::{Commands, Query};
use ozma_terminal::Clipboard;
use ozma_tty_engine::{Coalescer, TerminalHandle};

/// Bevy Plugin: registers the two observers and ensures the global
/// `Clipboard` resource exists (idempotent — `OzmaActionPlugin` already
/// provides it in the full binary). `CopyModeState` is inserted/removed
/// per-entity by the observers themselves; no global system needed.
pub struct CopyModePlugin;

impl Plugin for CopyModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Clipboard>()
            .add_observer(handle_enter_copy_mode_request)
            .add_observer(handle_exit_copy_mode);
    }
}

/// Marker: presence on a Surface entity means "copy mode is active".
#[derive(Component, Debug, Default)]
pub struct CopyModeState;

/// Request to enter copy mode on a specific Surface entity.
#[derive(EntityEvent, Debug)]
pub struct EnterCopyModeActionEvent {
    /// The Surface entity to enter copy mode on.
    pub entity: Entity,
}

/// Request to exit copy mode. The observer calls `TerminalHandle::exit_vi_mode`,
/// clears any selection, and removes `CopyModeState`.
#[derive(EntityEvent, Debug)]
pub struct ExitCopyMode {
    /// The Surface entity to exit copy mode on.
    pub entity: Entity,
}

/// Observer for `EnterCopyModeActionEvent`. Inserts `CopyModeState` on the
/// target entity and calls `TerminalHandle::enter_vi_mode`.
pub(crate) fn handle_enter_copy_mode_request(
    ev: On<EnterCopyModeActionEvent>,
    mut commands: Commands,
    mut q: Query<(&mut TerminalHandle, &mut Coalescer)>,
) {
    let Ok((mut handle, mut coalescer)) = q.get_mut(ev.entity) else {
        return;
    };
    handle.enter_vi_mode(&mut coalescer);
    commands.entity(ev.entity).insert(CopyModeState);
}

/// Observer for `ExitCopyMode`. Removes `CopyModeState`, clears any
/// selection, and calls `TerminalHandle::exit_vi_mode` (which snaps
/// the viewport to the live tail).
pub(crate) fn handle_exit_copy_mode(
    ev: On<ExitCopyMode>,
    mut commands: Commands,
    mut q: Query<(&mut TerminalHandle, &mut Coalescer)>,
) {
    let Ok((mut handle, mut coalescer)) = q.get_mut(ev.entity) else {
        return;
    };
    handle.selection_clear(&mut coalescer);
    handle.exit_vi_mode(&mut coalescer);
    commands.entity(ev.entity).remove::<CopyModeState>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::prelude::MinimalPlugins;
    use ozma_tty_engine::{SelectionType, SpawnOptions, TerminalBundle, TerminalHandle};

    fn spawn_terminal_entity(app: &mut App) -> Entity {
        let opts = SpawnOptions {
            cols: 10,
            rows: 5,
            shell: "/bin/sh".into(),
            cwd: None,
            env: Vec::new(),
            osc_webview_gate: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let bundle = TerminalBundle::spawn(opts).expect("spawn /bin/sh");
        app.world_mut().spawn(bundle).id()
    }

    #[test]
    fn enter_observer_inserts_copy_mode_state_and_does_not_create_selection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_copy_mode_request);

        let entity = spawn_terminal_entity(&mut app);

        app.world_mut().trigger(EnterCopyModeActionEvent { entity });
        app.update();

        assert!(app.world().get::<CopyModeState>(entity).is_some());
        let h = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            h.selection_type().is_none(),
            "enter must not auto-create a selection",
        );
    }

    #[test]
    fn exit_observer_removes_copy_mode_state_and_clears_selection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(handle_enter_copy_mode_request);
        app.add_observer(handle_exit_copy_mode);

        let entity = spawn_terminal_entity(&mut app);
        app.world_mut().trigger(EnterCopyModeActionEvent { entity });
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

        app.world_mut().trigger(ExitCopyMode { entity });
        app.update();

        assert!(app.world().get::<CopyModeState>(entity).is_none());
        let h = app.world().get::<TerminalHandle>(entity).unwrap();
        assert!(
            h.selection_type().is_none(),
            "exit must clear the selection"
        );
    }
}
