//! Cell-tree → Bevy UI Node builders. `build_cell_recursive` descends the
//! `LayoutCellState` and emits `Cell::Pane` / `Cell::Split` nodes as flex
//! containers; `split_ratio_to_flex_grows` is the math that normalizes
//! `lhs_weight` / `rhs_weight` into a `(flex_grow_lhs, flex_grow_rhs)`
//! pair so that `lhs_weight == rhs_weight == 0` falls back to 0.5/0.5.

use crate::theme;
use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::{PaneFrame, StructuralNode, palette};
use bevy::prelude::*;
use bevy::ui::{FlexDirection, UiRect, Val};
use ozmux_multiplexer::{ActivityId, Cell, CellId, LayoutCellState, SplitOrientation, Window};
use std::collections::HashSet;

/// Convert `(lhs_weight, rhs_weight)` into the `flex_grow` pair to set on
/// the two split children. Routes through `LayoutCellState::split_ratio`
/// so zero-total weight falls back to 0.5/0.5 (avoiding the flex-collapse
/// that raw `(0.0, 0.0)` would cause).
pub(crate) fn split_ratio_to_flex_grows(lhs_weight: f32, rhs_weight: f32) -> (f32, f32) {
    let ratio = LayoutCellState::split_ratio(lhs_weight, rhs_weight);
    (ratio, 1.0 - ratio)
}

/// Recursively build the Bevy UI tree for one Cell subtree under `parent`.
/// Walks `Cell::Split` → two children, lands on `Cell::Pane` to spawn the
/// pane frame + tab bar + activity host slot.
///
/// `Cell::Root` appearing mid-recursion is treated as an invariant
/// violation (warn-and-skip); the entry point in
/// `rebuild_structure_on_change` is expected to unwrap into
/// `RootCell::child` first.
pub(crate) fn build_cell_recursive(
    commands: &mut Commands,
    parent: Entity,
    window: &Window,
    cell_id: &CellId,
    registry: &mut ActivityEntityRegistry,
    live_activity_ids: &mut HashSet<ActivityId>,
) {
    let cell = match window.cells.cell(cell_id) {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(
                target: "ozmux_gui::layout",
                "cell {} missing from window {} ({:?})",
                cell_id,
                window.id,
                err,
            );
            return;
        }
    };

    match cell {
        Cell::Root(_) => {
            tracing::warn!(
                target: "ozmux_gui::layout",
                "unexpected nested Cell::Root at {}",
                cell_id,
            );
        }
        Cell::Pane(p) => build_pane(commands, parent, window, p, registry, live_activity_ids),
        Cell::Split(s) => {
            let (lhs_grow, rhs_grow) = split_ratio_to_flex_grows(s.lhs_weight, s.rhs_weight);
            let dir = match s.orientation {
                SplitOrientation::Horizontal => FlexDirection::Row,
                SplitOrientation::Vertical => FlexDirection::Column,
            };

            let container = commands
                .spawn((
                    Node {
                        flex_direction: dir,
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    StructuralNode,
                    ChildOf(parent),
                ))
                .id();

            let lhs = commands
                .spawn((
                    Node {
                        flex_grow: lhs_grow,
                        flex_basis: Val::Px(0.0),
                        ..default()
                    },
                    StructuralNode,
                    ChildOf(container),
                ))
                .id();
            build_cell_recursive(
                commands,
                lhs,
                window,
                &s.lhs_cell,
                registry,
                live_activity_ids,
            );

            let rhs = commands
                .spawn((
                    Node {
                        flex_grow: rhs_grow,
                        flex_basis: Val::Px(0.0),
                        ..default()
                    },
                    StructuralNode,
                    ChildOf(container),
                ))
                .id();
            build_cell_recursive(
                commands,
                rhs,
                window,
                &s.rhs_cell,
                registry,
                live_activity_ids,
            );
        }
    }
}

fn build_pane(
    commands: &mut Commands,
    parent: Entity,
    window: &Window,
    pane_cell: &ozmux_multiplexer::PaneCell,
    registry: &mut ActivityEntityRegistry,
    live_activity_ids: &mut HashSet<ActivityId>,
) {
    let Some(pane) = window.panes.get(&pane_cell.pane) else {
        tracing::warn!(
            target: "ozmux_gui::layout",
            "pane {} referenced by cell missing",
            pane_cell.pane,
        );
        return;
    };
    let is_active_pane = pane_cell.pane == window.active_pane;

    let pane_frame = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                border: UiRect::all(Val::Px(theme::PANE_BORDER_PX)),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BorderColor::all(palette::BORDER),
            BackgroundColor(palette::BACKGROUND),
            StructuralNode,
            PaneFrame,
            ChildOf(parent),
        ))
        .id();

    crate::ui::tab_bar::build_pane_tab_bar(commands, pane_frame, pane, is_active_pane);

    let activity_slot = commands
        .spawn((
            Node {
                flex_grow: 1.0,
                width: Val::Percent(100.0),
                ..default()
            },
            StructuralNode,
            ChildOf(pane_frame),
        ))
        .id();

    if let Some(activity) = pane
        .activities
        .iter()
        .find(|a| a.id == pane.active_activity)
    {
        let host = registry.get_or_spawn(commands, &activity.id, &activity.kind);
        commands.entity(host).insert(ChildOf(activity_slot));
        crate::ui::activity::build_activity_host_children(commands, host, activity);
        live_activity_ids.insert(activity.id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_weight_split_uses_half_half_ratio() {
        let (lhs, rhs) = split_ratio_to_flex_grows(0.0, 0.0);
        assert!((lhs - 0.5).abs() < f32::EPSILON);
        assert!((rhs - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn non_zero_weights_pass_through_split_ratio() {
        let (lhs, rhs) = split_ratio_to_flex_grows(1.0, 3.0);
        assert!((lhs - 0.25).abs() < f32::EPSILON);
        assert!((rhs - 0.75).abs() < f32::EPSILON);
    }
}
