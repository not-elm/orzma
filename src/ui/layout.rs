//! Cell-tree → Bevy UI Node builders. `build_cell_recursive` descends the
//! `LayoutCellState` and emits `Cell::Pane` / `Cell::Split` nodes as flex
//! containers (geometry only — tab/host/veil content is owned by
//! `sync_pane_activities`); `split_ratio_to_flex_grows` is the math that
//! normalizes `lhs_weight` / `rhs_weight` into a `(flex_grow_lhs,
//! flex_grow_rhs)` pair so that `lhs_weight == rhs_weight == 0` falls back to
//! 0.5/0.5.

use crate::theme;
use crate::ui::pane_chrome::PaneChrome;
use crate::ui::{ActivityHostNode, PaneFrame, StructuralNode, palette};
use bevy::prelude::*;
use bevy::ui::{FlexDirection, UiRect, Val};
use ozmux_multiplexer::{Cell, CellId, LayoutCellState, SplitOrientation};

/// Convert `(lhs_weight, rhs_weight)` into the `flex_grow` pair to set on
/// the two split children. Routes through `LayoutCellState::split_ratio`
/// so zero-total weight falls back to 0.5/0.5 (avoiding the flex-collapse
/// that raw `(0.0, 0.0)` would cause).
pub(crate) fn split_ratio_to_flex_grows(lhs_weight: f32, rhs_weight: f32) -> (f32, f32) {
    let ratio = LayoutCellState::split_ratio(lhs_weight, rhs_weight);
    (ratio, 1.0 - ratio)
}

/// Recursively build the Bevy UI **geometry** for one Cell subtree under
/// `parent`. Walks `Cell::Split` → two children, lands on `Cell::Pane` to
/// spawn the pane frame and reparent that pane's stable `PaneChrome`
/// containers under it. Tab/host/veil content is owned by
/// `sync_pane_activities`, not built here.
///
/// `Cell::Root` appearing mid-recursion is treated as an invariant
/// violation (warn-and-skip); the entry point in `rebuild_session_ui`
/// is expected to unwrap into `RootCell::child` first.
pub(crate) fn build_cell_recursive(
    commands: &mut Commands,
    parent: Entity,
    cells: &LayoutCellState,
    cell_id: &CellId,
    pane_chromes: &Query<&PaneChrome>,
) {
    let cell = match cells.cell(cell_id) {
        Ok(c) => c,
        Err(err) => {
            tracing::warn!(
                target: "ozmux_gui::layout",
                "cell {} missing ({:?})",
                cell_id,
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
        Cell::Pane(p) => build_pane(commands, parent, p.pane, pane_chromes),
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
            build_cell_recursive(commands, lhs, cells, &s.lhs_cell, pane_chromes);

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
            build_cell_recursive(commands, rhs, cells, &s.rhs_cell, pane_chromes);
        }
    }
}

/// Reads the pane's stable `PaneChrome`, spawning + inserting it on first use.
/// The two containers are spawned detached (no parent); the caller reparents
/// them under the pane frame.
///
/// `tab_bar_root` and `activity_slot` carry `ActivityHostNode` (not
/// `StructuralNode`) so the geometry rebuild's `descend_and_despawn_structural`
/// does not despawn them and `descend_and_detach_hosts` detaches them — the
/// same survive-and-reparent treatment activity hosts get.
fn get_or_spawn_pane_chrome(
    commands: &mut Commands,
    pane: Entity,
    existing: Option<&PaneChrome>,
) -> PaneChrome {
    if let Some(chrome) = existing {
        return *chrome;
    }
    let tab_bar_root = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                height: Val::Auto,
                padding: UiRect::ZERO,
                ..default()
            },
            BackgroundColor(palette::TAB_BAR_BG),
            ActivityHostNode,
        ))
        .id();
    let activity_slot = commands
        .spawn((
            Node {
                flex_grow: 1.0,
                border: UiRect::all(Val::Px(theme::PANE_BORDER_PX)),
                width: Val::Percent(100.0),
                ..default()
            },
            BorderColor::all(palette::BORDER),
            ActivityHostNode,
        ))
        .id();
    let chrome = PaneChrome {
        tab_bar_root,
        activity_slot,
    };
    commands.entity(pane).insert(chrome);
    chrome
}

fn build_pane(
    commands: &mut Commands,
    parent: Entity,
    pane_entity: Entity,
    pane_chromes: &Query<&PaneChrome>,
) {
    let pane_frame = commands
        .spawn((
            Name::new(format!("Pane({pane_entity:?})")),
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

    let chrome =
        get_or_spawn_pane_chrome(commands, pane_entity, pane_chromes.get(pane_entity).ok());
    commands
        .entity(chrome.tab_bar_root)
        .insert(ChildOf(pane_frame));
    commands
        .entity(chrome.activity_slot)
        .insert(ChildOf(pane_frame));
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
