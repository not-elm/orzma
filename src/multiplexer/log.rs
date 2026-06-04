//! Layout-change logging system. Logs a human-readable summary of a
//! workspace's pane set when a pane is added to it (split / bootstrap).
//! `OzmuxLayoutLogPlugin` registers the system in `Update`.

use bevy::prelude::*;
use ozmux_multiplexer::{OwningWorkspace, PaneMarker, WorkspaceMarker};
use std::collections::HashSet;

/// Bevy Plugin that registers `log_layout_changes` in the `Update` schedule.
/// The system fires when a pane is added to a workspace (`Added<PaneMarker>`),
/// covering splits and the bootstrap pane.
pub struct OzmuxLayoutLogPlugin;

impl Plugin for OzmuxLayoutLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, log_layout_changes);
    }
}

fn log_layout_changes(
    added_panes: Query<&OwningWorkspace, Added<PaneMarker>>,
    workspace_names: Query<&Name, With<WorkspaceMarker>>,
    panes: Query<(&Name, &OwningWorkspace), With<PaneMarker>>,
) {
    let mut seen: HashSet<Entity> = HashSet::new();
    for owner in added_panes.iter() {
        let workspace = owner.0;
        if !seen.insert(workspace) {
            continue;
        }
        let workspace_name = workspace_names
            .get(workspace)
            .map(|n| n.as_str().to_owned())
            .unwrap_or_default();
        let pane_names: Vec<&str> = panes
            .iter()
            .filter(|(_, o)| o.0 == workspace)
            .map(|(n, _)| n.as_str())
            .collect();
        tracing::info!(
            target: "ozmux_gui::layout",
            ?workspace,
            workspace = %workspace_name,
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
