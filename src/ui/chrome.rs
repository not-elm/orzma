//! Incremental per-pane chrome. Replaces the deleted full-rebuild path:
//! `build_pane_chrome` spawns each pane's tab bar + surface slot exactly once
//! on `Added<PaneMarker>`; `slot_active_surface` slots/parks surfaces on
//! `Changed<ActiveSurface>` (via the `Slotted` marker); `refresh_pane_tabs`
//! rebuilds only the affected pane's tab labels on surface / `Cwd` changes.

use crate::font::TerminalUiFont;
use crate::system_set::OzmuxSystems;
use crate::theme;
use crate::ui::surface::decorate_surface;
use crate::ui::tab_bar::{TabEntry, build_tab};
use crate::ui::tab_label::{LabelCtx, tab_label};
use crate::ui::{HomeDir, PaneFrame, Slotted, palette};
use bevy::prelude::*;
use bevy::ui::{UiRect, Val};
use ozmux_multiplexer::{
    ActiveSurface, Cwd, OwningWorkspace, PaneMarker, SurfaceMarker, SurfaceOf, Surfaces,
};
use std::collections::HashSet;

/// Bevy Plugin wiring the incremental chrome systems.
pub(crate) struct OzmuxChromePlugin;

impl Plugin for OzmuxChromePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, build_pane_chrome.in_set(OzmuxSystems::BuildChrome))
            .add_systems(
                Update,
                (slot_active_surface, refresh_pane_tabs).after(OzmuxSystems::BuildChrome),
            );
    }
}

/// Per-pane chrome handles, recorded on the Pane entity by
/// `build_pane_chrome`. `tab_bar` is the tab-bar row Node (its children are the
/// per-surface tabs, rebuilt by `refresh_pane_tabs`); `surface_slot` is the
/// bordered slot Node into which the active surface is parented (`Slotted`).
#[derive(Component)]
pub(crate) struct PaneChrome {
    tab_bar: Entity,
    surface_slot: Entity,
}

/// Builds the tab bar + surface slot for each newly-added Pane exactly once,
/// recording the handles in `PaneChrome`. Runs in `OzmuxSystems::BuildChrome`,
/// after the structural `ApplyDeferred`, so the Pane entity (and its own
/// `Node`) is fully committed before chrome attaches. Spawns only
/// self-contained children of the Pane — it never reads the Pane's parent or
/// siblings (Bevy early-hierarchy hazard #18671).
fn build_pane_chrome(
    mut commands: Commands,
    new_panes: Query<Entity, (Added<PaneMarker>, Without<PaneChrome>)>,
) {
    for pane in new_panes.iter() {
        let tab_bar = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    width: Val::Percent(100.0),
                    height: Val::Auto,
                    padding: UiRect::ZERO,
                    ..default()
                },
                BackgroundColor(palette::TAB_BAR_BG),
                ChildOf(pane),
            ))
            .id();

        let surface_slot = commands
            .spawn((
                Node {
                    flex_grow: 1.0,
                    border: UiRect::all(Val::Px(theme::PANE_BORDER_PX)),
                    width: Val::Percent(100.0),
                    ..default()
                },
                BorderColor::all(palette::BORDER),
                ChildOf(pane),
            ))
            .id();

        commands.entity(pane).insert((
            PaneFrame,
            BackgroundColor(palette::BACKGROUND),
            PaneChrome {
                tab_bar,
                surface_slot,
            },
        ));
    }
}

/// Slots the active surface into its pane's `surface_slot` and parks the
/// previously-active surface under the owning Workspace (a non-`Node` parent,
/// so the parked surface falls out of Bevy's UI walker). Fires on
/// `Changed<ActiveSurface>` — which covers both a surface switch and the
/// initial `ActiveSurface` set at pane creation. Decorates each surface (its
/// `Node` + `TerminalSurfaceMarker` + material attach point) on first slotting.
///
/// At most one surface per pane carries `Slotted`; the previously-slotted one
/// is found by scanning `Slotted` surfaces whose `SurfaceOf` is this pane.
fn slot_active_surface(
    mut commands: Commands,
    switched_panes: Query<
        (Entity, &ActiveSurface, &OwningWorkspace, &PaneChrome),
        Changed<ActiveSurface>,
    >,
    slotted: Query<(Entity, &SurfaceOf), With<Slotted>>,
) {
    for (pane, active, owning, chrome) in switched_panes.iter() {
        let new_surface = active.0;
        for (parked, owner) in slotted.iter() {
            if owner.0 == pane && parked != new_surface {
                commands
                    .entity(parked)
                    .remove::<Slotted>()
                    .insert(ChildOf(owning.0));
            }
        }
        decorate_surface(&mut commands, new_surface);
        commands
            .entity(new_surface)
            .insert((ChildOf(chrome.surface_slot), Slotted));
    }
}

/// Rebuilds a single pane's tab labels when its surface set changes
/// (`Added`/removed `SurfaceMarker` in the pane), its active surface switches,
/// or a slotted surface reports a new `Cwd` (OSC 7). Despawns the pane's
/// existing tabs and re-spawns one per owned surface — the tab-bar row Node
/// itself (in `PaneChrome.tab_bar`) is reused. Scopes work to the affected
/// panes; no full rebuild.
fn refresh_pane_tabs(
    mut commands: Commands,
    panes: Query<(Entity, &ActiveSurface, &Surfaces, &PaneChrome), With<PaneMarker>>,
    changed_active: Query<Entity, (With<PaneMarker>, Changed<ActiveSurface>)>,
    added_surfaces: Query<&SurfaceOf, Added<SurfaceMarker>>,
    changed_cwd: Query<&SurfaceOf, (With<SurfaceMarker>, Changed<Cwd>)>,
    children: Query<&Children>,
    surface_data: Query<Option<&Cwd>, With<SurfaceMarker>>,
    ui_font: Option<Res<TerminalUiFont>>,
    home_dir: Option<Res<HomeDir>>,
) {
    let mut dirty: HashSet<Entity> = changed_active.iter().collect();
    dirty.extend(
        added_surfaces
            .iter()
            .chain(changed_cwd.iter())
            .map(|owner| owner.0),
    );
    if dirty.is_empty() {
        return;
    }

    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();
    let label_ctx = LabelCtx {
        home: home_dir.and_then(|h| h.0.clone()),
        max_chars: theme::TAB_LABEL_MAX_CHARS,
    };

    for pane in dirty {
        let Ok((active, surfaces, chrome)) = panes.get(pane).map(|(_, a, s, c)| (a.0, s, c)) else {
            continue;
        };
        if let Ok(existing) = children.get(chrome.tab_bar) {
            for tab in existing.iter() {
                commands.entity(tab).despawn();
            }
        }
        for surface in surfaces.iter() {
            let Ok(cwd) = surface_data.get(surface) else {
                continue;
            };
            let entry = TabEntry {
                entity: surface,
                name: tab_label(cwd, label_ctx.home.as_deref(), label_ctx.max_chars),
                is_active: surface == active,
            };
            // NOTE: `is_active_pane` is always true in a single-workspace view;
            // the workspace-level active pane is not threaded here yet, so every
            // pane gets the solid accent indicator on its active tab.
            build_tab(
                &mut commands,
                chrome.tab_bar,
                pane,
                &entry,
                true,
                &ui_font_handle,
            );
        }
    }
}
