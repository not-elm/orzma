//! Cell-tree → Bevy UI Node builders. `build_cell_recursive` descends the
//! `LayoutCellState` and emits `Cell::Pane` / `Cell::Split` nodes as flex
//! containers; `split_ratio_to_flex_grows` is the math that normalizes
//! `lhs_weight` / `rhs_weight` into a `(flex_grow_lhs, flex_grow_rhs)`
//! pair so that `lhs_weight == rhs_weight == 0` falls back to 0.5/0.5.

use crate::theme;
use crate::ui::surface::decorate_surface;
use crate::ui::tab_bar::{TabEntry, build_pane_tab_bar};
use crate::ui::tab_label::{LabelCtx, tab_label};
use crate::ui::web_title::WebTitle;
use crate::ui::{PaneDimOverlay, PaneFrame, Slotted, StructuralNode, palette};
use bevy::prelude::*;
use bevy::ui::{FlexDirection, PositionType, UiRect, Val};
use ozmux_multiplexer::{
    ActiveSurface, Cell, CellId, Cwd, LayoutCellState, PaneMarker, SplitOrientation, SurfaceKind,
    SurfaceMarker, Surfaces,
};

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
/// pane frame + tab bar + surface host slot.
///
/// `Cell::Root` appearing mid-recursion is treated as an invariant
/// violation (warn-and-skip); the entry point in `rebuild_workspace_ui`
/// is expected to unwrap into `RootCell::child` first.
///
/// `inactive_host_parent` — the Entity under which inactive Surface entities
/// (within this workspace) are parked. In production this is the owning
/// Workspace entity itself, which lacks `Node`, so the surface falls out of
/// Bevy's UI walker (`UiChildren::iter_ui_children` filters `With<Node>`).
/// This is the mechanism that replaces the previous `hidden_stash`
/// `Display::None` workaround.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_cell_recursive(
    commands: &mut Commands,
    parent: Entity,
    cells: &LayoutCellState,
    cell_id: &CellId,
    inactive_host_parent: Entity,
    ui_font: &Handle<Font>,
    pane_children: &Query<&Surfaces, With<PaneMarker>>,
    surfaces: &Query<(&SurfaceKind, &Name, Option<&Cwd>, Option<&WebTitle>), With<SurfaceMarker>>,
    active_surfaces: &Query<&ActiveSurface, With<PaneMarker>>,
    active_pane: Entity,
    veil: Option<Color>,
    label_ctx: &LabelCtx,
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
        Cell::Pane(p) => build_pane(
            commands,
            parent,
            p.pane,
            inactive_host_parent,
            ui_font,
            pane_children,
            surfaces,
            active_surfaces,
            active_pane,
            veil,
            label_ctx,
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
                cells,
                &s.lhs_cell,
                inactive_host_parent,
                ui_font,
                pane_children,
                surfaces,
                active_surfaces,
                active_pane,
                veil,
                label_ctx,
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
                cells,
                &s.rhs_cell,
                inactive_host_parent,
                ui_font,
                pane_children,
                surfaces,
                active_surfaces,
                active_pane,
                veil,
                label_ctx,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_pane(
    commands: &mut Commands,
    parent: Entity,
    pane_entity: Entity,
    inactive_host_parent: Entity,
    ui_font: &Handle<Font>,
    pane_children: &Query<&Surfaces, With<PaneMarker>>,
    surfaces: &Query<(&SurfaceKind, &Name, Option<&Cwd>, Option<&WebTitle>), With<SurfaceMarker>>,
    active_surfaces: &Query<&ActiveSurface, With<PaneMarker>>,
    active_pane: Entity,
    veil: Option<Color>,
    label_ctx: &LabelCtx,
) {
    let active_surface = active_surfaces
        .get(pane_entity)
        .map(|a| a.0)
        .unwrap_or(Entity::PLACEHOLDER);

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

    let surface_entities: Vec<Entity> = pane_children
        .get(pane_entity)
        .map(|s| s.iter().filter(|e| surfaces.get(*e).is_ok()).collect())
        .unwrap_or_default();

    let tabs: Vec<TabEntry> = surface_entities
        .iter()
        .filter_map(|&ae| {
            let (kind, _name, cwd, web_title) = surfaces.get(ae).ok()?;
            Some(TabEntry {
                entity: ae,
                name: tab_label(
                    kind,
                    cwd,
                    web_title,
                    label_ctx.home.as_deref(),
                    label_ctx.max_chars,
                ),
                is_active: ae == active_surface,
            })
        })
        .collect();

    // NOTE: `is_active_pane` is always true in a single-workspace view. A
    // multi-workspace tab indicator would require knowing the workspace-level
    // active pane, which is not threaded here yet. Safe default: treat
    // every pane as the active one (solid accent indicator).
    build_pane_tab_bar(commands, pane_frame, pane_entity, &tabs, true, ui_font);

    let surface_slot = commands
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

    for &surface_entity in &surface_entities {
        let Ok((kind, _name, _cwd, _web_title)) = surfaces.get(surface_entity) else {
            continue;
        };
        decorate_surface(commands, surface_entity, kind);
        if surface_entity == active_surface {
            commands
                .entity(surface_entity)
                .insert((ChildOf(surface_slot), Slotted));
        } else {
            commands
                .entity(surface_entity)
                .insert(ChildOf(inactive_host_parent))
                .remove::<Slotted>();
        }
    }

    // NOTE: terminal panes are dimmed at the renderer (PaneDim uniform), so
    // they must NOT also get the veil — double-dimming would over-darken their
    // content. The veil is for non-terminal (e.g. webview) panes only.
    let active_is_terminal = matches!(
        surfaces.get(active_surface).map(|(kind, _, _, _)| kind),
        Ok(SurfaceKind::Terminal)
    );
    if let Some(veil_color) = veil
        && !active_is_terminal
    {
        let visibility = if pane_entity == active_pane {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
        commands.spawn((
            Name::new(format!("PaneDim({pane_entity:?})")),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(veil_color),
            visibility,
            Pickable::IGNORE,
            StructuralNode,
            PaneDimOverlay { pane: pane_entity },
            ChildOf(pane_frame),
        ));
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
