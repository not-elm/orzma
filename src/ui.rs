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

use crate::system_set::OzmuxSystems;
use crate::ui::registry::ActivityEntityRegistry;
use crate::ui::root::OzmuxUiRootPlugin;
use crate::ui::session::OzmuxSessionUiPlugin;
use crate::ui::terminal::OzmuxTerminalUiPlugin;
use bevy::prelude::*;

pub(crate) mod activity;
pub mod copy_mode;
pub mod copy_mode_indicator;
pub(crate) mod ime_overlay;
pub mod layout;
pub mod palette;
pub mod registry;
pub mod root;
pub mod session;
pub mod status_bar;
pub mod status_bar_sync;
#[cfg(test)]
pub(crate) mod stress_test;
pub mod tab_bar;
pub mod terminal;

/// Marker for the single root UI Node entity. Spawned once in Startup,
/// never despawned. Hosts `SessionUiRoot` (the attachment point for the
/// active session) and `StatusBarRoot` as direct children.
#[derive(Component)]
pub struct UiRoot;

/// Marker for the single attachment-point `Node` child of `UiRoot` that
/// receives whichever Session's `SessionUiSubtree` is currently attached.
/// `sync_active_session` reparents subtrees between this and their owning
/// Session entity. Spawned once in Startup; never despawned.
#[derive(Component)]
pub struct SessionUiRoot;

/// Marker for every transient UI Node (status bar, tab bar, pane frame,
/// split container, placeholder activity content). Rebuilds query this
/// and despawn every match. Activity host entities must NOT carry this.
#[derive(Component)]
pub struct StructuralNode;

/// Marker for the stable per-activity host entity. Survives structural
/// rebuilds; re-parented via `ChildOf` each rebuild. The `ActivityId →
/// Entity` mapping is owned by `ActivityEntityRegistry`; this marker
/// exists only so queries can filter for activity hosts.
#[derive(Component)]
pub struct ActivityHostNode;

/// Marks an Activity Host whose `kind` is `Terminal`. `finish_terminal_setup`
/// queries for `With<TerminalActivityMarker>` to find hosts that need a
/// `TerminalBundle` + `TerminalRenderBundle` attached.
#[derive(Component)]
pub struct TerminalActivityMarker;

/// Marks an Activity Host whose `kind` is `Extension`.
/// `finish_extension_setup` queries for `With<ExtensionActivityMarker>` to
/// find hosts that need a `bevy_cef` webview (`WebviewSource` +
/// `MaterialNode<WebviewUiMaterial>`) attached.
#[derive(Component)]
pub(crate) struct ExtensionActivityMarker;

/// Marks an Activity Host whose `kind` is `Browser`. The browser renderer
/// (`crate::browser_render`) queries `With<BrowserActivityMarker>` to find hosts
/// that need a native toolbar + a `bevy_cef` page webview attached.
#[derive(Component)]
pub(crate) struct BrowserActivityMarker;

/// On a browser activity host: points to its page-webview child entity. Its
/// presence also marks the host's chrome as already built (mount-once gate).
#[derive(Component)]
pub(crate) struct BrowserPageWebview(pub(crate) Entity);

/// On a browser page-webview child: points back to its owning activity host.
/// Lets navigation observers (which fire on the webview entity) reach the host.
#[derive(Component)]
pub(crate) struct PageWebviewOf(pub(crate) Entity);

/// On a browser activity host: the latest navigation state, written by the
/// `AddressChanged` / `LoadingStateChanged` observers and read by the toolbar
/// render + button-enablement systems.
#[derive(Component, Default, Clone)]
pub(crate) struct BrowserToolbarState {
    pub(crate) url: String,
    pub(crate) is_loading: bool,
    pub(crate) can_go_back: bool,
    pub(crate) can_go_forward: bool,
}

/// On a browser activity host: the address-bar edit buffer + caret. Pure edit
/// logic lives in `crate::browser_render`.
#[derive(Component, Default, Clone)]
pub(crate) struct AddressEdit {
    pub(crate) buffer: String,
    pub(crate) caret: usize,
}

/// On the address-bar node inside a browser toolbar: marks it and points to
/// its owning host. The node is a `Button` so it can be clicked to focus.
#[derive(Component)]
pub(crate) struct AddrBarText(pub(crate) Entity);

/// A toolbar navigation action a `BrowserNavButton` performs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NavAction {
    Back,
    Forward,
    Reload,
}

/// On a toolbar button: its owning host + the action it triggers.
#[derive(Component)]
pub(crate) struct BrowserNavButton {
    pub(crate) host: Entity,
    pub(crate) action: NavAction,
}

/// The browser activity host whose address bar currently owns the keyboard, or
/// `None`. Read by the browser editor + `dispatch_focused_key`.
#[derive(Resource, Default)]
pub(crate) struct AddressBarFocus(pub(crate) Option<Entity>);

/// Back-pointer from a stable Activity host entity to its owning
/// multiplexer Activity entity. Stamped by
/// `ActivityEntityRegistry::get_or_spawn`. `finish_terminal_setup` reads
/// this to resolve the host's multiplexer Pane / Session entities (via
/// `ChildOf`) so the spawned terminal's env carries the correct
/// `OZMUX_PANE_ID` for the `@memo` control bridge.
#[derive(Component)]
pub struct HostActivityEntity(pub Entity);

/// Records that `TerminalBundle::spawn` failed for this host, so
/// `finish_terminal_setup` will not retry on subsequent frames.
#[derive(Component)]
pub struct TerminalSpawnFailed;

/// Marker for the pane frame Node (the outermost Node of one
/// `Cell::Pane` subtree). Used by tests; not load-bearing for runtime.
#[derive(Component)]
pub struct PaneFrame;

/// Marks the per-pane dim veil — a translucent overlay node, last child of
/// the pane frame, drawn over both terminal and webview content when the
/// pane is NOT its session's active pane. `pane` is the multiplexer Pane
/// entity this veil belongs to; `sync_pane_dim` reads it to toggle
/// `Visibility` on focus changes.
#[derive(Component)]
pub(crate) struct PaneDimOverlay {
    pub(crate) pane: Entity,
}

/// Bevy Plugin wiring the native Bevy UI rebuild pipeline.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActivityEntityRegistry>()
            .add_plugins((
                OzmuxUiRootPlugin,
                OzmuxSessionUiPlugin,
                OzmuxTerminalUiPlugin,
            ))
            .add_systems(
                Update,
                (
                    // Host despawns must commit before the rebuild and activity
                    // setup observe them, else setup inserts a bundle onto a
                    // host this prune is despawning (insert-after-despawn panic).
                    registry::prune_registry_on_activity_removal.before(OzmuxSystems::SessionUi),
                    status_bar_sync::rebuild_status_bar_on_session_set_change,
                ),
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
    use bevy_terminal_renderer::material::TerminalUiMaterial;
    use bevy_terminal_renderer::{CellMetrics, TerminalCellMetricsResource};
    use ozmux_multiplexer::{AttachedSession, MultiplexerPlugin};

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
        // where `rebuild_session_ui` fires because the bootstrap session's
        // LayoutCells was just changed.
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
        use crate::ui::registry::ActivityEntityRegistry;
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let host_before = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, With<ActivityHostNode>>();
            q.iter(world)
                .next()
                .expect("at least one host after first rebuild")
        };

        {
            let world = app.world_mut();
            let session = world
                .query_filtered::<Entity, (
                    With<ozmux_multiplexer::SessionMarker>,
                    With<AttachedSession>,
                )>()
                .single(world)
                .expect("one attached session");
            world
                .entity_mut(session)
                .get_mut::<ozmux_multiplexer::LayoutCells>()
                .expect("LayoutCells")
                .set_changed();
        }
        app.update();

        let host_after = {
            let world = app.world_mut();
            let registry = world.resource::<ActivityEntityRegistry>();
            registry.iter_for_test().next().map(|(_, h)| h)
        };

        assert_eq!(
            Some(host_before),
            host_after,
            "activity host entity must survive a rebuild"
        );
    }

    #[test]
    fn split_pane_produces_two_pane_frames() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, SessionMarker, Side, SplitOrientation};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 sessions: bevy::prelude::Query<
                    bevy::prelude::Entity,
                    (With<SessionMarker>, With<AttachedSession>),
                >| {
                    let session = sessions.iter().next().expect("session");
                    let pane = mux.sessions_active_pane(session).expect("active pane");
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
    fn activity_registry_prunes_removed_activity() {
        use crate::ui::registry::ActivityEntityRegistry;
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, SessionMarker, Side, SplitOrientation};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 sessions: bevy::prelude::Query<
                    bevy::prelude::Entity,
                    (With<SessionMarker>, With<AttachedSession>),
                >| {
                    let session = sessions.iter().next().expect("session");
                    let pane = mux.sessions_active_pane(session).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        app.update();

        let registry_count_after_split = app
            .world()
            .resource::<ActivityEntityRegistry>()
            .len_for_test();
        assert_eq!(registry_count_after_split, 2, "two activities after split");

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 sessions: bevy::prelude::Query<
                    bevy::prelude::Entity,
                    (With<SessionMarker>, With<AttachedSession>),
                >| {
                    let session = sessions.iter().next().expect("session");
                    let pane = mux.sessions_active_pane(session).expect("active pane");
                    mux.close_pane(pane).expect("close_pane");
                },
            )
            .unwrap();
        app.update();

        let registry_count_after_close = app
            .world()
            .resource::<ActivityEntityRegistry>()
            .len_for_test();
        assert_eq!(
            registry_count_after_close, 1,
            "prune system must remove the closed activity from the registry"
        );
    }

    #[test]
    fn activity_host_not_caught_in_despawn_cascade() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, SessionMarker};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let entity_before = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, With<ActivityHostNode>>();
            q.iter(world).next().expect("at least one host")
        };

        // Rename via MultiplexerCommands — triggers Changed<Name> on the Session
        // which causes a rebuild in the next update.
        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands, sessions: Query<Entity, With<SessionMarker>>| {
                    let session = sessions.iter().next().expect("session");
                    mux.rename_session(session, "second-rename".into()).unwrap();
                },
            )
            .unwrap();
        app.update();

        assert!(
            app.world().get_entity(entity_before).is_ok(),
            "host entity must still exist after rebuild — load-bearing for stable handles"
        );
    }

    #[test]
    fn focus_session_switch_does_not_orphan_inactive_session_hosts() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, SessionMarker};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let host_before = {
            let world = app.world_mut();
            let mut q = world.query_filtered::<Entity, With<ActivityHostNode>>();
            q.iter(world).next().expect("at least one host")
        };

        let session_2 = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                mux.create_session(Some("session-2".into())).session
            })
            .unwrap();
        app.world_mut().flush();

        let session_1 = app
            .world_mut()
            .query_filtered::<Entity, (With<SessionMarker>, With<AttachedSession>)>()
            .single(app.world())
            .expect("session 1 still attached");

        app.world_mut()
            .entity_mut(session_1)
            .remove::<AttachedSession>();
        app.world_mut()
            .entity_mut(session_2)
            .insert(AttachedSession);
        app.update();

        assert!(
            app.world().get_entity(host_before).is_ok(),
            "session 1's activity host must survive when session 2 becomes active"
        );
    }

    #[test]
    fn inactive_activity_host_persists_across_focus_switch() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{ActivityKind, MultiplexerCommands, SessionMarker};

        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let (session, pane, first_activity) = app
            .world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 sessions: bevy::prelude::Query<
                    bevy::prelude::Entity,
                    (With<SessionMarker>, With<AttachedSession>),
                >| {
                    let session = sessions.iter().next()?;
                    let pane = mux.sessions_active_pane(session)?;
                    let activity = mux.panes_active_activity(pane)?;
                    Some((session, pane, activity))
                },
            )
            .unwrap()
            .expect("bootstrap session + pane + activity");

        let host_before = {
            let world = app.world_mut();
            let registry = world.resource::<crate::ui::registry::ActivityEntityRegistry>();
            registry
                .get(first_activity)
                .expect("first activity has a host")
        };

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

        {
            let world = app.world_mut();
            world
                .entity_mut(session)
                .get_mut::<ozmux_multiplexer::LayoutCells>()
                .expect("LayoutCells")
                .set_changed();
        }
        app.update();

        assert!(
            app.world().get_entity(host_before).is_ok(),
            "first activity host must survive when the second activity becomes active"
        );

        let host_parent = app
            .world()
            .get::<bevy::prelude::ChildOf>(host_before)
            .map(|c| c.parent());
        assert_eq!(
            host_parent,
            Some(session),
            "inactive host must be parked under the session entity (no Node, walker-skipped)"
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
    fn attached_session_marker_present_after_bootstrap() {
        let (mut app, _guard) = make_test_app();
        app.update();
        app.update();

        let count = app
            .world_mut()
            .query_filtered::<Entity, With<AttachedSession>>()
            .iter(app.world())
            .count();
        assert_eq!(count, 1, "exactly one AttachedSession after bootstrap");
    }

    /// Collects `(pane, PaneDim.0)` for every terminal host that
    /// `sync_terminal_dim` has assigned a `PaneDim`.
    fn terminal_host_pane_dims(world: &mut World) -> Vec<(Entity, f32)> {
        use bevy_terminal_renderer::material::PaneDim;
        let hosts: Vec<(Entity, f32)> = world
            .query_filtered::<(&HostActivityEntity, &PaneDim), With<TerminalActivityMarker>>()
            .iter(world)
            .map(|(h, d)| (h.0, d.0))
            .collect();
        hosts
            .into_iter()
            .filter_map(|(activity, dim)| {
                let pane = world.get::<ChildOf>(activity)?.parent();
                Some((pane, dim))
            })
            .collect()
    }

    #[test]
    fn split_dims_inactive_terminal_keeps_active_bright() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, SessionMarker, Side, SplitOrientation};

        let (mut app, _guard) = make_test_app();
        for _ in 0..3 {
            app.update();
        }

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next().expect("session");
                    let pane = mux.sessions_active_pane(session).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        for _ in 0..4 {
            app.update();
        }

        let active_pane = app
            .world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next().unwrap();
                    mux.sessions_active_pane(session).unwrap()
                },
            )
            .unwrap();

        {
            let world = app.world_mut();
            let overlay_count = world.query::<&PaneDimOverlay>().iter(world).count();
            assert_eq!(overlay_count, 0, "terminal panes get no veil overlay");
        }

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
    fn lone_terminal_pane_is_full_bright_and_unveiled() {
        use bevy_terminal_renderer::material::PaneDim;

        let (mut app, _guard) = make_test_app();
        for _ in 0..4 {
            app.update();
        }

        let world = app.world_mut();
        let overlay_count = world.query::<&PaneDimOverlay>().iter(world).count();
        assert_eq!(
            overlay_count, 0,
            "terminal panes must not get a veil overlay"
        );

        let dims: Vec<f32> = world
            .query_filtered::<&PaneDim, With<TerminalActivityMarker>>()
            .iter(world)
            .map(|d| d.0)
            .collect();
        assert_eq!(dims.len(), 1, "exactly one terminal host after bootstrap");
        assert_eq!(dims[0], 1.0, "lone active terminal is full-bright");
    }

    #[test]
    fn extension_pane_keeps_pickable_ignore_veil() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{ActivityKind, LayoutCells, MultiplexerCommands, SessionMarker};

        let (mut app, _guard) = make_test_app();
        for _ in 0..3 {
            app.update();
        }

        let pane = app
            .world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next().unwrap();
                    mux.sessions_active_pane(session).unwrap()
                },
            )
            .unwrap();
        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                let ext = mux.add_activity(
                    pane,
                    ActivityKind::Extension {
                        entry: "/tmp".into(),
                    },
                );
                mux.set_active_activity(pane, ext).unwrap();
            })
            .unwrap();
        // An activity switch reparents hosts via a rebuild; force it in the harness.
        app.world_mut()
            .run_system_once(
                |mut sessions: Query<&mut LayoutCells, With<SessionMarker>>| {
                    for mut lc in sessions.iter_mut() {
                        lc.set_changed();
                    }
                },
            )
            .unwrap();
        for _ in 0..2 {
            app.update();
        }

        let world = app.world_mut();
        let overlay = world
            .query_filtered::<Entity, With<PaneDimOverlay>>()
            .iter(world)
            .next()
            .expect("extension pane must have a veil overlay");
        let pickable = world
            .get::<Pickable>(overlay)
            .expect("veil must carry Pickable");
        assert!(!pickable.should_block_lower, "veil must not block lower");
        assert!(!pickable.is_hoverable, "veil must not be hoverable");
    }

    #[test]
    fn disabled_config_dims_nothing() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::{MultiplexerCommands, SessionMarker, Side, SplitOrientation};

        let (mut app, _guard) = make_test_app();
        // Override to disabled BEFORE hosts mount, so the first PaneDim
        // assignment and the veil decision both see enabled = false.
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
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next().expect("session");
                    let pane = mux.sessions_active_pane(session).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        for _ in 0..4 {
            app.update();
        }

        let world = app.world_mut();
        let overlay_count = world.query::<&PaneDimOverlay>().iter(world).count();
        assert_eq!(
            overlay_count, 0,
            "no veil overlays when dimming is disabled"
        );
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
        use ozmux_multiplexer::{MultiplexerCommands, SessionMarker, Side, SplitOrientation};

        let (mut app, _guard) = make_test_app();
        for _ in 0..3 {
            app.update();
        }

        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next().expect("session");
                    let pane = mux.sessions_active_pane(session).expect("active pane");
                    mux.split_pane(pane, Side::After, SplitOrientation::Horizontal)
                        .expect("split_pane");
                },
            )
            .unwrap();
        for _ in 0..4 {
            app.update();
        }

        let (session, target_pane) = app
            .world_mut()
            .run_system_once(
                |mux: MultiplexerCommands,
                 sessions: Query<Entity, (With<SessionMarker>, With<AttachedSession>)>| {
                    let session = sessions.iter().next().unwrap();
                    let active = mux.sessions_active_pane(session).unwrap();
                    let target = mux
                        .panes_of_session(session)
                        .find(|p| *p != active)
                        .expect("a non-active pane exists after split");
                    (session, target)
                },
            )
            .unwrap();

        app.world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.set_active_pane(session, target_pane)
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
    fn status_bar_chips_appear_in_session_creation_order_after_cmd_r() {
        use crate::ui::status_bar_sync::StatusBarRoot;
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::MultiplexerCommands;

        let (mut app, _guard) = make_test_app();
        // Two ticks for Startup + first Update so bootstrap settles and
        // the initial status bar is built.
        app.update();
        app.update();

        // Drive a CMD+R-equivalent dispatch_new_session.
        app.world_mut()
            .run_system_once(
                |mut mux: MultiplexerCommands,
                 mut commands: Commands,
                 mut counter: ResMut<crate::multiplexer::SessionNameCounter>,
                 attached_session: Query<
                    Entity,
                    (
                        With<ozmux_multiplexer::SessionMarker>,
                        With<AttachedSession>,
                    ),
                >| {
                    crate::input::dispatch_new_session(
                        &mut commands,
                        &mut mux,
                        &mut counter,
                        &attached_session,
                    );
                },
            )
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
        // Filter to just session chips ("session1", "session2", ...).
        let session_chips: Vec<String> = chip_names
            .into_iter()
            .filter(|n| n.starts_with("session"))
            .collect();
        assert_eq!(
            session_chips,
            vec!["session1".to_string(), "session2".to_string()],
            "status bar must show session1 leftmost, session2 to its right",
        );
    }
}
