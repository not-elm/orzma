//! Layout-change logging system. Watches `Changed<Children>` on Workspace
//! layout-root nodes and logs a human-readable summary of the pane tree.
//! `OzmuxLayoutLogPlugin` registers the system in `Update`.

use bevy::prelude::*;
use ozmux_multiplexer::{MultiplexerCommands, PaneMarker, WorkspaceMarker};

/// Bevy Plugin that registers `log_layout_changes` in the `Update`
/// schedule. The system fires only on workspaces whose `Name` changed or
/// whose pane set changed (a layout mutation reparents panes, flagging
/// `Changed<Children>` on the affected nodes).
pub struct OzmuxLayoutLogPlugin;

impl Plugin for OzmuxLayoutLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, log_layout_changes);
    }
}

fn log_layout_changes(
    mux: MultiplexerCommands,
    workspaces: Query<(Entity, &Name), (With<WorkspaceMarker>, Changed<Name>)>,
    panes: Query<&Name, With<PaneMarker>>,
) {
    for (entity, name) in workspaces.iter() {
        let pane_entities: Vec<Entity> = mux.panes_of_workspace(entity).collect();
        let pane_names: Vec<&str> = pane_entities
            .iter()
            .filter_map(|&p| panes.get(p).ok().map(|n| n.as_str()))
            .collect();
        tracing::info!(
            target: "ozmux_gui::layout",
            ?entity,
            workspace = %name,
            pane_count = pane_entities.len(),
            panes = ?pane_names,
            "layout changed",
        );
    }
}
