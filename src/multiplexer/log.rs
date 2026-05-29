//! Layout-change logging system. Watches `Changed<LayoutCells>` on
//! Session entities and logs a human-readable summary of the cell tree.
//! `OzmuxLayoutLogPlugin` registers the system in `Update`.

use bevy::prelude::*;
use ozmux_multiplexer::{Cell, LayoutCells, PaneMarker, SessionMarker};

/// Bevy Plugin that registers `log_layout_changes` in the `Update`
/// schedule behind `Changed<LayoutCells>` so it fires only on layout
/// mutations.
pub struct OzmuxLayoutLogPlugin;

impl Plugin for OzmuxLayoutLogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, log_layout_changes);
    }
}

fn log_layout_changes(
    sessions: Query<(Entity, &Name, &LayoutCells), (With<SessionMarker>, Changed<LayoutCells>)>,
    panes: Query<&Name, With<PaneMarker>>,
) {
    for (entity, name, layout) in sessions.iter() {
        let pane_count = count_panes(layout);
        let pane_names: Vec<&str> = collect_pane_names(layout, &panes);
        tracing::info!(
            target: "ozmux_gui::layout",
            ?entity,
            session = %name,
            pane_count,
            panes = ?pane_names,
            "layout changed",
        );
    }
}

/// Count the number of `Cell::Pane` leaves in the layout's cell tree.
fn count_panes(layout: &LayoutCells) -> usize {
    let mut count = 0;
    let mut stack = vec![layout.root];
    while let Some(cell_id) = stack.pop() {
        match layout.cells.cell(&cell_id) {
            Ok(Cell::Root(r)) => stack.push(r.child),
            Ok(Cell::Split(s)) => {
                stack.push(s.lhs_cell);
                stack.push(s.rhs_cell);
            }
            Ok(Cell::Pane(_)) => count += 1,
            Err(_) => {}
        }
    }
    count
}

/// Collect the display names of all panes in DFS order.
fn collect_pane_names<'a>(
    layout: &LayoutCells,
    panes: &'a Query<&Name, With<PaneMarker>>,
) -> Vec<&'a str> {
    let mut names = Vec::new();
    let mut stack = vec![layout.root];
    while let Some(cell_id) = stack.pop() {
        match layout.cells.cell(&cell_id) {
            Ok(Cell::Root(r)) => stack.push(r.child),
            Ok(Cell::Split(s)) => {
                stack.push(s.rhs_cell);
                stack.push(s.lhs_cell);
            }
            Ok(Cell::Pane(p)) => {
                if let Ok(name) = panes.get(p.pane) {
                    names.push(name.as_str());
                }
            }
            Err(_) => {}
        }
    }
    names
}
