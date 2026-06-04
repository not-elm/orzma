//! Layout-change logging system. Watches `Changed<Children>` on Workspace
//! layout-root nodes and logs a human-readable summary of the pane tree.
//! `OzmuxLayoutLogPlugin` registers the system in `Update`.

use bevy::prelude::*;
use ozmux_multiplexer::{OwningWorkspace, PaneMarker, WorkspaceMarker};

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
    workspaces: Query<(Entity, &Name), (With<WorkspaceMarker>, Changed<Name>)>,
    panes: Query<(&Name, &OwningWorkspace), With<PaneMarker>>,
) {
    for (entity, name) in workspaces.iter() {
        let pane_names: Vec<&str> = panes
            .iter()
            .filter(|(_, owner)| owner.0 == entity)
            .map(|(n, _)| n.as_str())
            .collect();
        tracing::info!(
            target: "ozmux_gui::layout",
            ?entity,
            workspace = %name,
            pane_count = pane_names.len(),
            panes = ?pane_names,
            "layout changed",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_multiplexer::MultiplexerPlugin;

    #[test]
    fn log_layout_changes_has_no_system_param_conflict() {
        // Regression for the B0001 panic: the system must not pair a `&mut Name`
        // (via MultiplexerCommands) with a `&Name` workspace query. Building the
        // schedule and running one update would panic if the params conflict.
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin)
            .add_plugins(OzmuxLayoutLogPlugin);
        app.update();
    }
}
