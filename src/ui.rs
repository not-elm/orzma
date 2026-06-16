//! Bevy UI Plugin and incremental chrome systems. Per-pane chrome (tab bar +
//! surface slot) is built once on `Added<PaneMarker>` by `crate::ui::chrome`
//! and diffed on `Changed<…>`; the active surface is slotted/parked via the
//! `Slotted` marker. Per-workspace UI subtrees are owned by their Workspace
//! entity and attached/parked by `sync_active_workspace`. The status bar
//! rebuilds independently via
//! `status_bar_sync::rebuild_status_bar_on_workspace_set_change` when the
//! workspace list or `AttachedWorkspace` marker changes. The multiplexer
//! Surface entity *is* its render host: it carries its own `Node` +
//! `TerminalSurfaceMarker` — the active surface (`Slotted`) under its pane's
//! surface slot, inactive surfaces under the owning Workspace entity (a
//! non-Node walker-skipped park).

use crate::ui::chrome::OzmuxChromePlugin;
use crate::ui::root::OzmuxUiRootPlugin;
use crate::ui::terminal::OzmuxTerminalUiPlugin;
use crate::ui::workspace::OzmuxWorkspaceUiPlugin;
use bevy::prelude::*;
use std::path::PathBuf;

pub(crate) mod chrome;
pub(crate) mod confirm_prompt;
pub mod copy_mode;
pub mod copy_mode_indicator;
pub(crate) mod copy_search;
pub(crate) mod ime_overlay;
pub mod palette;
pub mod root;
pub mod status_bar;
pub mod status_bar_sync;
#[cfg(test)]
pub(crate) mod stress_test;
pub(crate) mod surface;
pub mod tab_bar;
pub(crate) mod tab_input;
pub(crate) mod tab_label;
pub mod terminal;
pub(crate) mod tmux_dialog;
pub(crate) mod tmux_pane_focus;
pub(crate) mod tmux_window_bar;
pub(crate) mod tmux_window_bar_input;
pub mod workspace;

/// Marker for the single root UI Node entity. Spawned once in Startup,
/// never despawned. Hosts `WorkspaceUiRoot` (the attachment point for the
/// active workspace), the tmux window status bar (`WindowBarRoot`), and —
/// in non-tmux mode — the legacy `StatusBarRoot`, as direct children.
#[derive(Component)]
pub struct UiRoot;

/// Marker for the single attachment-point `Node` child of `UiRoot` that
/// receives whichever Workspace's `WorkspaceUiSubtree` is currently attached.
/// `sync_active_workspace` reparents subtrees between this and their owning
/// Workspace entity. Spawned once in Startup; never despawned.
#[derive(Component)]
pub struct WorkspaceUiRoot;

/// Marks the Surface entity currently slotted into its pane's visible
/// `surface_slot` (i.e. the active surface). Inactive surfaces are parked
/// under a non-`Node` parent and keep this marker removed.
///
/// # Invariants
///
/// Geometric hit-tests (`resolve_pane_at_phys`) MUST filter on this marker:
/// a parked surface is excluded from layout, so its `ComputedNode` retains
/// stale, often window-sized geometry. Without this filter a click resolves
/// to a parked surface of an already-active pane and focus never moves.
#[derive(Component)]
pub struct Slotted;

/// Marks a terminal Surface entity. `finish_terminal_setup` queries for
/// `With<TerminalSurfaceMarker>` to find surfaces that need a `TerminalBundle`
/// + `TerminalRenderBundle` attached.
#[derive(Component)]
pub struct TerminalSurfaceMarker;

/// On a tab-bar Node: marks it clickable and records which Surface (in which
/// Pane) selecting it activates. Read by `drive_tab_clicks` / `tab_hover_cursor`.
#[derive(Component, Clone, Copy)]
pub(crate) struct TabButton {
    pub(crate) pane: Entity,
    pub(crate) surface: Entity,
}

/// Records that `TerminalBundle::spawn` failed for this host, so
/// `finish_terminal_setup` will not retry on subsequent frames.
#[derive(Component)]
pub struct TerminalSpawnFailed;

/// Marker for the pane frame — the Pane entity's own Node, stamped by
/// `build_pane_chrome` once its chrome (tab bar + surface slot) exists. Used
/// by tests; not load-bearing for runtime.
#[derive(Component)]
pub struct PaneFrame;

/// Resolved `$HOME` at startup (`None` if unset). Read by `refresh_pane_tabs`
/// to home-abbreviate terminal tab paths; the value matches the terminal
/// spawner's `$HOME` fallback so the tab agrees with where the shell started.
#[derive(Resource)]
pub(crate) struct HomeDir(pub(crate) Option<PathBuf>);

/// Bevy Plugin wiring the native Bevy UI rebuild pipeline.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(HomeDir(std::env::var_os("HOME").map(PathBuf::from)))
            .add_plugins((
                OzmuxUiRootPlugin,
                OzmuxWorkspaceUiPlugin,
                OzmuxChromePlugin,
                OzmuxTerminalUiPlugin,
            ))
            .add_systems(
                Update,
                status_bar_sync::rebuild_status_bar_on_workspace_set_change
                    .run_if(not(status_bar_sync::tmux_projection_present)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::OzmuxBootstrapPlugin;
    use crate::configs::OzmuxConfigsPlugin;
    use bevy::asset::AssetPlugin;
    use bevy::image::ImagePlugin;
    use bevy::render::storage::ShaderStorageBuffer;
    use bevy::window::{PrimaryWindow, WindowResolution};
    use ozma_tty_renderer::material::TerminalUiMaterial;
    use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};
    use ozmux_multiplexer::{AttachedWorkspace, MultiplexerCommands, MultiplexerPlugin};

    /// Collects the rendered text of every tab (the `Text` child of each
    /// `TabButton` node). Order is unspecified — fine for single-tab assertions.
    fn tab_texts(world: &mut World) -> Vec<String> {
        let tabs: Vec<Entity> = world
            .query_filtered::<Entity, With<TabButton>>()
            .iter(world)
            .collect();
        let mut out = Vec::new();
        for tab in tabs {
            let kids: Vec<Entity> = world
                .get::<Children>(tab)
                .map(|c| c.iter().collect())
                .unwrap_or_default();
            for k in kids {
                if let Some(text) = world.get::<bevy::ui::widget::Text>(k) {
                    out.push(text.0.clone());
                }
            }
        }
        out
    }

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
            .add_plugins(MultiplexerPlugin)
            .add_plugins(OzmuxConfigsPlugin)
            .add_plugins(OzmuxBootstrapPlugin)
            .add_plugins(OzmuxUiPlugin)
            // NOTE: production no longer seeds a workspace at boot (tmux owns the
            // window now — see `bootstrap.rs`), but these tests exercise the
            // legacy multiplexer UI, which still requires an attached workspace.
            // Seed one here, replacing the seed that `OzmuxBootstrapPlugin` used
            // to provide.
            .add_systems(Startup, |mut mux: MultiplexerCommands| {
                let _ = mux.spawn_attached_workspace();
            });

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
    fn bootstrap_builds_pane_chrome_and_pane_frame() {
        use crate::ui::chrome::PaneChrome;
        let (mut app, _guard) = make_test_app();
        // NOTE: two `app.update()` calls are required here (and in every test that
        // needs visible chrome): the first tick runs Startup systems (bootstrap +
        // setup_root_camera_and_ui_root); the second tick runs the first Update
        // pass where `build_pane_chrome` fires on the `Added<PaneMarker>`.
        app.update();
        app.update();

        let world = app.world_mut();
        let chrome_count = world
            .query_filtered::<Entity, With<PaneChrome>>()
            .iter(world)
            .count();
        let pane_frame_count = world
            .query_filtered::<Entity, With<PaneFrame>>()
            .iter(world)
            .count();

        assert_eq!(
            chrome_count, 1,
            "expected exactly one PaneChrome after bootstrap"
        );
        assert_eq!(
            pane_frame_count, 1,
            "expected exactly one pane frame after bootstrap"
        );
    }

    #[test]
    fn surface_entity_persists_across_surface_switch() {
        use ozmux_multiplexer::SurfaceMarker;
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let surface_before = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, (With<SurfaceMarker>, With<Node>)>();
            q.iter(world)
                .next()
                .expect("at least one slotted surface after chrome build")
        };

        // A surface switch parks the previous surface but never despawns it.
        {
            use bevy::ecs::system::RunSystemOnce;
            use ozmux_multiplexer::{MultiplexerCommands, WorkspaceMarker};
            let pane = app
                .world_mut()
                .run_system_once(
                    |mux: MultiplexerCommands,
                     workspaces: Query<
                        Entity,
                        (With<WorkspaceMarker>, With<AttachedWorkspace>),
                    >| {
                        mux.workspaces_active_pane(workspaces.iter().next().unwrap())
                    },
                )
                .unwrap()
                .unwrap();
            let new_surface = app
                .world_mut()
                .run_system_once(move |mut mux: MultiplexerCommands| mux.add_surface(pane))
                .unwrap();
            app.world_mut().flush();
            app.world_mut()
                .run_system_once(move |mut mux: MultiplexerCommands| {
                    mux.set_active_surface(pane, new_surface).unwrap();
                })
                .unwrap();
        }
        app.update();

        assert!(
            app.world().get_entity(surface_before).is_ok(),
            "the Surface entity (= its own host) must survive a surface switch"
        );
        assert!(
            app.world().get::<Node>(surface_before).is_some(),
            "the parked surface keeps its Node"
        );
    }

    #[test]
    fn split_pane_produces_two_pane_frames() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, Side, SplitOrientation, WorkspaceMarker};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 workspaces: bevy::prelude::Query<
                    bevy::prelude::Entity,
                    (With<WorkspaceMarker>, With<AttachedWorkspace>),
                >| {
                    let workspace = workspaces.iter().next().expect("workspace");
                    let pane = mux.workspaces_active_pane(workspace).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        app.update();

        let pane_frame_count = app
            .world_mut()
            .query_filtered::<bevy::prelude::Entity, With<PaneFrame>>()
            .iter(app.world())
            .count();
        assert_eq!(pane_frame_count, 2, "split must produce two pane frames");
    }

    #[test]
    fn closing_pane_despawns_its_surface_via_linked_spawn() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{
            MultiplexerCommands, Side, SplitOrientation, SurfaceMarker, WorkspaceMarker,
        };

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 workspaces: bevy::prelude::Query<
                    bevy::prelude::Entity,
                    (With<WorkspaceMarker>, With<AttachedWorkspace>),
                >| {
                    let workspace = workspaces.iter().next().expect("workspace");
                    let pane = mux.workspaces_active_pane(workspace).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        app.update();

        let surfaces_after_split = app
            .world_mut()
            .query_filtered::<Entity, With<SurfaceMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(surfaces_after_split, 2, "two surfaces after split");

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 workspaces: bevy::prelude::Query<
                    bevy::prelude::Entity,
                    (With<WorkspaceMarker>, With<AttachedWorkspace>),
                >| {
                    let workspace = workspaces.iter().next().expect("workspace");
                    let pane = mux.workspaces_active_pane(workspace).expect("active pane");
                    mux.close_pane(pane).expect("close_pane");
                },
            )
            .unwrap();
        app.update();

        let surfaces_after_close = app
            .world_mut()
            .query_filtered::<Entity, With<SurfaceMarker>>()
            .iter(app.world())
            .count();
        assert_eq!(
            surfaces_after_close, 1,
            "closing a pane must cascade-despawn its surface (Surfaces linked_spawn)"
        );
    }

    #[test]
    fn surface_entity_not_caught_in_despawn_cascade() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, SurfaceMarker, WorkspaceMarker};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let entity_before = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, (With<SurfaceMarker>, With<Node>)>();
            q.iter(world).next().expect("at least one slotted surface")
        };

        // Rename via MultiplexerCommands — a workspace mutation that must not
        // disturb the persistent surface entity.
        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands, workspaces: Query<Entity, With<WorkspaceMarker>>| {
                    let workspace = workspaces.iter().next().expect("workspace");
                    mux.rename_workspace(workspace, "second-rename".into())
                        .unwrap();
                },
            )
            .unwrap();
        app.update();

        assert!(
            app.world().get_entity(entity_before).is_ok(),
            "surface entity must still exist after a workspace mutation — load-bearing for stable handles"
        );
    }

    #[test]
    fn focus_workspace_switch_does_not_orphan_inactive_workspace_surfaces() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, SurfaceMarker, WorkspaceMarker};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let surface_before = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, (With<SurfaceMarker>, With<Node>)>();
            q.iter(world).next().expect("at least one slotted surface")
        };

        let workspace_2 = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_workspace(Some("workspace-2".into())).workspace
            })
            .unwrap();
        app.world_mut().flush();

        let workspace_1 = app
            .world_mut()
            .query_filtered::<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>()
            .single(app.world())
            .expect("workspace 1 still attached");

        app.world_mut()
            .entity_mut(workspace_1)
            .remove::<AttachedWorkspace>();
        app.world_mut()
            .entity_mut(workspace_2)
            .insert(AttachedWorkspace);
        app.update();

        assert!(
            app.world().get_entity(surface_before).is_ok(),
            "workspace 1's surface must survive when workspace 2 becomes active"
        );
    }

    #[test]
    fn inactive_surface_persists_across_focus_switch() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, WorkspaceMarker};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let (workspace, pane, first_surface) = app
            .world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 workspaces: bevy::prelude::Query<
                    bevy::prelude::Entity,
                    (With<WorkspaceMarker>, With<AttachedWorkspace>),
                >| {
                    let workspace = workspaces.iter().next()?;
                    let pane = mux.workspaces_active_pane(workspace)?;
                    let surface = mux.panes_active_surface(pane)?;
                    Some((workspace, pane, surface))
                },
            )
            .unwrap()
            .expect("bootstrap workspace + pane + surface");

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

        assert!(
            app.world().get_entity(first_surface).is_ok(),
            "first surface must survive when the second surface becomes active"
        );

        let first_parent = app
            .world()
            .get::<bevy::prelude::ChildOf>(first_surface)
            .map(|c| c.parent());
        assert_eq!(
            first_parent,
            Some(workspace),
            "inactive surface must be parked under the workspace entity (no Node, walker-skipped)"
        );
    }

    #[test]
    fn status_bar_root_spawned_on_startup() {
        use crate::ui::status_bar_sync::StatusBarRoot;
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let count = app
            .world_mut()
            .query_filtered::<Entity, With<StatusBarRoot>>()
            .iter(app.world())
            .count();
        assert!(count > 0, "StatusBarRoot must be present after startup");
    }

    #[test]
    fn attached_workspace_marker_present_after_bootstrap() {
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let count = app
            .world_mut()
            .query_filtered::<Entity, With<AttachedWorkspace>>()
            .iter(app.world())
            .count();
        assert_eq!(count, 1, "exactly one AttachedWorkspace after bootstrap");
    }

    /// Collects `(pane, PaneDim.0)` for every terminal surface that
    /// `sync_terminal_dim` has assigned a `PaneDim`. The surface *is* its own
    /// host, so the owning pane is resolved via `SurfaceOf`.
    fn terminal_host_pane_dims(world: &mut World) -> Vec<(Entity, f32)> {
        use ozma_tty_renderer::material::PaneDim;
        use ozmux_multiplexer::SurfaceOf;
        world
            .query_filtered::<(&SurfaceOf, &PaneDim), With<TerminalSurfaceMarker>>()
            .iter(world)
            .map(|(o, d)| (o.0, d.0))
            .collect()
    }

    /// Headless-safe terminal mount. `finish_terminal_setup` spawns the shell
    /// from `$SHELL` (default `/bin/zsh`), which fails on runners lacking it
    /// (e.g. Linux CI), leaving the surface with no `MaterialNode` — so
    /// `sync_terminal_dim_on_mount` never assigns `PaneDim`. Stub the render
    /// material on any terminal surface that did not really mount, then tick so
    /// the dim systems run; a surface that mounted for real already carries a
    /// material.
    fn mount_terminal_hosts(app: &mut App) {
        let surfaces: Vec<Entity> = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, (
                    With<TerminalSurfaceMarker>,
                    Without<MaterialNode<TerminalUiMaterial>>,
                )>()
                .iter(world)
                .collect()
        };
        for surface in surfaces {
            let handle = app
                .world_mut()
                .resource_mut::<Assets<TerminalUiMaterial>>()
                .add(TerminalUiMaterial::default());
            app.world_mut()
                .entity_mut(surface)
                .insert(MaterialNode(handle));
        }
        app.update();
    }

    #[test]
    fn split_dims_inactive_terminal_keeps_active_bright() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, Side, SplitOrientation, WorkspaceMarker};

        let (mut app, _guard) = make_test_app();
        for _ in 0..3 {
            app.update();
        }

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 workspaces: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>| {
                    let workspace = workspaces.iter().next().expect("workspace");
                    let pane = mux.workspaces_active_pane(workspace).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        for _ in 0..4 {
            app.update();
        }
        mount_terminal_hosts(&mut app);

        let active_pane =
            app.world_mut()
                .run_system_once(
                    |mux: MultiplexerCommands,
                     workspaces: Query<
                        Entity,
                        (With<WorkspaceMarker>, With<AttachedWorkspace>),
                    >| {
                        let workspace = workspaces.iter().next().unwrap();
                        mux.workspaces_active_pane(workspace).unwrap()
                    },
                )
                .unwrap();

        let dims = terminal_host_pane_dims(app.world_mut());
        assert_eq!(dims.len(), 2, "two terminal hosts after split");
        for (pane, dim) in dims {
            if pane == active_pane {
                assert_eq!(dim, 1.0, "active terminal is full-bright");
            } else {
                assert_eq!(dim, 0.5, "inactive terminal dimmed to default factor");
            }
        }
    }

    #[test]
    fn lone_terminal_pane_is_full_bright() {
        use ozma_tty_renderer::material::PaneDim;

        let (mut app, _guard) = make_test_app();
        for _ in 0..4 {
            app.update();
        }
        mount_terminal_hosts(&mut app);

        let world = app.world_mut();
        let dims: Vec<f32> = world
            .query_filtered::<&PaneDim, With<TerminalSurfaceMarker>>()
            .iter(world)
            .map(|d| d.0)
            .collect();
        assert_eq!(dims.len(), 1, "exactly one terminal host after bootstrap");
        assert_eq!(dims[0], 1.0, "lone active terminal is full-bright");
    }

    #[test]
    fn disabled_config_dims_nothing() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, Side, SplitOrientation, WorkspaceMarker};

        let (mut app, _guard) = make_test_app();
        // Override to disabled BEFORE hosts mount, so the first PaneDim
        // assignment sees enabled = false.
        let custom = ozmux_configs::OzmuxConfigs {
            inactive_pane: ozmux_configs::inactive_pane::InactivePaneConfig {
                enabled: false,
                opacity: 0.5,
                color: "#000000".to_string(),
                dim: 0.3,
            },
            ..Default::default()
        };
        app.insert_resource(crate::configs::OzmuxConfigsResource(custom));
        for _ in 0..3 {
            app.update();
        }

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 workspaces: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>| {
                    let workspace = workspaces.iter().next().expect("workspace");
                    let pane = mux.workspaces_active_pane(workspace).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        for _ in 0..4 {
            app.update();
        }
        mount_terminal_hosts(&mut app);

        let world = app.world_mut();
        let dims = terminal_host_pane_dims(world);
        assert_eq!(dims.len(), 2, "two terminal hosts after split");
        assert!(
            dims.iter().all(|(_, d)| *d == 1.0),
            "disabled dimming leaves every terminal full-bright (got {dims:?})"
        );
    }

    #[test]
    fn focus_change_moves_terminal_dim() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, Side, SplitOrientation, WorkspaceMarker};

        let (mut app, _guard) = make_test_app();
        for _ in 0..3 {
            app.update();
        }

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 workspaces: Query<Entity, (With<WorkspaceMarker>, With<AttachedWorkspace>)>| {
                    let workspace = workspaces.iter().next().expect("workspace");
                    let pane = mux.workspaces_active_pane(workspace).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        for _ in 0..4 {
            app.update();
        }
        mount_terminal_hosts(&mut app);

        let (workspace, target_pane) =
            app.world_mut()
                .run_system_once(
                    |mux: MultiplexerCommands,
                     workspaces: Query<
                        Entity,
                        (With<WorkspaceMarker>, With<AttachedWorkspace>),
                    >| {
                        let workspace = workspaces.iter().next().unwrap();
                        let active = mux.workspaces_active_pane(workspace).unwrap();
                        let target = mux
                            .panes_of_workspace(workspace)
                            .find(|p| *p != active)
                            .expect("a non-active pane exists after split");
                        (workspace, target)
                    },
                )
                .unwrap();

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_pane(workspace, target_pane)
                    .expect("set_active_pane");
            })
            .unwrap();
        for _ in 0..2 {
            app.update();
        }

        let dims = terminal_host_pane_dims(app.world_mut());
        assert_eq!(dims.len(), 2, "two terminal hosts");
        for (pane, dim) in dims {
            if pane == target_pane {
                assert_eq!(dim, 1.0, "newly-focused terminal is full-bright");
            } else {
                assert_eq!(dim, 0.5, "newly-inactive terminal is dimmed");
            }
        }
    }

    #[test]
    fn rebuilt_tabs_carry_tab_button() {
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let world = app.world_mut();
        let tab_count = world
            .query_filtered::<Entity, With<TabButton>>()
            .iter(world)
            .count();
        assert_eq!(
            tab_count, 1,
            "the bootstrap pane has one surface, so its tab bar has one TabButton-tagged tab",
        );
    }

    #[test]
    fn status_bar_chips_appear_in_workspace_creation_order_after_cmd_r() {
        use crate::ui::status_bar_sync::StatusBarRoot;
        use bevy::ecs::system::RunSystemOnce;

        let (mut app, _guard) = make_test_app();
        // Two ticks for Startup + first Update so bootstrap settles and
        // the initial status bar is built.
        app.update();
        app.update();

        // Mint a second attached workspace directly (tmux owns this in
        // production now); the status bar reacts to the resulting ECS state.
        let attached = app
            .world_mut()
            .query_filtered::<Entity, (
                With<ozmux_multiplexer::WorkspaceMarker>,
                With<AttachedWorkspace>,
            )>()
            .single(app.world())
            .expect("one attached workspace before second workspace");
        app.world_mut()
            .entity_mut(attached)
            .remove::<AttachedWorkspace>();
        app.world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.spawn_attached_workspace();
            })
            .unwrap();
        // One tick for commands to flush + status bar rebuild to enqueue,
        // one for rebuild's commands to flush.
        app.update();
        app.update();

        // Walk the StatusBarRoot's descendants and collect every chip's
        // Name as a Text node. The chip order in DFS = insertion order =
        // left-to-right visual order in FlexDirection::Row.
        let world = app.world_mut();
        let bar = world
            .query_filtered::<Entity, With<StatusBarRoot>>()
            .single(world)
            .expect("StatusBarRoot present");
        let mut chip_names: Vec<String> = Vec::new();
        let mut stack: Vec<Entity> = vec![bar];
        while let Some(e) = stack.pop() {
            if let Some(text) = world.get::<bevy::ui::widget::Text>(e) {
                chip_names.push(text.0.clone());
            }
            if let Some(children) = world.get::<Children>(e) {
                // Push children in reverse so DFS visits them left-to-right.
                let mut kids: Vec<Entity> = children.iter().collect();
                kids.reverse();
                stack.extend(kids);
            }
        }
        // Filter to just workspace chips ("workspace1", "workspace2", ...).
        let workspace_chips: Vec<String> = chip_names
            .into_iter()
            .filter(|n| n.starts_with("workspace"))
            .collect();
        assert_eq!(
            workspace_chips,
            vec!["workspace1".to_string(), "workspace2".to_string()],
            "status bar must show workspace1 leftmost, workspace2 to its right",
        );
    }

    #[test]
    fn terminal_tab_node_is_width_capped() {
        use bevy::ui::OverflowAxis;

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let world = app.world_mut();
        let tab = world
            .query_filtered::<Entity, With<TabButton>>()
            .iter(world)
            .next()
            .expect("one tab after bootstrap");
        let node = world.get::<Node>(tab).expect("tab has a Node");
        assert_eq!(node.max_width, Val::Px(crate::theme::TAB_MAX_WIDTH_PX));
        assert_eq!(node.overflow.x, OverflowAxis::Clip);
    }

    #[test]
    fn osc7_current_dir_updates_tab() {
        use ozma_tty_engine::TerminalCurrentDir;
        use ozmux_multiplexer::SurfaceMarker;

        let (mut app, _guard) = make_test_app();
        app.insert_resource(HomeDir(None));
        app.update();
        app.update();

        let surface = app
            .world_mut()
            .query_filtered::<Entity, With<SurfaceMarker>>()
            .iter(app.world())
            .next()
            .expect("a surface exists after rebuild");
        app.world_mut().trigger(TerminalCurrentDir {
            entity: surface,
            path: "/tmp/proj".into(),
        });
        for _ in 0..3 {
            app.update();
        }

        assert_eq!(tab_texts(app.world_mut()), vec!["/tmp/proj".to_string()]);
    }

    #[test]
    fn cwd_change_refreshes_tab_without_layout_change() {
        use ozmux_multiplexer::{Cwd, SurfaceMarker};

        let (mut app, _guard) = make_test_app();
        app.insert_resource(HomeDir(None));
        app.update();
        app.update();

        let surface = app
            .world_mut()
            .query_filtered::<Entity, With<SurfaceMarker>>()
            .single(app.world())
            .expect("one bootstrap surface");
        app.world_mut()
            .entity_mut(surface)
            .insert(Cwd("/tmp/proj".into()));
        app.update();
        app.update();

        assert_eq!(tab_texts(app.world_mut()), vec!["/tmp/proj".to_string()]);
    }
}
