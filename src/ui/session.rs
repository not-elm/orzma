//! System that reparents the attached Session's UI subtree between
//! `SessionUiRoot` (active) and its owning Session entity (parked). The
//! Session entity is non-`Node`, so a parked subtree is skipped by Bevy's
//! UI walker — no layout, no `ComputedNode` updates, no resize-pass work.

use std::collections::HashSet;

use crate::font::TerminalUiFont;
use crate::multiplexer::{AttachedSession, Multiplexer, SessionEntityId, SessionUiSubtree};
use crate::system_set::OzmuxSystems;
use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::{ActivityHostNode, SessionUiRoot, StructuralNode};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::ui::UiSystems;
use ozmux_multiplexer::{ActivityId, Cell, SessionId};

pub struct OzmuxSessionUiPlugin;

impl Plugin for OzmuxSessionUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            rebuild_session_ui
                .run_if(resource_exists_and_changed::<Multiplexer>)
                .in_set(OzmuxSystems::SessionUi),
        )
        .add_systems(PostUpdate, sync_active_session.before(UiSystems::Prepare));
    }
}

/// Runs every Update; only does work when the set of `AttachedSession`
/// markers changes. Tracks the previously-attached session's Entity in a
/// `Local<Option<Entity>>` so we can look up its `SessionUiSubtree` and
/// park it back under the Session entity.
fn sync_active_session(
    mut commands: Commands,
    attached_session: Query<&SessionUiSubtree, Added<AttachedSession>>,
    sessions: Query<(Entity, &SessionUiSubtree), Without<AttachedSession>>,
    session_ui_root: Query<Entity, With<SessionUiRoot>>,
) {
    let Ok(newly_attached_subtree) = attached_session.single() else {
        return;
    };
    let Ok(session_ui_root) = session_ui_root.single() else {
        return;
    };

    commands
        .entity(newly_attached_subtree.0)
        .insert(ChildOf(session_ui_root));

    for (session_entity, tree) in sessions.iter() {
        commands.entity(tree.0).insert(ChildOf(session_entity));
    }
}

/// Rebuilds the UI subtree of every Session whose epoch advanced since the
/// last run. Skips sessions whose epoch is unchanged. The rebuild walks
/// `session.cells` and replaces every `StructuralNode` descendant of the
/// session's `SessionUiSubtree` root — Activity hosts are preserved via
/// `ActivityEntityRegistry` and re-parented.
fn rebuild_session_ui(
    mut commands: Commands,
    mut last_epochs: Local<HashMap<SessionId, u64>>,
    mut registry: ResMut<ActivityEntityRegistry>,
    mux: Res<Multiplexer>,
    sessions: Query<(
        Entity,
        &SessionEntityId,
        &SessionUiSubtree,
        Has<AttachedSession>,
    )>,
    structurals: Query<(Entity, Option<&ChildOf>), With<StructuralNode>>,
    activity_hosts: Query<(Entity, &ActivityHostNode)>,
    children: Query<&Children>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();

    let live_activity_ids: HashSet<ActivityId> = mux
        .sessions
        .values()
        .flat_map(|s| s.pane_ids().filter_map(|pid| s.pane(pid).ok()))
        .flat_map(|p| p.activity_ids().cloned())
        .collect();

    for (session_entity, session_eid, subtree, _is_attached) in sessions.iter() {
        let sid = session_eid.0;
        let cur_epoch = mux.epoch_of(&sid);
        let prev = last_epochs.get(&sid).copied().unwrap_or(0);
        if cur_epoch <= prev {
            continue;
        }

        let Some(session) = mux.sessions.get(&sid) else {
            continue;
        };

        descend_and_detach_hosts(&mut commands, subtree.0, &children, &activity_hosts);

        descend_and_despawn_structural(&mut commands, subtree.0, &children, &structurals);

        match session.cells.cell(&session.root_cell) {
            Ok(Cell::Root(root)) => {
                crate::ui::layout::build_cell_recursive(
                    &mut commands,
                    subtree.0,
                    session,
                    &root.child,
                    &mut registry,
                    session_entity,
                    &ui_font_handle,
                );
            }
            Ok(_) => {
                tracing::warn!(
                    target: "ozmux_gui::ui",
                    session = ?sid,
                    "session.root_cell is not Cell::Root",
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: "ozmux_gui::ui",
                    session = ?sid,
                    ?err,
                    "session.root_cell missing",
                );
            }
        }

        last_epochs.insert(sid, cur_epoch);
    }

    registry.prune(&mut commands, &live_activity_ids);
}

fn descend_and_detach_hosts(
    commands: &mut Commands,
    root: Entity,
    children_q: &Query<&Children>,
    activity_hosts_q: &Query<(Entity, &ActivityHostNode)>,
) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if activity_hosts_q.get(e).is_ok() {
            commands.entity(e).remove::<ChildOf>();
            continue;
        }
        if let Ok(children) = children_q.get(e) {
            for c in children.iter() {
                stack.push(c);
            }
        }
    }
}

fn descend_and_despawn_structural(
    commands: &mut Commands,
    root: Entity,
    children_q: &Query<&Children>,
    structural_q: &Query<(Entity, Option<&ChildOf>), With<StructuralNode>>,
) {
    let mut to_despawn = vec![];
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if let Ok(children) = children_q.get(e) {
            for c in children.iter() {
                stack.push(c);
            }
        }
        if structural_q.get(e).is_ok() && e != root {
            to_despawn.push(e);
        }
    }
    for e in to_despawn {
        commands.entity(e).try_despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::OzmuxBootstrapPlugin;
    use crate::configs::OzmuxConfigsPlugin;
    use crate::multiplexer::OzmuxMultiplexerPlugin;
    use crate::ui::OzmuxUiPlugin;
    use crate::ui::SessionUiRoot;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::render::storage::ShaderStorageBuffer;
    use bevy::window::{PrimaryWindow, WindowResolution};
    use bevy_terminal_renderer::material::TerminalUiMaterial;
    use bevy_terminal_renderer::{CellMetrics, TerminalCellMetricsResource};
    use ozmux_multiplexer::SessionId;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(800, 600),
                ..default()
            },
            PrimaryWindow,
        ));
        let ui_root = app.world_mut().spawn(Node::default()).id();
        app.world_mut()
            .spawn((Node::default(), SessionUiRoot, ChildOf(ui_root)));
        app.add_systems(Update, sync_active_session);
        app
    }

    #[test]
    fn attaches_initial_session_subtree_to_session_ui_root() {
        let mut app = build_app();

        let subtree = app.world_mut().spawn(Node::default()).id();
        let session = app
            .world_mut()
            .spawn((
                SessionEntityId(SessionId(0)),
                AttachedSession,
                SessionUiSubtree(subtree),
            ))
            .id();
        app.world_mut().entity_mut(subtree).insert(ChildOf(session));

        app.update();

        let session_ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<SessionUiRoot>>()
            .single(app.world())
            .expect("SessionUiRoot");
        let parent = app
            .world()
            .get::<ChildOf>(subtree)
            .expect("subtree has parent")
            .parent();
        assert_eq!(
            parent, session_ui_root,
            "active session's subtree must be under SessionUiRoot"
        );
    }

    #[test]
    fn switching_active_session_parks_previous_subtree_under_its_session_entity() {
        let mut app = build_app();

        let subtree_a = app.world_mut().spawn(Node::default()).id();
        let session_a = app
            .world_mut()
            .spawn((
                SessionEntityId(SessionId(0)),
                AttachedSession,
                SessionUiSubtree(subtree_a),
            ))
            .id();
        app.world_mut()
            .entity_mut(subtree_a)
            .insert(ChildOf(session_a));

        let subtree_b = app.world_mut().spawn(Node::default()).id();
        let session_b = app
            .world_mut()
            .spawn((SessionEntityId(SessionId(1)), SessionUiSubtree(subtree_b)))
            .id();
        app.world_mut()
            .entity_mut(subtree_b)
            .insert(ChildOf(session_b));

        app.update();

        app.world_mut()
            .entity_mut(session_a)
            .remove::<AttachedSession>();
        app.world_mut()
            .entity_mut(session_b)
            .insert(AttachedSession);
        app.update();

        let session_ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<SessionUiRoot>>()
            .single(app.world())
            .expect("SessionUiRoot");
        let parent_a = app
            .world()
            .get::<ChildOf>(subtree_a)
            .expect("a subtree has parent")
            .parent();
        let parent_b = app
            .world()
            .get::<ChildOf>(subtree_b)
            .expect("b subtree has parent")
            .parent();
        assert_eq!(
            parent_a, session_a,
            "previous subtree must park under its Session entity"
        );
        assert_eq!(
            parent_b, session_ui_root,
            "new subtree must attach to SessionUiRoot"
        );
    }

    fn make_test_app_v2() -> (App, std::sync::MutexGuard<'static, ()>) {
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
    fn inactive_activity_within_active_session_parks_under_session_entity() {
        use ozmux_multiplexer::Activity;

        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let (bootstrap_aid, second_aid, session_entity, _sid) = {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let sid = *mux.sessions.keys().next().expect("session");
            let (active_pane, bootstrap_aid) = {
                let session = mux.sessions.get(&sid).expect("session");
                let pane = session.pane(&session.active_pane).expect("pane");
                (session.active_pane.clone(), pane.active_activity.clone())
            };
            let second = ActivityId::new();
            mux.with_session(&sid, |s| {
                s.pane_mut(&active_pane)
                    .expect("pane_mut")
                    .add_activity(Activity::terminal(second.clone()))
            })
            .expect("with_session")
            .expect("add_activity");
            mux.bump_epoch(&sid);

            let mut q = world.query::<(Entity, &SessionEntityId)>();
            let (entity, _) = q.iter(world).next().expect("session entity");
            (bootstrap_aid, second, entity, sid)
        };
        app.update();

        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let sid = *mux.sessions.keys().next().expect("session");
            let active_pane = mux.sessions.get(&sid).expect("session").active_pane.clone();
            let _outcome = mux
                .with_session(&sid, |s| {
                    s.pane_mut(&active_pane)
                        .expect("pane_mut")
                        .set_active_activity(&second_aid)
                })
                .expect("with_session")
                .expect("set_active_activity");
            mux.bump_epoch(&sid);
        }
        app.update();

        let registry = app
            .world()
            .resource::<crate::ui::registry::ActivityEntityRegistry>();
        let bootstrap_host = registry
            .get(&bootstrap_aid)
            .expect("bootstrap host in registry");
        let parent = app
            .world()
            .get::<ChildOf>(bootstrap_host)
            .expect("inactive host must have parent");
        assert_eq!(
            parent.parent(),
            session_entity,
            "inactive Activity host within active session must be ChildOf the Session entity (non-Node parent => walker-skipped)",
        );
        assert!(
            app.world().get::<bevy::ui::Node>(session_entity).is_none(),
            "Session entity must not carry Node (load-bearing for walker-skip)",
        );
    }

    #[test]
    fn parked_subtree_has_no_computed_node_updates() {
        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let inactive_sid = {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let (sid, _, _) = mux.create_session(Some("inactive".into()));
            mux.bump_epoch(&sid);
            sid
        };
        {
            let world = app.world_mut();
            let subtree = world.spawn(Node::default()).id();
            let session_entity = world
                .spawn((
                    SessionEntityId(inactive_sid),
                    SessionUiSubtree(subtree),
                    Name::new("inactive"),
                ))
                .id();
            world.entity_mut(subtree).insert(ChildOf(session_entity));
        }
        app.update();
        app.update();

        let inactive_subtree = {
            let world = app.world_mut();
            let mut q = world.query::<(&SessionEntityId, &SessionUiSubtree)>();
            q.iter(world)
                .find_map(|(sid, sub)| (sid.0 == inactive_sid).then_some(sub.0))
        }
        .expect("inactive subtree present");

        for _ in 0..5 {
            app.update();
        }
        let computed = app.world().get::<bevy::ui::ComputedNode>(inactive_subtree);
        match computed {
            None => {
                // Walker skipped — ideal.
            }
            Some(c) => {
                assert_eq!(
                    c.size,
                    bevy::math::Vec2::ZERO,
                    "parked subtree's ComputedNode size must be zero (walker should not lay it out)",
                );
            }
        }
    }

    #[test]
    fn per_session_rebuild_only_touches_changed_session() {
        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let (sid_b, subtree_b) = {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let (sid, _, _) = mux.create_session(Some("b".into()));
            mux.bump_epoch(&sid);
            let subtree = world.spawn(Node::default()).id();
            let entity = world
                .spawn((
                    SessionEntityId(sid),
                    SessionUiSubtree(subtree),
                    Name::new("b"),
                ))
                .id();
            world.entity_mut(subtree).insert(ChildOf(entity));
            (sid, subtree)
        };
        app.update();
        app.update();

        let children_before: Vec<Entity> = app
            .world()
            .get::<Children>(subtree_b)
            .map(|c| c.iter().collect())
            .unwrap_or_default();

        {
            let world = app.world_mut();
            let mut mux = world.resource_mut::<Multiplexer>();
            let sid_a = *mux
                .sessions
                .keys()
                .find(|s| **s != sid_b)
                .expect("session A (distinct from B)");
            mux.rename_session(&sid_a, "renamed".into())
                .expect("rename");
            mux.bump_epoch(&sid_a);
        }
        app.update();

        let children_after: Vec<Entity> = app
            .world()
            .get::<Children>(subtree_b)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(
            children_before, children_after,
            "Session B's subtree children must not change when only Session A's epoch bumped",
        );
    }

    #[test]
    fn session_subtree_root_has_explicit_sizing() {
        // Regression guard: the SessionUiSubtree root must carry explicit
        // `width: Percent(100), height: Percent(100)`. Without this,
        // `Node::default()` (`width: Auto, height: Auto, flex_grow: 0.0`)
        // makes the subtree collapse to zero size when attached to
        // SessionUiRoot, so taffy lays out every descendant (pane frames,
        // activity slots, terminal hosts) as zero-sized and
        // `resize_terminals_to_node` clamps the PTY grid to 1x1.
        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let active_subtree = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<&SessionUiSubtree, With<AttachedSession>>();
            q.single(world).expect("one attached subtree").0
        };

        let node = app
            .world()
            .get::<bevy::ui::Node>(active_subtree)
            .expect("SessionUiSubtree root must have a Node component");
        assert_eq!(
            node.width,
            bevy::ui::Val::Percent(100.0),
            "subtree root must set width: Percent(100) so it fills SessionUiRoot",
        );
        assert_eq!(
            node.height,
            bevy::ui::Val::Percent(100.0),
            "subtree root must set height: Percent(100) so it fills SessionUiRoot",
        );
    }
}
