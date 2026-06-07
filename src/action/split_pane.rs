//! Split-pane shortcut action: splits the active pane along an orientation
//! when a `SplitPaneActionEvent` is triggered.
use bevy::prelude::*;
use ozmux_multiplexer::{Cwd, SplitOrientation};
#[cfg(not(feature = "thin-client"))]
use ozmux_multiplexer::{MultiplexerCommands, Side, SurfaceKind};

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
    pub workspace: Entity,
    pub orientation: SplitOrientation,
}

// NOTE: `mut mux` precedes `mut commands` so the multiplexer's deferred spawn
// of the new surface flushes before `commands` inserts `Cwd` on it (sanctioned
// rust.md ordering exception).
fn apply_split(
    trigger: On<SplitPaneActionEvent>,
    #[cfg(not(feature = "thin-client"))] mut mux: MultiplexerCommands,
    #[cfg(not(feature = "thin-client"))] mut commands: Commands,
    #[cfg(feature = "thin-client")] mut conn: bevy::ecs::system::NonSendMut<
        crate::thin_client::ThinClientConn,
    >,
    #[cfg(feature = "thin-client")] query: ozmux_multiplexer::MultiplexerQuery,
    #[cfg(feature = "thin-client")] pane_ids: Query<&ozmux_multiplexer::MuxPaneId>,
    cwds: Query<&Cwd>,
) {
    #[cfg(not(feature = "thin-client"))]
    {
        let SplitPaneActionEvent {
            workspace,
            orientation,
        } = trigger.event();
        let Some(active_pane) = mux.workspaces_active_pane(*workspace) else {
            tracing::warn!(target: "ozmux_gui::commands", ?workspace, "Split: workspace vanished");
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
    #[cfg(feature = "thin-client")]
    {
        let SplitPaneActionEvent {
            workspace,
            orientation,
        } = trigger.event();
        let Some(active_pane) = query.workspaces_active_pane(*workspace) else {
            return;
        };
        let Ok(pane) = pane_ids.get(active_pane).map(|c| c.0) else {
            return;
        };
        let cwd = query
            .panes_active_surface(active_pane)
            .and_then(|s| cwds.get(s).ok())
            .map(|c| c.0.clone());
        crate::thin_client::send_cmd(
            &mut conn,
            ozmux_proto::ClientMessage::Split {
                pane,
                orientation: split_orientation_to_wire(*orientation),
                side: ozmux_proto::Side::After,
                kind: ozmux_proto::SurfaceKind::Terminal,
                cwd,
            },
        );
    }
}

#[cfg(feature = "thin-client")]
fn split_orientation_to_wire(o: SplitOrientation) -> ozmux_proto::SplitOrientation {
    match o {
        SplitOrientation::Horizontal => ozmux_proto::SplitOrientation::Horizontal,
        SplitOrientation::Vertical => ozmux_proto::SplitOrientation::Vertical,
    }
}

#[cfg(all(test, not(feature = "thin-client")))]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use ozmux_multiplexer::{ActivePane, Cwd, MultiplexerCommands, MultiplexerPlugin};

    #[test]
    fn split_copies_active_surface_cwd_to_new_surface() {
        let mut app = App::new();
        app.add_plugins(MultiplexerPlugin);
        app.add_plugins(SplitPaneActionPlugin);
        let workspace = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("t".into())).workspace
            })
            .unwrap();
        let active_pane = app
            .world()
            .get::<ActivePane>(workspace)
            .map(|a| a.0)
            .unwrap();
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
            workspace,
            orientation: SplitOrientation::Vertical,
        });
        app.world_mut().flush();

        let new_surface = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| {
                let p = mux.workspaces_active_pane(workspace).unwrap();
                mux.panes_active_surface(p).unwrap()
            })
            .unwrap();
        assert_eq!(
            app.world().get::<Cwd>(new_surface),
            Some(&Cwd("/tmp/proj".into()))
        );
    }
}
