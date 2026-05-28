//! System that reparents the attached Session's UI subtree between
//! `SessionUiRoot` (active) and its owning Session entity (parked). The
//! Session entity is non-`Node`, so a parked subtree is skipped by Bevy's
//! UI walker — no layout, no `ComputedNode` updates, no resize-pass work.

use crate::font::TerminalUiFont;
use crate::system_set::OzmuxSystems;
use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::{ActivityHostNode, SessionUiRoot, StructuralNode};
use bevy::prelude::*;
use bevy::ui::UiSystems;
use ozmux_multiplexer::{
    ActiveActivity, ActivityKind, ActivityMarker, AttachedSession, Cell, LayoutCells, PaneMarker,
    SessionMarker, SessionUiSubtree,
};

pub struct OzmuxSessionUiPlugin;

impl Plugin for OzmuxSessionUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            rebuild_session_ui.in_set(OzmuxSystems::SessionUi),
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

/// Rebuilds the UI subtree of every Session whose `LayoutCells` changed
/// since the last run. Native Bevy `Changed<LayoutCells>` replaces the
/// old epoch-comparison gate. The rebuild walks `layout.cells` and
/// replaces every `StructuralNode` descendant of the session's
/// `SessionUiSubtree` root — Activity hosts are preserved via
/// `ActivityEntityRegistry` and re-parented. Pruning of stale registry
/// entries is handled by `prune_registry_on_activity_removal` driven by
/// `RemovedComponents<ActivityMarker>`.
fn rebuild_session_ui(
    mut commands: Commands,
    mut registry: ResMut<ActivityEntityRegistry>,
    sessions: Query<
        (Entity, &LayoutCells, &SessionUiSubtree, Has<AttachedSession>),
        (With<SessionMarker>, Changed<LayoutCells>),
    >,
    structurals: Query<(Entity, Option<&ChildOf>), With<StructuralNode>>,
    activity_hosts: Query<(Entity, &ActivityHostNode)>,
    children: Query<&Children>,
    activity_q: Query<(&ActivityKind, &Name), With<ActivityMarker>>,
    active_activity_q: Query<&ActiveActivity, With<PaneMarker>>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();

    for (session_entity, layout, subtree, _is_attached) in sessions.iter() {
        descend_and_detach_hosts(&mut commands, subtree.0, &children, &activity_hosts);
        descend_and_despawn_structural(&mut commands, subtree.0, &children, &structurals);

        let root_cell_id = layout.root;
        match layout.cells.cell(&root_cell_id) {
            Ok(Cell::Root(root)) => {
                crate::ui::layout::build_cell_recursive(
                    &mut commands,
                    subtree.0,
                    &layout.cells,
                    &root.child,
                    &mut registry,
                    session_entity,
                    &ui_font_handle,
                    &children,
                    &activity_q,
                    &active_activity_q,
                );
            }
            Ok(_) => tracing::warn!(target: "ozmux_gui::ui", "root_cell is not Cell::Root"),
            Err(err) => tracing::warn!(target: "ozmux_gui::ui", ?err, "root_cell missing"),
        }
    }
}

fn descend_and_detach_hosts(
    commands: &mut Commands,
    root: Entity,
    children: &Query<&Children>,
    activity_hosts: &Query<(Entity, &ActivityHostNode)>,
) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if activity_hosts.get(e).is_ok() {
            commands.entity(e).remove::<ChildOf>();
            continue;
        }
        if let Ok(children) = children.get(e) {
            for c in children.iter() {
                stack.push(c);
            }
        }
    }
}

fn descend_and_despawn_structural(
    commands: &mut Commands,
    root: Entity,
    children: &Query<&Children>,
    structurals: &Query<(Entity, Option<&ChildOf>), With<StructuralNode>>,
) {
    let mut to_despawn = vec![];
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if let Ok(children) = children.get(e) {
            for c in children.iter() {
                stack.push(c);
            }
        }
        if structurals.get(e).is_ok() && e != root {
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
    use crate::ui::OzmuxUiPlugin;
    use crate::ui::SessionUiRoot;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::render::storage::ShaderStorageBuffer;
    use bevy::window::{PrimaryWindow, WindowResolution};
    use bevy_terminal_renderer::material::TerminalUiMaterial;
    use bevy_terminal_renderer::{CellMetrics, TerminalCellMetricsResource};
    use ozmux_multiplexer::{MultiplexerPlugin, SessionMarker};

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
                SessionMarker,
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
                SessionMarker,
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
            .spawn((SessionMarker, SessionUiSubtree(subtree_b)))
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
    fn inactive_activity_within_active_session_parks_under_session_entity() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{ActivityKind, AttachedSession, MultiplexerCommands};

        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let (session, pane, first_activity) = app
            .world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next()?;
                    let pane = mux.sessions_active_pane(session)?;
                    let activity = mux.panes_active_activity(pane)?;
                    Some((session, pane, activity))
                },
            )
            .unwrap()
            .expect("bootstrap session + pane + first_activity");

        let first_host = app
            .world()
            .resource::<crate::ui::registry::ActivityEntityRegistry>()
            .get(first_activity)
            .expect("first activity must have a host after initial rebuild");

        let second_activity = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_activity(pane, ActivityKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_activity(pane, second_activity).unwrap();
            })
            .unwrap();

        app.world_mut()
            .entity_mut(session)
            .get_mut::<LayoutCells>()
            .expect("LayoutCells")
            .set_changed();
        app.update();

        let first_host_parent = app
            .world()
            .get::<ChildOf>(first_host)
            .map(|c| c.parent());
        assert_eq!(
            first_host_parent,
            Some(session),
            "inactive activity host must be parked under the Session entity (non-Node, walker-skipped)"
        );
    }

    #[test]
    fn parked_subtree_has_no_computed_node_updates() {
        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        // Create a second session entity with a SessionUiSubtree but no AttachedSession.
        let inactive_session = {
            let world = app.world_mut();
            let subtree = world.spawn(Node::default()).id();
            let session_entity = world
                .spawn((
                    SessionMarker,
                    SessionUiSubtree(subtree),
                    Name::new("inactive"),
                ))
                .id();
            world.entity_mut(subtree).insert(ChildOf(session_entity));
            subtree
        };
        app.update();
        app.update();

        for _ in 0..5 {
            app.update();
        }
        let computed = app.world().get::<bevy::ui::ComputedNode>(inactive_session);
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

        // Spawn a second session (session B) with a subtree, not attached.
        // Session B has no LayoutCells, so Changed<LayoutCells> never fires for it.
        let (_session_b, subtree_b) = {
            let world = app.world_mut();
            let subtree = world.spawn(Node::default()).id();
            let entity = world
                .spawn((
                    SessionMarker,
                    SessionUiSubtree(subtree),
                    Name::new("b"),
                ))
                .id();
            world.entity_mut(subtree).insert(ChildOf(entity));
            (entity, subtree)
        };
        app.update();
        app.update();

        let children_before: Vec<Entity> = app
            .world()
            .get::<Children>(subtree_b)
            .map(|c| c.iter().collect())
            .unwrap_or_default();

        // Mark session A's LayoutCells as changed to trigger a rebuild on A only.
        // Session B has no LayoutCells, so the Changed<LayoutCells> filter
        // will not include it.
        {
            let world = app.world_mut();
            let session_a = world
                .query_filtered::<Entity, (With<SessionMarker>, With<AttachedSession>)>()
                .single(world)
                .expect("attached session A");
            world
                .entity_mut(session_a)
                .get_mut::<LayoutCells>()
                .expect("LayoutCells on session A")
                .set_changed();
        }
        app.update();

        let children_after: Vec<Entity> = app
            .world()
            .get::<Children>(subtree_b)
            .map(|c| c.iter().collect())
            .unwrap_or_default();
        assert_eq!(
            children_before, children_after,
            "Session B's subtree children must not change when only Session A's LayoutCells changed",
        );
    }

    #[test]
    fn session_subtree_root_has_explicit_sizing() {
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
