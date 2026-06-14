//! System that reparents the attached Workspace's UI subtree between
//! `WorkspaceUiRoot` (active) and its owning Workspace entity (parked). The
//! Workspace entity is non-`Node`, so a parked subtree is skipped by Bevy's
//! UI walker — no layout, no `ComputedNode` updates, no resize-pass work.

use crate::configs::OzmuxConfigsResource;
use crate::system_set::OzmuxSystems;
use crate::ui::terminal::resolve_pane_workspace;
use crate::ui::{TerminalSurfaceMarker, WorkspaceUiRoot};
use bevy::prelude::*;
use bevy::ui::UiSystems;
use ozma_tty_renderer::material::{PaneDim, TerminalUiMaterial};
use ozmux_multiplexer::{
    ActivePane, AttachedWorkspace, OwningWorkspace, PaneMarker, SurfaceOf, WorkspaceUiSubtree,
};

pub struct OzmuxWorkspaceUiPlugin;

impl Plugin for OzmuxWorkspaceUiPlugin {
    fn build(&self, app: &mut App) {
        order_surface_pipeline(app);
        app.add_systems(
            Update,
            sync_terminal_dim_on_focus.after(OzmuxSystems::Input),
        )
        .add_systems(
            Update,
            sync_terminal_dim_on_mount.after(OzmuxSystems::SetupSurface),
        )
        .add_systems(PostUpdate, sync_active_workspace.before(UiSystems::Prepare));
    }
}

/// Orders the per-frame surface pipeline so each stage sees the previous
/// stage's committed `Commands` — Bevy inserts an `ApplyDeferred` sync point on
/// each ordering edge: input ([`OzmuxSystems::Input`]) → chrome build
/// ([`OzmuxSystems::BuildChrome`]) → surface setup
/// ([`OzmuxSystems::SetupSurface`], which attaches terminals/webviews).
///
/// Without this, unordered stages race nondeterministically: chrome can run
/// before the split's deferred pane / `ActiveSurface` / `ChildOf` commands
/// flush, and surface setup can queue a bundle insert onto a surface before its
/// pane's slot exists.
fn order_surface_pipeline(app: &mut App) {
    app.configure_sets(
        Update,
        (
            OzmuxSystems::BuildChrome.after(OzmuxSystems::Input),
            OzmuxSystems::SetupSurface.after(OzmuxSystems::BuildChrome),
        ),
    );
}

/// Runs every Update; only does work when the set of `AttachedWorkspace`
/// markers changes. Tracks the previously-attached workspace's Entity in a
/// `Local<Option<Entity>>` so we can look up its `WorkspaceUiSubtree` and
/// park it back under the Workspace entity.
fn sync_active_workspace(
    mut commands: Commands,
    attached_workspace: Query<&WorkspaceUiSubtree, Added<AttachedWorkspace>>,
    workspaces: Query<(Entity, &WorkspaceUiSubtree), Without<AttachedWorkspace>>,
    workspace_ui_root: Query<Entity, With<WorkspaceUiRoot>>,
) {
    let Ok(newly_attached_subtree) = attached_workspace.single() else {
        return;
    };
    let Ok(workspace_ui_root) = workspace_ui_root.single() else {
        return;
    };

    commands
        .entity(newly_attached_subtree.0)
        .insert(ChildOf(workspace_ui_root));

    for (workspace_entity, tree) in workspaces.iter() {
        commands.entity(tree.0).insert(ChildOf(workspace_entity));
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
/// workspace: `1.0` for the active pane's terminal, the configured dim factor
/// otherwise. A darkening veil is invisible on a black terminal, so terminals
/// are dimmed at the renderer instead. Pane→workspace is resolved via `ChildOf`;
/// `MultiplexerCommands` can't be used here (it holds `&mut ActivePane`).
fn sync_terminal_dim_on_focus(
    mut commands: Commands,
    changed_workspaces: Query<(Entity, &ActivePane), Changed<ActivePane>>,
    surfaces: Query<
        Entity,
        (
            With<TerminalSurfaceMarker>,
            With<MaterialNode<TerminalUiMaterial>>,
        ),
    >,
    owners: Query<&SurfaceOf>,
    pane_workspaces: Query<&OwningWorkspace, With<PaneMarker>>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let dim_factor = inactive_dim_factor(configs.as_deref());
    for (workspace, active) in changed_workspaces.iter() {
        for surface in surfaces.iter() {
            let Some((pane, host_workspace)) =
                resolve_pane_workspace(surface, &owners, &pane_workspaces)
            else {
                continue;
            };
            if host_workspace != workspace {
                continue;
            }
            let want = if pane == active.0 { 1.0 } else { dim_factor };
            commands.entity(surface).insert(PaneDim(want));
        }
    }
}

/// Sets the initial [`PaneDim`] on a terminal host the frame its material is
/// mounted. Hosts attach lazily after a rebuild — possibly a frame after the
/// focus change `sync_terminal_dim_on_focus` reacts to — so this reads the
/// host's workspace `ActivePane` directly, dimming a freshly-split inactive
/// terminal without waiting for the next focus change.
fn sync_terminal_dim_on_mount(
    mut commands: Commands,
    newly_mounted: Query<
        Entity,
        (
            With<TerminalSurfaceMarker>,
            Added<MaterialNode<TerminalUiMaterial>>,
        ),
    >,
    active_panes: Query<&ActivePane>,
    owners: Query<&SurfaceOf>,
    pane_workspaces: Query<&OwningWorkspace, With<PaneMarker>>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let dim_factor = inactive_dim_factor(configs.as_deref());
    for surface in newly_mounted.iter() {
        let Some((pane, workspace)) = resolve_pane_workspace(surface, &owners, &pane_workspaces)
        else {
            continue;
        };
        let is_active = active_panes
            .get(workspace)
            .map(|a| a.0 == pane)
            .unwrap_or(true);
        let want = if is_active { 1.0 } else { dim_factor };
        commands.entity(surface).insert(PaneDim(want));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::OzmuxActionPlugin;
    use crate::bootstrap::OzmuxBootstrapPlugin;
    use crate::configs::OzmuxConfigsPlugin;
    use crate::ui::OzmuxUiPlugin;
    use crate::ui::WorkspaceUiRoot;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::render::storage::ShaderStorageBuffer;
    use bevy::window::{PrimaryWindow, WindowResolution};
    use ozma_tty_renderer::material::TerminalUiMaterial;
    use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};
    use ozmux_multiplexer::{MultiplexerPlugin, WorkspaceMarker};

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
            .spawn((Node::default(), WorkspaceUiRoot, ChildOf(ui_root)));
        app.add_systems(Update, sync_active_workspace);
        app
    }

    #[test]
    fn chrome_runs_after_input_so_deferred_commands_are_visible() {
        // Regression for the intermittent dark/empty pane: an in-app split spawns
        // the new pane via deferred `Commands` in the input phase
        // (`OzmuxSystems::Input`). The chrome-build stage
        // (`OzmuxSystems::BuildChrome`) must run after input so the inserted
        // `ApplyDeferred` flushes those commands before chrome reads the new pane.
        #[derive(Resource, Default)]
        struct ChromeSaw(Option<bool>);
        #[derive(Component)]
        struct InputSpawned;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ChromeSaw>()
            .add_plugins(OzmuxWorkspaceUiPlugin);
        app.add_systems(
            Update,
            (
                (|mut commands: Commands| {
                    commands.spawn(InputSpawned);
                })
                .in_set(OzmuxSystems::Input),
                (|q: Query<(), With<InputSpawned>>, mut saw: ResMut<ChromeSaw>| {
                    saw.0 = Some(!q.is_empty());
                })
                .in_set(OzmuxSystems::BuildChrome),
            ),
        );

        app.update();

        assert_eq!(
            app.world().resource::<ChromeSaw>().0,
            Some(true),
            "a BuildChrome-set system must observe an Input-set system's deferred \
             spawn within the same frame; BuildChrome must be ordered after \
             OzmuxSystems::Input (inserting an ApplyDeferred sync point)"
        );
    }

    #[test]
    fn attaches_initial_workspace_subtree_to_workspace_ui_root() {
        let mut app = build_app();

        let subtree = app.world_mut().spawn(Node::default()).id();
        let workspace = app
            .world_mut()
            .spawn((
                WorkspaceMarker,
                AttachedWorkspace,
                WorkspaceUiSubtree(subtree),
            ))
            .id();
        app.world_mut()
            .entity_mut(subtree)
            .insert(ChildOf(workspace));

        app.update();

        let workspace_ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceUiRoot>>()
            .single(app.world())
            .expect("WorkspaceUiRoot");
        let parent = app
            .world()
            .get::<ChildOf>(subtree)
            .expect("subtree has parent")
            .parent();
        assert_eq!(
            parent, workspace_ui_root,
            "active workspace's subtree must be under WorkspaceUiRoot"
        );
    }

    #[test]
    fn switching_active_workspace_parks_previous_subtree_under_its_workspace_entity() {
        let mut app = build_app();

        let subtree_a = app.world_mut().spawn(Node::default()).id();
        let workspace_a = app
            .world_mut()
            .spawn((
                WorkspaceMarker,
                AttachedWorkspace,
                WorkspaceUiSubtree(subtree_a),
            ))
            .id();
        app.world_mut()
            .entity_mut(subtree_a)
            .insert(ChildOf(workspace_a));

        let subtree_b = app.world_mut().spawn(Node::default()).id();
        let workspace_b = app
            .world_mut()
            .spawn((WorkspaceMarker, WorkspaceUiSubtree(subtree_b)))
            .id();
        app.world_mut()
            .entity_mut(subtree_b)
            .insert(ChildOf(workspace_b));

        app.update();

        app.world_mut()
            .entity_mut(workspace_a)
            .remove::<AttachedWorkspace>();
        app.world_mut()
            .entity_mut(workspace_b)
            .insert(AttachedWorkspace);
        app.update();

        let workspace_ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceUiRoot>>()
            .single(app.world())
            .expect("WorkspaceUiRoot");
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
            parent_a, workspace_a,
            "previous subtree must park under its Workspace entity"
        );
        assert_eq!(
            parent_b, workspace_ui_root,
            "new subtree must attach to WorkspaceUiRoot"
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
    fn in_pane_surface_switch_slots_new_surface() {
        use crate::ui::Slotted;
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{AttachedWorkspace, MultiplexerCommands};

        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let pane =
            app.world_mut()
                .run_system_once(
                    |mux: MultiplexerCommands,
                     workspaces: Query<
                        Entity,
                        (With<WorkspaceMarker>, With<AttachedWorkspace>),
                    >| {
                        let workspace = workspaces.iter().next()?;
                        mux.workspaces_active_pane(workspace)
                    },
                )
                .unwrap()
                .expect("bootstrap workspace + active pane");

        // Add an in-pane surface, then make it active. The slot/park system
        // must slot the new surface (Node + Slotted) into the pane.
        let added = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| mux.add_surface(pane))
            .unwrap();
        app.world_mut().flush();
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_surface(pane, added).unwrap();
            })
            .unwrap();
        app.update();
        app.update();

        assert!(
            app.world().get::<Slotted>(added).is_some(),
            "the newly-activated surface must be slotted (carry Slotted)"
        );
        assert!(
            app.world().get::<Node>(added).is_some(),
            "the slotted surface must be decorated with a Node"
        );
    }

    #[test]
    fn inactive_surface_within_active_workspace_parks_under_workspace_entity() {
        use crate::ui::Slotted;
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{AttachedWorkspace, MultiplexerCommands};

        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let (workspace, pane, first_surface) =
            app.world_mut()
                .run_system_once(
                    |mux: MultiplexerCommands,
                     workspaces: Query<
                        Entity,
                        (With<WorkspaceMarker>, With<AttachedWorkspace>),
                    >| {
                        let workspace = workspaces.iter().next()?;
                        let pane = mux.workspaces_active_pane(workspace)?;
                        let surface = mux.panes_active_surface(pane)?;
                        Some((workspace, pane, surface))
                    },
                )
                .unwrap()
                .expect("bootstrap workspace + pane + first_surface");

        assert!(
            app.world().get::<Slotted>(first_surface).is_some(),
            "first surface must be slotted after initial chrome build"
        );

        let second_surface = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| mux.add_surface(pane))
            .unwrap();
        app.world_mut().flush();

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_surface(pane, second_surface).unwrap();
            })
            .unwrap();
        app.update();
        app.update();

        let first_parent = app
            .world()
            .get::<ChildOf>(first_surface)
            .map(|c| c.parent());
        assert_eq!(
            first_parent,
            Some(workspace),
            "inactive surface must be parked under the Workspace entity (non-Node, walker-skipped)"
        );
        assert!(
            app.world().get::<Slotted>(first_surface).is_none(),
            "parked surface must no longer carry Slotted"
        );
    }

    #[test]
    fn parked_subtree_has_no_computed_node_updates() {
        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        // Create a second workspace entity with a WorkspaceUiSubtree but no AttachedWorkspace.
        let inactive_workspace = {
            let world = app.world_mut();
            let subtree = world.spawn(Node::default()).id();
            let workspace_entity = world
                .spawn((
                    WorkspaceMarker,
                    WorkspaceUiSubtree(subtree),
                    Name::new("inactive"),
                ))
                .id();
            world.entity_mut(subtree).insert(ChildOf(workspace_entity));
            subtree
        };
        app.update();
        app.update();

        for _ in 0..5 {
            app.update();
        }
        let computed = app
            .world()
            .get::<bevy::ui::ComputedNode>(inactive_workspace);
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
    fn workspace_subtree_root_has_explicit_sizing() {
        let (mut app, _guard) = make_test_app_v2();
        app.update();
        app.update();

        let active_subtree = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<&WorkspaceUiSubtree, With<AttachedWorkspace>>();
            q.single(world).expect("one attached subtree").0
        };

        let node = app
            .world()
            .get::<bevy::ui::Node>(active_subtree)
            .expect("WorkspaceUiSubtree root must have a Node component");
        assert_eq!(
            node.width,
            bevy::ui::Val::Percent(100.0),
            "subtree root must set width: Percent(100) so it fills WorkspaceUiRoot",
        );
        assert_eq!(
            node.height,
            bevy::ui::Val::Percent(100.0),
            "subtree root must set height: Percent(100) so it fills WorkspaceUiRoot",
        );
    }

    #[test]
    fn new_workspace_action_reparents_new_subtree_to_workspace_ui_root() {
        let (mut app, _guard) = make_test_app_v2();
        // Two ticks for Startup + first Update so bootstrap settles and
        // sync_active_workspace runs at least once in PostUpdate.
        app.update();
        app.update();

        let workspace_ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<WorkspaceUiRoot>>()
            .single(app.world())
            .expect("WorkspaceUiRoot");
        let bootstrap_workspace = app
            .world_mut()
            .query_filtered::<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>()
            .single(app.world())
            .expect("exactly one bootstrap workspace");
        let bootstrap_subtree = app
            .world()
            .get::<WorkspaceUiSubtree>(bootstrap_workspace)
            .expect("bootstrap workspace has WorkspaceUiSubtree pointer")
            .0;
        assert_eq!(
            app.world()
                .get::<ChildOf>(bootstrap_subtree)
                .expect("bootstrap subtree has ChildOf")
                .parent(),
            workspace_ui_root,
            "bootstrap subtree must start under WorkspaceUiRoot",
        );

        app.world_mut()
            .trigger(crate::action::workspace::NewWorkspaceActionEvent {
                workspace: bootstrap_workspace,
            });
        // One tick for commands to flush + chrome to build, one for
        // PostUpdate sync_active_workspace to react.
        app.update();
        app.update();

        let new_workspace = app
            .world_mut()
            .query_filtered::<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>()
            .single(app.world())
            .expect("exactly one attached workspace after CMD+R");
        assert_ne!(
            new_workspace, bootstrap_workspace,
            "new workspace entity must differ from bootstrap",
        );

        let new_subtree = app
            .world()
            .get::<WorkspaceUiSubtree>(new_workspace)
            .expect("new workspace has WorkspaceUiSubtree pointer")
            .0;
        assert_eq!(
            app.world()
                .get::<ChildOf>(new_subtree)
                .expect("new subtree has ChildOf")
                .parent(),
            workspace_ui_root,
            "new workspace's subtree must be reparented to WorkspaceUiRoot",
        );

        let old_subtree = app
            .world()
            .get::<WorkspaceUiSubtree>(bootstrap_workspace)
            .expect("old workspace retains WorkspaceUiSubtree pointer")
            .0;
        assert_eq!(
            app.world()
                .get::<ChildOf>(old_subtree)
                .expect("old subtree has ChildOf")
                .parent(),
            bootstrap_workspace,
            "old workspace's subtree must be parked under its workspace entity",
        );
    }
}
