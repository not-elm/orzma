//! System that reparents the attached Session's UI subtree between
//! `SessionUiRoot` (active) and its owning Session entity (parked). The
//! Session entity is non-`Node`, so a parked subtree is skipped by Bevy's
//! UI walker — no layout, no `ComputedNode` updates, no resize-pass work.

use crate::configs::OzmuxConfigsResource;
use crate::font::TerminalUiFont;
use crate::system_set::OzmuxSystems;
use crate::theme;
use crate::ui::layout::build_cell_recursive;
use crate::ui::registry::SurfaceEntityRegistry;
use crate::ui::tab_label::LabelCtx;
use crate::ui::terminal::resolve_pane_session;
use crate::ui::{
    HomeDir, HostSurfaceEntity, PaneDimOverlay, SessionUiDirty, SessionUiRoot, StructuralNode,
    SurfaceHostNode, TerminalSurfaceMarker,
};
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy_terminal_renderer::material::{PaneDim, TerminalUiMaterial};
use ozmux_extension_host::ExtensionControlSet;
use ozmux_multiplexer::{
    ActivePane, ActiveSurface, AttachedSession, Cell, Cwd, LayoutCells, PaneMarker, SessionMarker,
    SessionUiSubtree, SurfaceKind, SurfaceMarker,
};

pub struct OzmuxSessionUiPlugin;

impl Plugin for OzmuxSessionUiPlugin {
    fn build(&self, app: &mut App) {
        order_surface_pipeline(app);
        app.add_systems(
            Update,
            flag_chrome_dirty_on_surface_change.in_set(OzmuxSystems::ChromeInvalidate),
        )
        .add_systems(Update, rebuild_session_ui.in_set(OzmuxSystems::SessionUi))
        .add_systems(Update, sync_pane_dim.after(OzmuxSystems::Input))
        .add_systems(
            Update,
            sync_terminal_dim_on_focus.after(OzmuxSystems::Input),
        )
        .add_systems(
            Update,
            sync_terminal_dim_on_mount.after(OzmuxSystems::SetupSurface),
        )
        .add_systems(PostUpdate, sync_active_session.before(UiSystems::Prepare));
    }
}

/// Orders the per-frame surface pipeline so each stage sees the previous
/// stage's committed `Commands` — Bevy inserts an `ApplyDeferred` sync point on
/// each ordering edge: control-bridge drain ([`ExtensionControlSet::Drain`]) →
/// session-UI rebuild ([`OzmuxSystems::SessionUi`]) → surface setup
/// ([`OzmuxSystems::SetupSurface`], which attaches terminals/webviews).
///
/// Without this, unordered stages race nondeterministically:
/// - the rebuild can run before the split's deferred pane/`ActiveSurface`/
///   `ChildOf` commands flush → a pane with no surface tab, no host, no webview
///   (sticky: the one-shot `Changed<LayoutCells>` is already consumed);
/// - surface setup can queue a bundle insert onto a host the rebuild/prune is
///   about to despawn → an insert-after-despawn panic.
///
/// `prune_registry_on_surface_removal` is ordered before `SessionUi` separately
/// (in `OzmuxUiPlugin`), so host despawns are committed before both the rebuild
/// and surface setup observe them.
fn order_surface_pipeline(app: &mut App) {
    app.configure_sets(
        Update,
        (
            OzmuxSystems::ChromeInvalidate
                .after(ExtensionControlSet::Drain)
                .after(OzmuxSystems::Input),
            OzmuxSystems::SessionUi.after(OzmuxSystems::ChromeInvalidate),
            OzmuxSystems::SetupSurface.after(OzmuxSystems::SessionUi),
        ),
    );
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
/// `SessionUiSubtree` root — Surface hosts are preserved via
/// `SurfaceEntityRegistry` and re-parented. Pruning of stale registry
/// entries is handled by `prune_registry_on_surface_removal` driven by
/// `RemovedComponents<SurfaceMarker>`.
fn rebuild_session_ui(
    mut commands: Commands,
    mut registry: ResMut<SurfaceEntityRegistry>,
    sessions: Query<
        (
            Entity,
            &LayoutCells,
            &SessionUiSubtree,
            Option<&ActivePane>,
            Has<AttachedSession>,
        ),
        (
            With<SessionMarker>,
            Or<(Changed<LayoutCells>, With<SessionUiDirty>)>,
        ),
    >,
    structurals: Query<(Entity, Option<&ChildOf>), With<StructuralNode>>,
    surface_hosts: Query<(Entity, &SurfaceHostNode)>,
    children: Query<&Children>,
    surfaces: Query<(&SurfaceKind, &Name, Option<&Cwd>), With<SurfaceMarker>>,
    active_surfaces: Query<&ActiveSurface, With<PaneMarker>>,
    ui_font: Option<Res<TerminalUiFont>>,
    configs: Option<Res<OzmuxConfigsResource>>,
    home_dir: Option<Res<HomeDir>>,
) {
    let ui_font_handle = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();

    let veil: Option<Color> = match configs.as_deref() {
        Some(cfg) if cfg.inactive_pane.enabled => {
            let (r, g, b) = cfg.inactive_pane.rgb();
            Some(Color::srgb_u8(r, g, b).with_alpha(cfg.inactive_pane.opacity))
        }
        _ => None,
    };
    let label_ctx = LabelCtx {
        home: home_dir.and_then(|h| h.0.clone()),
        max_chars: theme::TAB_LABEL_MAX_CHARS,
    };

    for (session_entity, layout, subtree, active_pane, _is_attached) in sessions.iter() {
        let active_pane = active_pane.map(|a| a.0);
        let session_veil = if active_pane.is_some() { veil } else { None };
        let active_pane = active_pane.unwrap_or(Entity::PLACEHOLDER);
        descend_and_detach_hosts(&mut commands, subtree.0, &children, &surface_hosts);
        descend_and_despawn_structural(&mut commands, subtree.0, &children, &structurals);

        let root_cell_id = layout.root;
        match layout.cells.cell(&root_cell_id) {
            Ok(Cell::Root(root)) => {
                build_cell_recursive(
                    &mut commands,
                    subtree.0,
                    &layout.cells,
                    &root.child,
                    &mut registry,
                    session_entity,
                    &ui_font_handle,
                    &children,
                    &surfaces,
                    &active_surfaces,
                    active_pane,
                    session_veil,
                    &label_ctx,
                );
            }
            Ok(_) => tracing::warn!(target: "ozmux_gui::ui", "root_cell is not Cell::Root"),
            Err(err) => tracing::warn!(target: "ozmux_gui::ui", ?err, "root_cell missing"),
        }
        // NOTE: must remove the marker after rebuilding, or `With<SessionUiDirty>`
        // keeps matching and the session rebuilds every frame. No-op when the
        // rebuild was triggered by `Changed<LayoutCells>` (marker absent).
        commands.entity(session_entity).remove::<SessionUiDirty>();
    }
}

/// Flags a session `SessionUiDirty` when one of its panes gains a surface
/// (`Added<SurfaceMarker>`), switches its active surface
/// (`Changed<ActiveSurface>`), or a surface reports a new working directory
/// (`Changed<Cwd>`, via OSC 7) — in-pane changes that do not mutate
/// `LayoutCells` and so would otherwise not trigger `rebuild_session_ui`.
/// Covers both the `@md` control-bridge path and the in-app shortcuts via a
/// single UI-layer hook, keeping the multiplexer crate free of UI concerns.
/// Ordered after the drain / input and before `SessionUi` so the marker is
/// visible to the rebuild the same frame. The split case already changes
/// `LayoutCells`; a redundant flag there is harmless.
fn flag_chrome_dirty_on_surface_change(
    mut commands: Commands,
    added_surfaces: Query<Entity, Added<SurfaceMarker>>,
    switched_panes: Query<Entity, (With<PaneMarker>, Changed<ActiveSurface>)>,
    changed_cwd: Query<Entity, (With<SurfaceMarker>, Changed<Cwd>)>,
    child_of: Query<&ChildOf>,
) {
    for surface in added_surfaces.iter() {
        if let Some((_pane, session)) = resolve_pane_session(surface, &child_of) {
            commands.entity(session).insert(SessionUiDirty);
        }
    }
    for pane in switched_panes.iter() {
        if let Ok(pane_parent) = child_of.get(pane) {
            commands.entity(pane_parent.parent()).insert(SessionUiDirty);
        }
    }
    for surface in changed_cwd.iter() {
        if let Some((_pane, session)) = resolve_pane_session(surface, &child_of) {
            commands.entity(session).insert(SessionUiDirty);
        }
    }
}

fn descend_and_detach_hosts(
    commands: &mut Commands,
    root: Entity,
    children: &Query<&Children>,
    surface_hosts: &Query<(Entity, &SurfaceHostNode)>,
) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if surface_hosts.get(e).is_ok() {
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

/// Flips each pane's dim veil when its session's `ActivePane` changes
/// (focus moves between panes without a layout rebuild). For every session
/// whose `ActivePane` changed, sets each `PaneDimOverlay` belonging to that
/// session to `Hidden` iff its pane is the new active pane, else `Visible`.
/// Pane→session is resolved via `ChildOf`; using `MultiplexerCommands` here
/// would conflict on its `&mut ActivePane`.
fn sync_pane_dim(
    mut overlays: Query<(&PaneDimOverlay, &mut Visibility)>,
    changed_sessions: Query<(Entity, &ActivePane), Changed<ActivePane>>,
    panes: Query<&ChildOf, With<PaneMarker>>,
) {
    for (session, active) in changed_sessions.iter() {
        for (overlay, mut visibility) in overlays.iter_mut() {
            let Ok(child_of) = panes.get(overlay.pane) else {
                continue;
            };
            if child_of.parent() != session {
                continue;
            }
            let want = if overlay.pane == active.0 {
                Visibility::Hidden
            } else {
                Visibility::Visible
            };
            visibility.set_if_neq(want);
        }
    }
}

/// Inactive-terminal brightness multiplier from config: `inactive_pane.dim`
/// when dimming is enabled, else `1.0` (disabled or absent config = no dim).
fn inactive_dim_factor(configs: Option<&OzmuxConfigsResource>) -> f32 {
    match configs {
        Some(cfg) if cfg.inactive_pane.enabled => cfg.inactive_pane.dim,
        _ => 1.0,
    }
}

/// On focus change, sets each terminal host's [`PaneDim`] in the changed
/// session: `1.0` for the active pane's terminal, the configured dim factor
/// otherwise. A darkening veil is invisible on a black terminal, so terminals
/// are dimmed at the renderer instead. Pane→session is resolved via `ChildOf`;
/// `MultiplexerCommands` can't be used here (it holds `&mut ActivePane`).
fn sync_terminal_dim_on_focus(
    mut commands: Commands,
    changed_sessions: Query<(Entity, &ActivePane), Changed<ActivePane>>,
    hosts: Query<
        (Entity, &HostSurfaceEntity),
        (
            With<TerminalSurfaceMarker>,
            With<MaterialNode<TerminalUiMaterial>>,
        ),
    >,
    child_of: Query<&ChildOf>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let dim_factor = inactive_dim_factor(configs.as_deref());
    for (session, active) in changed_sessions.iter() {
        for (host, host_surface) in hosts.iter() {
            let Some((pane, host_session)) = resolve_pane_session(host_surface.0, &child_of) else {
                continue;
            };
            if host_session != session {
                continue;
            }
            let want = if pane == active.0 { 1.0 } else { dim_factor };
            commands.entity(host).insert(PaneDim(want));
        }
    }
}

/// Sets the initial [`PaneDim`] on a terminal host the frame its material is
/// mounted. Hosts attach lazily after a rebuild — possibly a frame after the
/// focus change `sync_terminal_dim_on_focus` reacts to — so this reads the
/// host's session `ActivePane` directly, dimming a freshly-split inactive
/// terminal without waiting for the next focus change.
fn sync_terminal_dim_on_mount(
    mut commands: Commands,
    newly_mounted: Query<
        (Entity, &HostSurfaceEntity),
        (
            With<TerminalSurfaceMarker>,
            Added<MaterialNode<TerminalUiMaterial>>,
        ),
    >,
    active_panes: Query<&ActivePane>,
    child_of: Query<&ChildOf>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let dim_factor = inactive_dim_factor(configs.as_deref());
    for (host, host_surface) in newly_mounted.iter() {
        let Some((pane, session)) = resolve_pane_session(host_surface.0, &child_of) else {
            continue;
        };
        let is_active = active_panes
            .get(session)
            .map(|a| a.0 == pane)
            .unwrap_or(true);
        let want = if is_active { 1.0 } else { dim_factor };
        commands.entity(host).insert(PaneDim(want));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::OzmuxActionPlugin;
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
    fn session_ui_runs_after_control_drain_so_deferred_commands_are_visible() {
        // Regression for the intermittent dark/empty extension pane: the `@memo`
        // split mutates `LayoutCells` immediately but wires the new pane's
        // `ActiveSurface` / `ChildOf` through deferred `Commands`.
        // `rebuild_session_ui` (in `OzmuxSystems::SessionUi`) must run after the
        // control-bridge drain (`ExtensionControlSet::Drain`) so the inserted
        // `ApplyDeferred` flushes those commands before the rebuild reads the
        // layout. This adds the real `OzmuxSessionUiPlugin` (which wires the
        // ordering) and proves a SessionUi-set system observes a Drain-set
        // system's deferred spawn within the same frame.
        #[derive(Resource, Default)]
        struct RebuildSaw(Option<bool>);
        #[derive(Component)]
        struct DrainSpawned;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SurfaceEntityRegistry>()
            .init_resource::<RebuildSaw>()
            .add_plugins(OzmuxSessionUiPlugin);
        app.add_systems(
            Update,
            (
                (|mut commands: Commands| {
                    commands.spawn(DrainSpawned);
                })
                .in_set(ExtensionControlSet::Drain),
                (|q: Query<(), With<DrainSpawned>>, mut saw: ResMut<RebuildSaw>| {
                    saw.0 = Some(!q.is_empty());
                })
                .in_set(OzmuxSystems::SessionUi),
            ),
        );

        app.update();

        assert_eq!(
            app.world().resource::<RebuildSaw>().0,
            Some(true),
            "a SessionUi-set system must observe the control drain's deferred \
             spawn within the same frame; SessionUi must be ordered after \
             ExtensionControlSet::Drain (inserting an ApplyDeferred sync point)"
        );
    }

    #[test]
    fn attaches_initial_session_subtree_to_session_ui_root() {
        let mut app = build_app();

        let subtree = app.world_mut().spawn(Node::default()).id();
        let session = app
            .world_mut()
            .spawn((SessionMarker, AttachedSession, SessionUiSubtree(subtree)))
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
            .spawn((SessionMarker, AttachedSession, SessionUiSubtree(subtree_a)))
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
            .add_plugins(OzmuxActionPlugin)
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
    fn in_pane_surface_add_triggers_rebuild_via_session_ui_dirty() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{AttachedSession, MultiplexerCommands, SurfaceKind};

        let (mut app, _guard) = make_test_app_v2();
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
            .expect("bootstrap session + active pane");

        // Add an in-pane surface WITHOUT touching LayoutCells. The only path
        // that can drive a rebuild here is flag_chrome_dirty_on_surface_change
        // setting SessionUiDirty from Added<SurfaceMarker>.
        let added = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(pane, SurfaceKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();
        app.update();
        app.update();

        assert!(
            app.world()
                .resource::<SurfaceEntityRegistry>()
                .get(added)
                .is_some(),
            "adding an in-pane surface (no LayoutCells change) must trigger a \
             SessionUiDirty rebuild; build_pane proves it ran by spawning a host \
             for the new surface"
        );
    }

    #[test]
    fn inactive_surface_within_active_session_parks_under_session_entity() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{AttachedSession, MultiplexerCommands, SurfaceKind};

        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let (session, pane, first_surface) = app
            .world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next()?;
                    let pane = mux.sessions_active_pane(session)?;
                    let surface = mux.panes_active_surface(pane)?;
                    Some((session, pane, surface))
                },
            )
            .unwrap()
            .expect("bootstrap session + pane + first_surface");

        let first_host = app
            .world()
            .resource::<crate::ui::registry::SurfaceEntityRegistry>()
            .get(first_surface)
            .expect("first surface must have a host after initial rebuild");

        let second_surface = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.add_surface(pane, SurfaceKind::Terminal)
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_surface(pane, second_surface).unwrap();
            })
            .unwrap();

        app.world_mut()
            .entity_mut(session)
            .get_mut::<LayoutCells>()
            .expect("LayoutCells")
            .set_changed();
        app.update();

        let first_host_parent = app.world().get::<ChildOf>(first_host).map(|c| c.parent());
        assert_eq!(
            first_host_parent,
            Some(session),
            "inactive surface host must be parked under the Session entity (non-Node, walker-skipped)"
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
                .spawn((SessionMarker, SessionUiSubtree(subtree), Name::new("b")))
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

    #[test]
    fn new_session_action_reparents_new_subtree_to_session_ui_root() {
        let (mut app, _guard) = make_test_app_v2();
        // Two ticks for Startup + first Update so bootstrap settles and
        // sync_active_session runs at least once in PostUpdate.
        app.update();
        app.update();

        let session_ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<SessionUiRoot>>()
            .single(app.world())
            .expect("SessionUiRoot");
        let bootstrap_session = app
            .world_mut()
            .query_filtered::<Entity, (With<SessionMarker>, With<AttachedSession>)>()
            .single(app.world())
            .expect("exactly one bootstrap session");
        let bootstrap_subtree = app
            .world()
            .get::<SessionUiSubtree>(bootstrap_session)
            .expect("bootstrap session has SessionUiSubtree pointer")
            .0;
        assert_eq!(
            app.world()
                .get::<ChildOf>(bootstrap_subtree)
                .expect("bootstrap subtree has ChildOf")
                .parent(),
            session_ui_root,
            "bootstrap subtree must start under SessionUiRoot",
        );

        app.world_mut()
            .trigger(crate::action::session::NewSessionActionEvent {
                session: bootstrap_session,
            });
        // One tick for commands to flush + rebuild_session_ui to run, one for
        // PostUpdate sync_active_session to react.
        app.update();
        app.update();

        let new_session = app
            .world_mut()
            .query_filtered::<Entity, (With<SessionMarker>, With<AttachedSession>)>()
            .single(app.world())
            .expect("exactly one attached session after CMD+R");
        assert_ne!(
            new_session, bootstrap_session,
            "new session entity must differ from bootstrap",
        );

        let new_subtree = app
            .world()
            .get::<SessionUiSubtree>(new_session)
            .expect("new session has SessionUiSubtree pointer")
            .0;
        assert_eq!(
            app.world()
                .get::<ChildOf>(new_subtree)
                .expect("new subtree has ChildOf")
                .parent(),
            session_ui_root,
            "new session's subtree must be reparented to SessionUiRoot",
        );

        let old_subtree = app
            .world()
            .get::<SessionUiSubtree>(bootstrap_session)
            .expect("old session retains SessionUiSubtree pointer")
            .0;
        assert_eq!(
            app.world()
                .get::<ChildOf>(old_subtree)
                .expect("old subtree has ChildOf")
                .parent(),
            bootstrap_session,
            "old session's subtree must be parked under its session entity",
        );
    }
}
