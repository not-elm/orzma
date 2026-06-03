//! Split-pane shortcut action: splits the active pane along an orientation
//! when a `SplitPaneActionEvent` is triggered.
use bevy::prelude::*;
use ozmux_multiplexer::{Cwd, MultiplexerCommands, Side, SplitOrientation, SurfaceKind};

/// Registers the `apply_split` observer for `SplitPaneActionEvent`.
pub struct SplitPaneActionPlugin;

impl Plugin for SplitPaneActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(apply_split);
    }
}

/// Request to split the active pane along `orientation`. Triggered by
/// `ShortcutAction::SplitPane`.
#[derive(EntityEvent, Debug)]
pub struct SplitPaneActionEvent {
    #[event_target]
    pub session: Entity,
    pub orientation: SplitOrientation,
}

// NOTE: `mut mux` precedes `mut commands` so the multiplexer's deferred spawn
// of the new surface flushes before `commands` inserts `Cwd` on it (sanctioned
// rust.md ordering exception).
fn apply_split(
    trigger: On<SplitPaneActionEvent>,
    mut mux: MultiplexerCommands,
    mut commands: Commands,
    cwds: Query<&Cwd>,
) {
    let SplitPaneActionEvent {
        session,
        orientation,
    } = trigger.event();
    let Some(active_pane) = mux.sessions_active_pane(*session) else {
        tracing::warn!(target: "ozmux_gui::commands", ?session, "Split: session vanished");
        return;
    };
    let seed = mux
        .panes_active_surface(active_pane)
        .and_then(|s| cwds.get(s).ok().cloned());
    match mux.split_pane_with_surface(
        active_pane,
        Side::After,
        *orientation,
        SurfaceKind::Terminal,
    ) {
        Ok(outcome) => {
            if let Some(cwd) = seed {
                commands.entity(outcome.surface).insert(cwd);
            }
        }
        Err(e) => tracing::warn!(target: "ozmux_gui::commands", ?e, "split_pane failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{ActivePane, Cwd, MultiplexerCommands, MultiplexerPlugin};

    #[test]
    fn split_copies_active_surface_cwd_to_new_surface() {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(SplitPaneActionPlugin);
        let session = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("t".into())).session
            })
            .unwrap();
        let active_pane = app.world().get::<ActivePane>(session).map(|a| a.0).unwrap();
        let src_surface = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                mux.panes_active_surface(active_pane).unwrap()
            })
            .unwrap();
        app.world_mut()
            .entity_mut(src_surface)
            .insert(Cwd("/tmp/proj".into()));

        app.world_mut().trigger(SplitPaneActionEvent {
            session,
            orientation: SplitOrientation::Vertical,
        });
        app.world_mut().flush();

        let new_surface = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                let p = mux.sessions_active_pane(session).unwrap();
                mux.panes_active_surface(p).unwrap()
            })
            .unwrap();
        assert_eq!(
            app.world().get::<Cwd>(new_surface),
            Some(&Cwd("/tmp/proj".into()))
        );
    }
}
