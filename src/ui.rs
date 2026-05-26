//! Bevy UI Plugin and rebuild systems. Per-session UI subtrees are owned
//! by their Session entity and rebuilt via
//! `rebuild_session::rebuild_session_ui_on_data_change` whenever the
//! per-session epoch in `MultiplexerService` advances. The status bar
//! rebuilds independently via
//! `status_bar_sync::rebuild_status_bar_on_session_set_change` when the
//! session list or `AttachedSession` marker changes. Activity host
//! entities (managed by `ActivityEntityRegistry`) are kept stable across
//! rebuilds and re-parented via `ChildOf` — active hosts under the
//! active session's pane slot, inactive hosts under the owning Session
//! entity (a non-Node walker-skipped park).

use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::terminal::OzmuxTerminalUiPlugin;
use bevy::prelude::*;

pub(crate) mod activity;
pub(crate) mod copy_mode;
pub(crate) mod copy_mode_indicator;
pub(crate) mod layout;
pub(crate) mod palette;
pub(crate) mod rebuild_session;
pub(crate) mod registry;
pub(crate) mod root;
pub(crate) mod status_bar;
pub(crate) mod status_bar_sync;
pub(crate) mod sync_session;
pub(crate) mod tab_bar;
pub(crate) mod terminal;

/// Marker for the single root UI Node entity. Spawned once in Startup,
/// never despawned. Hosts `SessionUiRoot` (the attachment point for the
/// active session) and `StatusBarRoot` as direct children.
#[derive(Component)]
pub(crate) struct UiRoot;

/// Marker for the single attachment-point `Node` child of `UiRoot` that
/// receives whichever Session's `SessionUiSubtree` is currently attached.
/// `sync_active_session` reparents subtrees between this and their owning
/// Session entity. Spawned once in Startup; never despawned.
#[derive(Component)]
pub(crate) struct SessionUiRoot;

/// Marker for every transient UI Node (status bar, tab bar, pane frame,
/// split container, placeholder activity content). Rebuilds query this
/// and despawn every match. Activity host entities must NOT carry this.
#[derive(Component)]
pub(crate) struct StructuralNode;

/// Marker for the stable per-activity host entity. Survives structural
/// rebuilds; re-parented via `ChildOf` each rebuild. The `ActivityId →
/// Entity` mapping is owned by `ActivityEntityRegistry`; this marker
/// exists only so queries can filter for activity hosts.
#[derive(Component)]
pub(crate) struct ActivityHostNode;

/// Marks an Activity Host whose `kind` is `Terminal`. `finish_terminal_setup`
/// queries for `With<TerminalActivityMarker>` to find hosts that need a
/// `TerminalBundle` + `TerminalRenderBundle` attached.
#[derive(Component)]
pub(crate) struct TerminalActivityMarker;

/// Records that `TerminalBundle::spawn` failed for this host, so
/// `finish_terminal_setup` will not retry on subsequent frames.
#[derive(Component)]
pub(crate) struct TerminalSpawnFailed;

/// Marker for the pane frame Node (the outermost Node of one
/// `Cell::Pane` subtree). Used by tests; not load-bearing for runtime.
#[derive(Component)]
pub(crate) struct PaneFrame;

/// Bevy Plugin wiring the native Bevy UI rebuild pipeline.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActivityEntityRegistry>()
            .add_plugins(OzmuxTerminalUiPlugin)
            .add_systems(Startup, root::setup_root_camera_and_ui_root)
            .add_systems(
                Update,
                (
                    rebuild_session::rebuild_session_ui_on_data_change,
                    status_bar_sync::rebuild_status_bar_on_session_set_change,
                ),
            )
            .add_systems(
                PostUpdate,
                sync_session::sync_active_session.before(bevy::ui::UiSystems::Prepare),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::OzmuxBootstrapPlugin;
    use crate::configs::OzmuxConfigsPlugin;
    use crate::multiplexer::{
        AttachedSession, Multiplexer, OzmuxMultiplexerPlugin, SessionEntityId,
    };
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::render::storage::ShaderStorageBuffer;
    use bevy::window::{PrimaryWindow, WindowResolution};
    use bevy_terminal_renderer::material::TerminalUiMaterial;
    use bevy_terminal_renderer::{CellMetrics, TerminalCellMetricsResource};
    use ozmux_multiplexer::ActivityId;

    fn make_test_app() -> (App, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }

        // NOTE: `finish_terminal_setup` takes `ResMut<Assets<TerminalUiMaterial>>`,
        // so `Assets<TerminalUiMaterial>` (and its `ShaderStorageBuffer`
        // dependency) must exist as resources before `OzmuxUiPlugin` runs.
        // Production wires this via `TerminalRendererPlugin`; the headless
        // tests register the assets directly to avoid the WGPU stack.
        // `resize_terminals_to_node` requires `TerminalCellMetricsResource`;
        // production inserts it via `TerminalFontPlugin` (inside
        // `TerminalRendererPlugin`). Insert a DPR=1 / 12px fallback here so
        // headless tests don't panic on "Resource does not exist".
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
            .add_plugins(OzmuxMultiplexerPlugin)
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
    fn rebuild_after_bootstrap_spawns_structural_and_pane_frame() {
        let (mut app, _guard) = make_test_app();
        // NOTE: two `app.update()` calls are required here (and in every test that
        // needs a visible rebuild): the first tick runs Startup systems (bootstrap +
        // setup_root_camera_and_ui_root); the second tick runs the first Update pass
        // where `rebuild_session_ui_on_data_change` fires because the bootstrap
        // bumped the per-session epoch.
        app.update();
        app.update();

        let world = app.world_mut();
        let structural_count = world
            .query_filtered::<Entity, With<StructuralNode>>()
            .iter(world)
            .count();
        let pane_frame_count = world
            .query_filtered::<Entity, With<PaneFrame>>()
            .iter(world)
            .count();

        assert!(
            structural_count > 0,
            "expected structural nodes after bootstrap"
        );
        assert_eq!(
            pane_frame_count, 1,
            "expected exactly one pane frame after bootstrap"
        );
    }

    #[test]
    fn activity_entity_persists_across_rebuild() {
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let first_snapshot: Vec<(ActivityId, Entity)> = {
            let registry = app.world().resource::<ActivityEntityRegistry>();
            registry
                .iter()
                .map(|(id, entity)| (id.clone(), entity))
                .collect()
        };
        assert!(
            !first_snapshot.is_empty(),
            "expected at least one Activity host after bootstrap"
        );

        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let sid = *mux.sessions.keys().next().expect("session");
            mux.rename_session(&sid, "renamed".into()).expect("rename");
            mux.bump_epoch(&sid);
        }
        app.update();

        let registry = app.world().resource::<ActivityEntityRegistry>();
        for (id, entity_before) in &first_snapshot {
            let entity_after = registry.get(id).expect("registry retained id");
            assert_eq!(
                *entity_before, entity_after,
                "Activity Entity for {id} must be stable across rebuilds (flicker contract)"
            );
        }

        let world = app.world();
        for (_id, entity_before) in &first_snapshot {
            let parent = world
                .get::<ChildOf>(*entity_before)
                .expect("host must be re-parented after rebuild — orphan would mean ChildOf was removed but not re-attached");
            assert!(
                world
                    .get::<crate::ui::StructuralNode>(parent.parent())
                    .is_some(),
                "host's new parent must be a StructuralNode (the new activity_slot)"
            );
        }
    }

    #[test]
    fn split_pane_produces_two_pane_frames() {
        use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let (sid, active_pane) = {
                let (sid, session) = mux.sessions.iter().next().expect("session");
                (*sid, session.active_pane.clone())
            };
            let new_pane_id = PaneId::new();
            let new_activity = Activity::terminal(ActivityId::new());
            mux.with_session(&sid, |s| {
                s.split_pane(
                    &active_pane,
                    new_pane_id,
                    new_activity,
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .expect("with_session returned Some")
            .expect("split_pane Ok");
            mux.bump_epoch(&sid);
        }
        app.update();

        let world = app.world_mut();
        let pane_frame_count = world
            .query_filtered::<Entity, With<PaneFrame>>()
            .iter(world)
            .count();
        assert_eq!(
            pane_frame_count, 2,
            "expected 2 pane frames after one split"
        );
    }

    #[test]
    fn activity_registry_prunes_removed_activity() {
        use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let new_pane_id = PaneId::new();
        let new_activity_id = ActivityId::new();
        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let (sid, active_pane) = {
                let (sid, session) = mux.sessions.iter().next().expect("session");
                (*sid, session.active_pane.clone())
            };
            mux.with_session(&sid, |s| {
                s.split_pane(
                    &active_pane,
                    new_pane_id.clone(),
                    Activity::terminal(new_activity_id.clone()),
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .expect("with_session")
            .expect("split_pane");
            mux.bump_epoch(&sid);
        }
        app.update();

        {
            let registry = app.world().resource::<ActivityEntityRegistry>();
            assert!(
                registry.get(&new_activity_id).is_some(),
                "newly-spawned Activity must be in registry"
            );
        }

        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let sid = *mux.sessions.keys().next().expect("session");
            mux.with_session(&sid, |s| s.close_pane(&new_pane_id))
                .expect("with_session")
                .expect("close_pane");
            mux.bump_epoch(&sid);
        }
        app.update();

        let registry = app.world().resource::<ActivityEntityRegistry>();
        assert!(
            registry.get(&new_activity_id).is_none(),
            "closed Activity must be pruned from registry"
        );
    }

    #[test]
    fn activity_host_not_caught_in_despawn_cascade() {
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let entity_before = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, With<ActivityHostNode>>();
            q.iter(world).next().expect("at least one host")
        };

        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let sid = *mux.sessions.keys().next().expect("session");
            mux.rename_session(&sid, "second-rename".into())
                .expect("rename");
            mux.bump_epoch(&sid);
        }
        app.update();

        assert!(
            app.world().get_entity(entity_before).is_ok(),
            "host entity must still exist after rebuild — load-bearing for stable handles"
        );
    }

    #[test]
    fn focus_session_switch_does_not_orphan_inactive_session_hosts() {
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let bootstrap_aid = {
            let mux = app.world().resource::<Multiplexer>();
            let (_sid, session) = mux.sessions.iter().next().expect("session");
            let pane = session.pane(&session.active_pane).expect("pane");
            pane.active_activity.clone()
        };

        // Mint a second session (entity + SessionUiSubtree included so
        // sync_active_session can park subtrees back to their owners).
        let second_sid = {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let (sid, _, _) = mux.create_session(Some("second".into()));
            mux.bump_epoch(&sid);
            sid
        };
        let second_subtree = app.world_mut().spawn(Node::default()).id();
        let second_entity = app
            .world_mut()
            .spawn((
                SessionEntityId(second_sid),
                crate::multiplexer::SessionUiSubtree(second_subtree),
                Name::new("Session:second"),
            ))
            .id();
        app.world_mut()
            .entity_mut(second_subtree)
            .insert(ChildOf(second_entity));
        app.update();

        // Swap AttachedSession to the second session entity.
        let bootstrap_entity = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, With<AttachedSession>>();

            q.single(world).expect("exactly one attached")
        };
        app.world_mut()
            .entity_mut(bootstrap_entity)
            .remove::<AttachedSession>();
        app.world_mut()
            .entity_mut(second_entity)
            .insert(AttachedSession);
        {
            let mut mux = app.world_mut().resource_mut::<Multiplexer>();
            mux.set_changed();
        }
        app.update();

        let registry = app.world().resource::<ActivityEntityRegistry>();
        let bootstrap_host = registry
            .get(&bootstrap_aid)
            .expect("bootstrap activity host must remain in registry across session switch");

        // Walk up the ChildOf chain from the host. Must terminate at a
        // Session entity (which carries SessionEntityId and has no Node).
        let mut cursor = bootstrap_host;
        let final_parent = loop {
            match app.world().get::<ChildOf>(cursor) {
                Some(c) => cursor = c.parent(),
                None => break cursor,
            }
        };
        assert!(
            app.world().get::<SessionEntityId>(final_parent).is_some(),
            "host's chain must terminate at a Session entity",
        );
        assert!(
            app.world().get::<bevy::ui::Node>(final_parent).is_none(),
            "Session entity must not carry Node (walker-skip)",
        );
    }

    #[test]
    fn inactive_activity_host_persists_across_focus_switch() {
        use ozmux_multiplexer::Activity;

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        // Add a SECOND Activity to the bootstrap pane and capture both
        // ActivityIds.
        let (bootstrap_id, second_id) = {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let sid = *mux.sessions.keys().next().expect("session");
            let (active_pane, bootstrap_aid) = {
                let session = mux.sessions.get(&sid).expect("session");
                let active_pane = session.active_pane.clone();
                let pane = session.pane(&active_pane).expect("pane");
                (active_pane, pane.active_activity.clone())
            };
            let second_aid = ActivityId::new();
            mux.with_session(&sid, |s| {
                s.pane_mut(&active_pane)
                    .expect("pane_mut")
                    .add_activity(Activity::terminal(second_aid.clone()))
            })
            .expect("with_session")
            .expect("add_activity");
            mux.bump_epoch(&sid);
            (bootstrap_aid, second_aid)
        };
        app.update();

        // Both Activity hosts must be in the registry — even though only
        // one is the active tab.
        let (bootstrap_entity, second_entity) = {
            let registry = app.world().resource::<ActivityEntityRegistry>();
            let b = registry.get(&bootstrap_id).expect(
                "bootstrap activity host must remain in registry while inactive sibling exists",
            );
            let s = registry
                .get(&second_id)
                .expect("newly-added activity host must be in registry");
            (b, s)
        };

        // Switch focus to the second activity.
        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let sid = *mux.sessions.keys().next().expect("session");
            let active_pane = mux.sessions.get(&sid).expect("session").active_pane.clone();
            let _outcome = mux
                .with_session(&sid, |s| {
                    s.pane_mut(&active_pane)
                        .expect("pane_mut")
                        .set_active_activity(&second_id)
                })
                .expect("with_session")
                .expect("set_active_activity");
            mux.bump_epoch(&sid);
        }
        app.update();

        // The ORIGINAL (now inactive) host Entity must survive the
        // focus-switch rebuild. Without the domain-driven
        // `live_activity_ids`, `registry.prune` would despawn it and the
        // terminal's PTY child + alacritty grid + scrollback would be
        // lost — the bug this test guards against.
        let registry = app.world().resource::<ActivityEntityRegistry>();
        assert_eq!(
            registry.get(&bootstrap_id),
            Some(bootstrap_entity),
            "inactive Activity must keep the SAME host Entity after focus switches away from it"
        );
        assert_eq!(
            registry.get(&second_id),
            Some(second_entity),
            "newly-active Activity must keep the SAME host Entity (no respawn)"
        );
        assert!(
            app.world().get_entity(bootstrap_entity).is_ok(),
            "inactive host Entity must still exist in the world"
        );

        // Inactive host MUST be parented to the owning Session entity. The
        // Session entity carries `SessionEntityId` but no `Node`, so it falls
        // outside Bevy's UI walker (`UiChildren::iter_ui_children` filters
        // `With<Node>`) — the inactive host's subtree is layout-skipped
        // entirely, no `Display::None` workaround needed.
        let bootstrap_parent = app
            .world()
            .get::<ChildOf>(bootstrap_entity)
            .expect("inactive host must have a parent (the owning Session entity)")
            .parent();
        assert!(
            app.world()
                .get::<SessionEntityId>(bootstrap_parent)
                .is_some(),
            "inactive host's parent must be the owning Session entity",
        );
        assert!(
            app.world()
                .get::<bevy::ui::Node>(bootstrap_parent)
                .is_none(),
            "Session entity must not carry Node (the walker-skip invariant)",
        );

        // Toggle focus back and forth several times — this exercises
        // the case the original taffy "invalid SlotMap key used" panic
        // was reproduced under (alternating focus between two terminal
        // Activities). The hierarchy must stay valid each frame:
        // both hosts have valid parents, the previously-active host
        // is parked under the Session entity (non-Node, walker-skipped),
        // the newly-active host moves to the visible activity slot.
        for target_id in [&bootstrap_id, &second_id, &bootstrap_id, &second_id] {
            {
                let world = app.world_mut();
                let mut mux = world.resource_mut::<Multiplexer>();
                let sid = *mux.sessions.keys().next().expect("session");
                let active_pane = mux.sessions.get(&sid).expect("session").active_pane.clone();
                let _outcome = mux
                    .with_session(&sid, |s| {
                        s.pane_mut(&active_pane)
                            .expect("pane_mut")
                            .set_active_activity(target_id)
                    })
                    .expect("with_session")
                    .expect("set_active_activity");
                mux.bump_epoch(&sid);
            }
            app.update();

            // After every switch both hosts must still be alive and
            // have a valid parent.
            for (id, expected_entity) in [
                (&bootstrap_id, bootstrap_entity),
                (&second_id, second_entity),
            ] {
                let registry = app.world().resource::<ActivityEntityRegistry>();
                assert_eq!(
                    registry.get(id),
                    Some(expected_entity),
                    "host Entity for {id} must stay stable across focus toggles"
                );
                assert!(
                    app.world().get::<ChildOf>(expected_entity).is_some(),
                    "host {id} must have a ChildOf every frame (active = activity_slot, inactive = Session entity)"
                );
            }
        }
    }
}
