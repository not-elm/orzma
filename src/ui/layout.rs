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
use ozmux_multiplexer::{Cell, CellId, LayoutCellState, Session, SplitOrientation};

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
/// `rebuild_session_ui_on_data_change` is expected to unwrap into
/// `RootCell::child` first.
///
/// `inactive_host_parent` — the Entity under which inactive Activity hosts
/// (within this session) are parked. In production this is the owning
/// Session entity itself, which lacks `Node`, so the host falls out of
/// Bevy's UI walker (`UiChildren::iter_ui_children` filters `With<Node>`).
/// This is the mechanism that replaces the previous `hidden_stash`
/// `Display::None` workaround.
pub(crate) fn build_cell_recursive(
    commands: &mut Commands,
    parent: Entity,
    session: &Session,
    cell_id: &CellId,
    registry: &mut ActivityEntityRegistry,
    inactive_host_parent: Entity,
    ui_font: &Handle<Font>,
) {
    let cell = match session.cells.cell(cell_id) {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(
                target: "ozmux_gui::layout",
                "cell {} missing from session {} ({:?})",
                cell_id,
                session.id,
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
        Cell::Pane(p) => build_pane(
            commands,
            parent,
            session,
            p,
            registry,
            inactive_host_parent,
            ui_font,
        ),
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
                session,
                &s.lhs_cell,
                registry,
                inactive_host_parent,
                ui_font,
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
                session,
                &s.rhs_cell,
                registry,
                inactive_host_parent,
                ui_font,
            );
        }
    }
}

fn build_pane(
    commands: &mut Commands,
    parent: Entity,
    session: &Session,
    pane_cell: &ozmux_multiplexer::PaneCell,
    registry: &mut ActivityEntityRegistry,
    inactive_host_parent: Entity,
    ui_font: &Handle<Font>,
) {
    let Some(pane) = session.panes.get(&pane_cell.pane) else {
        tracing::warn!(
            target: "ozmux_gui::layout",
            "pane {} referenced by cell missing",
            pane_cell.pane,
        );
        return;
    };
    let is_active_pane = pane_cell.pane == session.active_pane;

    let pane_frame = commands
        .spawn((
            Name::new(format!("Pane({})", session.name)),
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                padding: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            StructuralNode,
            PaneFrame,
            ChildOf(parent),
        ))
        .id();

    crate::ui::tab_bar::build_pane_tab_bar(commands, pane_frame, pane, is_active_pane, ui_font);

    let activity_slot = commands
        .spawn((
            Node {
                flex_grow: 1.0,
                border: UiRect::all(Val::Px(theme::PANE_BORDER_PX)),
                width: Val::Percent(100.0),
                ..default()
            },
            StructuralNode,
            BorderColor::all(palette::BORDER),
            ChildOf(pane_frame),
        ))
        .id();

    // NOTE: Inactive activity hosts are parked under `inactive_host_parent`,
    // which the caller guarantees is a non-`Node` entity (the owning
    // Session). The UI walker filters `With<Node>` on parents in
    // `UiChildren::iter_ui_children`, so inactive hosts never appear in
    // `ui_layout_system`'s traversal — no layout, no `ComputedNode`
    // updates, no resize-pass work.
    for activity in &pane.activities {
        let host = registry.get_or_spawn(commands, &activity.id, &activity.kind);
        if activity.id == pane.active_activity {
            commands.entity(host).insert(ChildOf(activity_slot));
            crate::ui::activity::build_activity_host_children(commands, host, activity);
        } else {
            commands.entity(host).insert(ChildOf(inactive_host_parent));
        }
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
