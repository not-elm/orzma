//! Bevy UI Plugin and the rebuild system. The structural shell
//! (status bar / tab bars / pane frames / split containers) is despawned
//! and rebuilt whenever `Multiplexer` or the primary window's
//! `AttachedSession` changes. Activity host entities (managed by
//! `ActivityEntityRegistry`) are kept stable across rebuilds and
//! re-parented via `ChildOf`.

use crate::multiplexer::{AttachedSession, Multiplexer};
use crate::ui::registry::ActivityEntityRegistry;
use bevy::ecs::change_detection::DetectChanges;
use bevy::prelude::*;
use ozmux_multiplexer::{ActivityId, Cell};
use std::collections::HashSet;

pub(crate) mod activity;
pub(crate) mod layout;
pub(crate) mod palette;
pub(crate) mod registry;
pub(crate) mod root;
pub(crate) mod status_bar;
pub(crate) mod tab_bar;

/// Marker for the single root UI Node entity. Spawned once in Startup,
/// never despawned; rebuilds replace its descendants only.
#[derive(Component)]
pub(crate) struct UiRoot;

/// Marker for every transient UI Node (status bar, tab bar, pane frame,
/// split container, placeholder activity content). Rebuilds query this
/// and despawn every match. Activity host entities must NOT carry this.
#[derive(Component)]
pub(crate) struct StructuralNode;

/// Marker for the stable per-activity host entity. Carries the
/// ActivityId for registry reverse lookup. Survives structural
/// rebuilds; re-parented via `ChildOf` each rebuild.
#[derive(Component)]
pub(crate) struct ActivityHostNode(
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "runtime reverse lookup planned for input dispatch; read by integration tests"
        )
    )]
    pub(crate) ActivityId,
);

/// Marker for the pane frame Node (the outermost Node of one
/// `Cell::Pane` subtree). Used by tests; not load-bearing for runtime.
#[derive(Component)]
pub(crate) struct PaneFrame;

/// Bevy Plugin wiring the native Bevy UI rebuild pipeline.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: `.after(bootstrap)` relies on Bevy 0.18's auto-sync-point
        // insertion: bootstrap uses Commands to insert AttachedSession on
        // the primary Window, and setup_root_camera_and_ui_root reads
        // `With<AttachedSession>` — Bevy detects the conflict and inserts
        // an `apply_deferred` between them so the commands are visible.
        // If a future Bevy upgrade weakens that guarantee, move
        // setup_root_camera_and_ui_root to `PostStartup` instead.
        app.init_resource::<ActivityEntityRegistry>()
            .add_systems(
                Startup,
                root::setup_root_camera_and_ui_root.after(crate::bootstrap::bootstrap),
            )
            .add_systems(Update, rebuild_structure_on_change);
    }
}

fn rebuild_structure_on_change(
    mut commands: Commands,
    mux: Res<Multiplexer>,
    attached_q: Query<(Entity, Ref<AttachedSession>), With<Window>>,
    ui_root_q: Query<Entity, With<UiRoot>>,
    structural_q: Query<Entity, With<StructuralNode>>,
    activity_hosts_q: Query<(Entity, &ActivityHostNode)>,
    mut registry: ResMut<ActivityEntityRegistry>,
) {
    let Ok((_window_entity, attached_ref)) = attached_q.single() else {
        return;
    };
    if !mux.is_changed() && !attached_ref.is_changed() {
        return;
    }

    let Ok(ui_root) = ui_root_q.single() else {
        tracing::warn!(
            target: "ozmux_gui::ui",
            "rebuild_structure_on_change: UiRoot missing",
        );
        return;
    };

    let attached_sid = attached_ref.0.clone();
    let Ok(session) = mux.sessions.get(&attached_sid) else {
        tracing::warn!(
            target: "ozmux_gui::ui",
            "attached session {} missing from multiplexer",
            attached_sid,
        );
        return;
    };
    let Some(active_wid) = session.active_window.as_ref() else {
        return;
    };
    let Some(window) = mux.windows.get(active_wid) else {
        tracing::warn!(
            target: "ozmux_gui::ui",
            "active_window {} missing from multiplexer",
            active_wid,
        );
        return;
    };

    // NOTE: removing `ChildOf` must run BEFORE the structural despawn below.
    // Bevy 0.16+ `Children` cascade-despawns descendants of the despawned
    // parent; without this detach the Activity hosts (children of the
    // structural slot we're about to despawn) would be wiped out, breaking
    // the stable-identity contract.
    for (host, _) in activity_hosts_q.iter() {
        commands.entity(host).remove::<ChildOf>();
    }

    for entity in structural_q.iter() {
        commands.entity(entity).despawn();
    }

    let content = commands
        .spawn((
            Node {
                flex_grow: 1.0,
                width: bevy::ui::Val::Percent(100.0),
                padding: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            StructuralNode,
            ChildOf(ui_root),
        ))
        .id();

    let mut live_activity_ids: HashSet<ActivityId> = HashSet::new();
    match window.cells.cell(&window.root_cell) {
        Ok(Cell::Root(root)) => {
            layout::build_cell_recursive(
                &mut commands,
                content,
                window,
                &root.child,
                &mut registry,
                &mut live_activity_ids,
            );
        }
        Ok(_) => {
            tracing::warn!(
                target: "ozmux_gui::ui",
                "window.root_cell {} is not Cell::Root",
                window.root_cell,
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "ozmux_gui::ui",
                "window.root_cell {} missing ({:?})",
                window.root_cell,
                err,
            );
        }
    }

    status_bar::build_status_bar(&mut commands, ui_root, session, active_wid, &mux.windows);

    registry.prune(&mut commands, &live_activity_ids);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::OzmuxBootstrapPlugin;
    use crate::configs::OzmuxConfigsPlugin;
    use crate::multiplexer::OzmuxMultiplexerPlugin;
    use bevy::window::{PrimaryWindow, WindowResolution};

    fn make_test_app() -> (App, std::sync::MutexGuard<'static, ()>) {
        let guard = crate::configs::env_guard();
        // SAFETY: env mutations are serialized by env_guard() for this crate's tests.
        unsafe {
            std::env::remove_var("OZMUX_CONFIG");
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
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
        // where `rebuild_structure_on_change` fires because `AttachedSession` was
        // just inserted.
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
            let world = app.world_mut();
            let mut q = world.query::<(Entity, &ActivityHostNode)>();
            let hosts: Vec<(Entity, ActivityId)> =
                q.iter(world).map(|(e, h)| (e, h.0.clone())).collect();
            let registry = app.world().resource::<ActivityEntityRegistry>();
            hosts
                .into_iter()
                .filter(|(_, id)| registry.get(id).is_some())
                .map(|(e, id)| (id, e))
                .collect()
        };
        assert!(
            !first_snapshot.is_empty(),
            "expected at least one Activity host after bootstrap"
        );

        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let wid = {
                let (_sid, session) = mux.sessions.iter().next().expect("session");
                session.active_window.clone().expect("active window")
            };
            mux.rename_window(&wid, "renamed".into()).expect("rename");
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
            let (wid, active_pane) = {
                let (_sid, session) = mux.sessions.iter().next().expect("session");
                let wid = session.active_window.clone().expect("active window");
                let window = mux.windows.get(&wid).expect("window");
                (wid, window.active_pane.clone())
            };
            let new_pane_id = PaneId::new();
            let new_activity = Activity::terminal(ActivityId::new());
            mux.with_window(&wid, |w| {
                w.split_pane(
                    &active_pane,
                    new_pane_id,
                    new_activity,
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .expect("with_window returned Some")
            .expect("split_pane Ok");
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
            let (wid, active_pane) = {
                let (_sid, session) = mux.sessions.iter().next().expect("session");
                let wid = session.active_window.clone().expect("active window");
                let window = mux.windows.get(&wid).expect("window");
                (wid, window.active_pane.clone())
            };
            mux.with_window(&wid, |w| {
                w.split_pane(
                    &active_pane,
                    new_pane_id.clone(),
                    Activity::terminal(new_activity_id.clone()),
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .expect("with_window")
            .expect("split_pane");
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
            let wid = mux
                .sessions
                .iter()
                .next()
                .expect("session")
                .1
                .active_window
                .clone()
                .expect("active window");
            mux.with_window(&wid, |w| w.close_pane(&new_pane_id))
                .expect("with_window")
                .expect("close_pane");
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
            let mut q = world.query::<(Entity, &ActivityHostNode)>();
            q.iter(world)
                .next()
                .map(|(e, _)| e)
                .expect("at least one host")
        };

        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let wid = {
                let (_sid, session) = mux.sessions.iter().next().expect("session");
                session.active_window.clone().expect("active window")
            };
            mux.rename_window(&wid, "second-rename".into())
                .expect("rename");
        }
        app.update();

        assert!(
            app.world().get_entity(entity_before).is_ok(),
            "host entity must still exist after rebuild — load-bearing for stable handles"
        );
    }
}
