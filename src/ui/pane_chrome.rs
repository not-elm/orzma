//! Per-pane stable chrome containers. `PaneChrome` records the two UI
//! entities (tab-bar root + activity slot) that survive geometry rebuilds
//! for one Pane, so tab/host content is not torn down on every layout
//! change. `sync_pane_activities` reacts to per-pane component changes
//! (`Changed<Children>` / `Changed<ActiveActivity>`) to rebuild a pane's tab
//! bar, mount its active host, and manage its dim veil — independently of the
//! geometry rebuild. `despawn_pane_chrome_on_pane_removal` despawns the
//! containers when the Pane closes (they are not children of the despawned
//! pane frame, so they need explicit cleanup).

use crate::configs::OzmuxConfigsResource;
use crate::font::TerminalUiFont;
use crate::ui::activity::build_activity_host_children;
use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::tab_bar::{TabEntry, populate_tab_bar};
use crate::ui::{ActivityChromeRoot, ActivityHostNode, PaneDimOverlay};
use bevy::prelude::*;
use bevy::ui::{PositionType, Val};
use ozmux_multiplexer::{
    ActiveActivity, ActivePane, ActivityKind, ActivityMarker, PaneMarker, SessionMarker,
};

/// The stable tab-bar root and activity slot for one Pane. Inserted lazily on
/// the Pane entity by the chrome systems; read by the geometry rebuild (to
/// reparent the containers under the new pane frame) and by
/// `sync_pane_activities` (to fill them).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct PaneChrome {
    /// The stable Row node that holds one tab per activity.
    pub(crate) tab_bar_root: Entity,
    /// The stable node the active activity's host is parented under.
    pub(crate) activity_slot: Entity,
}

/// Despawns a closed Pane's stable chrome containers. Driven by
/// `On<Remove, PaneMarker>` so it reads `PaneChrome` while the Pane still
/// exists.
pub(crate) fn despawn_pane_chrome_on_pane_removal(
    ev: On<Remove, PaneMarker>,
    chromes: Query<&PaneChrome>,
    mut commands: Commands,
) {
    if let Ok(chrome) = chromes.get(ev.entity) {
        commands.entity(chrome.tab_bar_root).despawn();
        commands.entity(chrome.activity_slot).despawn();
    }
}

/// Rebuilds one pane's tab bar, mounts its active activity host, and manages
/// its dim veil whenever the pane's activity set (`Changed<Children>`) or
/// active activity (`Changed<ActiveActivity>`) changes — independently of the
/// geometry rebuild. Reacts to the model's real component changes (no
/// `set_changed`). The tab entities and veil are tagged `ActivityChromeRoot`
/// (not `StructuralNode`) so they ride along on the stable `PaneChrome`
/// containers across a geometry rebuild and are owned solely by this system.
#[expect(
    clippy::too_many_arguments,
    reason = "chrome sync needs the registry, several pane/activity/session queries, and the config + font resources; splitting would obscure the single-pass lifecycle"
)]
pub(crate) fn sync_pane_activities(
    mut commands: Commands,
    mut registry: ResMut<ActivityEntityRegistry>,
    changed_panes: Query<
        (Entity, &Children, &ActiveActivity, &ChildOf, &PaneChrome),
        (
            With<PaneMarker>,
            Or<(Changed<Children>, Changed<ActiveActivity>)>,
        ),
    >,
    activities: Query<(&ActivityKind, &Name), With<ActivityMarker>>,
    host_parents: Query<&ChildOf, With<ActivityHostNode>>,
    sessions_active: Query<&ActivePane, With<SessionMarker>>,
    tab_children: Query<&Children>,
    chrome_tabs: Query<(), With<ActivityChromeRoot>>,
    ui_font: Option<Res<TerminalUiFont>>,
    configs: Option<Res<OzmuxConfigsResource>>,
    veils: Query<(Entity, &PaneDimOverlay)>,
) {
    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();

    for (pane, children, active, pane_parent, chrome) in changed_panes.iter() {
        let session = pane_parent.parent();
        let is_active_pane = sessions_active
            .get(session)
            .map(|a| a.0 == pane)
            .unwrap_or(true);

        if let Ok(existing) = tab_children.get(chrome.tab_bar_root) {
            for child in existing.iter() {
                if chrome_tabs.get(child).is_ok() {
                    commands.entity(child).despawn();
                }
            }
        }
        let tabs: Vec<TabEntry> = children
            .iter()
            .filter_map(|ae| {
                let (_, name) = activities.get(ae).ok()?;
                Some(TabEntry {
                    entity: ae,
                    name: name.as_str().to_string(),
                    is_active: ae == active.0,
                })
            })
            .collect();
        populate_tab_bar(
            &mut commands,
            chrome.tab_bar_root,
            &tabs,
            is_active_pane,
            &ui_font_handle,
        );

        for ae in children.iter() {
            let Ok((kind, name)) = activities.get(ae) else {
                continue;
            };
            let host = registry.get_or_spawn(&mut commands, ae, kind);
            build_activity_host_children(&mut commands, host, kind, name);
            let target = if ae == active.0 {
                chrome.activity_slot
            } else {
                session
            };
            let current = host_parents.get(host).ok().map(|c| c.parent());
            if current != Some(target) {
                commands.entity(host).insert(ChildOf(target));
            }
        }

        sync_pane_veil(
            &mut commands,
            pane,
            chrome.activity_slot,
            is_active_pane,
            &activities,
            active.0,
            configs.as_deref(),
            &veils,
        );
    }
}

/// Spawns or despawns the pane's dim veil under its `activity_slot` so the
/// veil rides on the stable container across geometry rebuilds. The veil is
/// present only when dimming is enabled and the active activity is NOT a
/// terminal (terminal panes dim at the renderer via `PaneDim`; a veil would
/// double-dim them). It is tagged `ActivityChromeRoot` (not `StructuralNode`)
/// for the same survive-the-rebuild reason as the tabs.
#[expect(
    clippy::too_many_arguments,
    reason = "veil presence depends on the pane, slot, focus, active activity kind, and config; threading these is clearer than packing them into a struct"
)]
fn sync_pane_veil(
    commands: &mut Commands,
    pane: Entity,
    activity_slot: Entity,
    is_active_pane: bool,
    activities: &Query<(&ActivityKind, &Name), With<ActivityMarker>>,
    active_activity: Entity,
    configs: Option<&OzmuxConfigsResource>,
    veils: &Query<(Entity, &PaneDimOverlay)>,
) {
    let active_is_terminal = matches!(
        activities.get(active_activity).map(|(k, _)| k),
        Ok(ActivityKind::Terminal)
    );
    let existing_veil = veils.iter().find(|(_, v)| v.pane == pane).map(|(e, _)| e);
    let want_veil =
        configs.map(|c| c.inactive_pane.enabled).unwrap_or(false) && !active_is_terminal;

    match (want_veil, existing_veil) {
        (true, None) => {
            let cfg = configs.expect("want_veil implies configs is Some");
            let (r, g, b) = cfg.inactive_pane.rgb();
            let color = Color::srgb_u8(r, g, b).with_alpha(cfg.inactive_pane.opacity);
            let visibility = if is_active_pane {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
            commands.spawn((
                Name::new(format!("PaneDim({pane:?})")),
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(0.0),
                    left: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(color),
                visibility,
                Pickable::IGNORE,
                ActivityChromeRoot,
                PaneDimOverlay { pane },
                ChildOf(activity_slot),
            ));
        }
        (false, Some(v)) => {
            commands.entity(v).despawn();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::MinimalPlugins;
    use bevy::app::App;

    #[test]
    fn observer_despawns_chrome_containers_when_pane_is_removed() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(despawn_pane_chrome_on_pane_removal);

        let tab_bar = app.world_mut().spawn(Node::default()).id();
        let slot = app.world_mut().spawn(Node::default()).id();
        let pane = app
            .world_mut()
            .spawn((
                PaneMarker,
                PaneChrome {
                    tab_bar_root: tab_bar,
                    activity_slot: slot,
                },
            ))
            .id();

        app.world_mut().despawn(pane);
        app.world_mut().flush();

        assert!(
            app.world().get_entity(tab_bar).is_err(),
            "tab_bar_root must be despawned on pane removal"
        );
        assert!(
            app.world().get_entity(slot).is_err(),
            "activity_slot must be despawned on pane removal"
        );
    }

    use crate::bootstrap::OzmuxBootstrapPlugin;
    use crate::configs::OzmuxConfigsPlugin;
    use crate::ui::OzmuxUiPlugin;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::render::storage::ShaderStorageBuffer;
    use bevy::window::{PrimaryWindow, WindowResolution};
    use bevy_terminal_renderer::material::TerminalUiMaterial;
    use bevy_terminal_renderer::{CellMetrics, TerminalCellMetricsResource};
    use ozmux_multiplexer::{AttachedSession, MultiplexerPlugin};

    fn make_ui_test_app() -> (App, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(ImagePlugin::default())
            .init_asset::<TerminalUiMaterial>()
            .init_asset::<ShaderStorageBuffer>()
            .insert_resource(TerminalCellMetricsResource {
                metrics: CellMetrics {
                    advance_phys: 8.0,
                    line_height_phys: 16.0,
                    ascent_phys: 12.0,
                    descent_phys: 4.0,
                    underline_position_phys: -2.0,
                    underline_thickness_phys: 1.0,
                    max_overflow_phys: 0.0,
                },
                phys_font_size: 12,
            })
            .add_plugins(MultiplexerPlugin)
            .add_plugins(OzmuxConfigsPlugin)
            .add_plugins(OzmuxBootstrapPlugin)
            .add_plugins(OzmuxUiPlugin);

        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));

        (app, guard)
    }

    #[test]
    fn adding_in_pane_activity_renders_tab_and_mounts_host_without_layout_change() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::MultiplexerCommands;

        let (mut app, _guard) = make_ui_test_app();
        app.update();
        app.update();

        let pane = app
            .world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next()?;
                    mux.sessions_active_pane(session)
                },
            )
            .unwrap()
            .expect("bootstrap session + pane");

        // Capture the pane frame BEFORE the activity change. A geometry rebuild
        // would despawn + respawn it (a new Entity); proving it is the SAME
        // Entity afterward proves no rebuild ran and the tab/host appeared
        // purely via sync_pane_activities.
        let pane_frame_before = app
            .world_mut()
            .query_filtered::<Entity, With<crate::ui::PaneFrame>>()
            .single(app.world())
            .expect("exactly one pane frame after bootstrap");

        // Add an extension activity and activate it WITHOUT touching LayoutCells.
        // sync_pane_activities must react to Changed<Children>/Changed<ActiveActivity>.
        let ext = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                let ext = mux.add_activity(
                    pane,
                    ActivityKind::Extension {
                        entry: "/tmp".into(),
                    },
                );
                mux.set_active_activity(pane, ext).unwrap();
                ext
            })
            .unwrap();
        app.world_mut().flush();
        app.update();

        let pane_frame_after = app
            .world_mut()
            .query_filtered::<Entity, With<crate::ui::PaneFrame>>()
            .single(app.world())
            .expect("exactly one pane frame after the chrome sync");
        assert_eq!(
            pane_frame_after, pane_frame_before,
            "the pane frame Entity must be unchanged — no geometry rebuild ran \
             (no LayoutCells change), so the tab + host appeared via sync_pane_activities alone"
        );

        let chrome = *app
            .world()
            .get::<PaneChrome>(pane)
            .expect("pane has PaneChrome after a chrome sync");

        let ext_host = app
            .world()
            .resource::<ActivityEntityRegistry>()
            .get(ext)
            .expect("extension activity has a host");
        let host_parent = app.world().get::<ChildOf>(ext_host).map(|c| c.parent());
        assert_eq!(
            host_parent,
            Some(chrome.activity_slot),
            "active extension host must be mounted under the pane's activity_slot"
        );

        let tab_count = app
            .world()
            .get::<Children>(chrome.tab_bar_root)
            .map(|kids| {
                kids.iter()
                    .filter(|c| app.world().get::<ActivityChromeRoot>(*c).is_some())
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(
            tab_count, 2,
            "tab_bar_root must have one ActivityChromeRoot tab per activity (terminal + ext)"
        );
    }
}
