//! Adoption lifecycle: bridges a detected `tmux -CC` handshake into a live
//! connection and tears it down again.
//!
//! When a Default-mode shell runs `tmux -CC`, the engine fires
//! [`ControlModeDetected`] on that terminal. [`on_control_mode_detected`] adopts
//! the terminal's PTY as the control-mode gateway, promotes it out of the
//! Default view (despawning the now-empty Default container so a fresh shell is
//! lazily re-spawned on return), inserts [`TmuxPresence`] to activate the drive
//! chain, and enters [`AppMode::Tmux`]. The matching teardown fires when tmux
//! ends the control client — either via `%exit` (a detach, where the shell
//! process survives) or the gateway child process actually exiting — closing the
//! connection, despawning the gateway, and returning to [`AppMode::Default`].

use crate::app_mode::{AppMode, DefaultModeUi};
use crate::ui::UiRoot;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_terminal::{KeyboardFocused, cells_for};
use ozma_tty_engine::{ControlModeDetected, TerminalChildExit, TerminalResize};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozmux_tmux::{
    ClientEvent, ControlEvent, EnumerationState, TmuxConnection, TmuxConnectionClosed,
    TmuxConnectionReset, TmuxEventBatch, TmuxPresence, TmuxProjectionSet, TransportEvent,
};

/// Registers the adoption observer and the teardown systems/observer.
pub(crate) struct AdoptPlugin;

impl Plugin for AdoptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GatewaySize>()
            .add_observer(on_control_mode_detected)
            .add_observer(on_gateway_child_exit)
            .add_systems(
                Update,
                teardown_on_exit_notification
                    .after(TmuxProjectionSet)
                    .run_if(resource_exists::<TmuxPresence>),
            )
            .add_systems(
                Update,
                sync_gateway_size.run_if(
                    resource_exists::<TmuxPresence>
                        .and(resource_exists::<TerminalCellMetricsResource>),
                ),
            );
    }
}

/// The `(gateway, cols, rows)` last sent to the gateway PTY, deduping
/// [`sync_gateway_size`] so a `TerminalResize` is emitted only when the gateway
/// entity or the derived size actually changes.
#[derive(Resource, Default)]
struct GatewaySize(Option<(Entity, u16, u16)>);

/// Sizes the adopted gateway PTY to the full GUI window in cells.
///
/// tmux lays panes out to the control client's tty size — for the adopted
/// connection that is the gateway PTY — so the gateway must track the window.
/// Runs while connected (`TmuxPresence`) and covers both the adopt edge (the
/// gateway entity changes, forcing the first emit) and live window resizes (the
/// derived cell size changes). The full-window size (no status-bar row reserved)
/// matches the gateway birth-size policy: `sync_client_size` then pins the
/// active window one row smaller, so reconciliation is always a shrink.
///
/// Emits [`TerminalResize`] only when the gateway or size changed, so it never
/// spams the PTY each frame.
fn sync_gateway_size(
    mut commands: Commands,
    mut last: ResMut<GatewaySize>,
    connection: NonSend<TmuxConnection>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(gateway) = connection.gateway() else {
        return;
    };
    let Ok(window) = window.single() else {
        return;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let (cols, rows) = cells_for(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
        cell_w,
        cell_h,
    );
    if last.0 == Some((gateway, cols, rows)) {
        return;
    }
    commands.trigger(TerminalResize {
        entity: gateway,
        cols,
        rows,
    });
    last.0 = Some((gateway, cols, rows));
}

/// Adopts a detected control-mode handshake and enters `AppMode::Tmux`.
///
/// Promotes the detected gateway terminal out of the Default view subtree
/// (reparented to `UiRoot`, hidden, and stripped of `KeyboardFocused`) and
/// despawns the now-empty `DefaultModeUi` container so `ensure_default_mode_ui`
/// re-spawns a fresh Default shell on the next return to `AppMode::Default`.
/// On-attach enumeration is NOT kicked here: the gateway's first protocol bytes
/// flip `ConnectionState` to `Attached`, and the crate's own attach-edge
/// detection emits the enumeration trigger.
fn on_control_mode_detected(
    ev: On<ControlModeDetected>,
    mut commands: Commands,
    mut connection: NonSendMut<TmuxConnection>,
    mut next_mode: ResMut<NextState<AppMode>>,
    ui_root: Query<Entity, With<UiRoot>>,
    containers: Query<Entity, With<DefaultModeUi>>,
) {
    let gateway = ev.entity;
    // NOTE: replace any already-live connection rather than overwriting it
    // blindly: a second handshake (e.g. running `tmux -CC` again after a
    // view-hide toggle left the connection live) must not orphan the previous
    // gateway's PTY/child or leave its stale window/pane projection on screen.
    // Despawn the old gateway and reset the projection/connection state before
    // adopting the new.
    if let Some(old) = connection.gateway() {
        if old != gateway {
            commands.entity(old).despawn();
        }
        commands.trigger(TmuxConnectionReset);
        // NOTE: re-inserting an already-present `TmuxPresence` below does NOT fire
        // `Added`, so `resource_added::<TmuxPresence>` consumers — notably
        // `refresh_ozma_sock`, which re-propagates `$OZMA_SOCK` to the newly
        // adopted tmux server — would silently skip this re-adoption. Remove it
        // first so the insert re-arms them for the new gateway.
        commands.remove_resource::<TmuxPresence>();
    }
    connection.adopt(gateway);
    commands.entity(gateway).insert(EnumerationState::default());

    // NOTE: reparent the gateway out of the Default view BEFORE despawning the
    // container. Inserting a new `ChildOf` breaks the old relationship (removing
    // the gateway from the container's `Children`), so the recursive container
    // despawn below cannot take the gateway with it. The gateway is pure
    // transport from now on, so it is hidden and never rendered as a pane.
    {
        let mut gateway_cmds = commands.entity(gateway);
        gateway_cmds.remove::<KeyboardFocused>().insert(Node {
            display: Display::None,
            ..default()
        });
        match ui_root.single() {
            Ok(ui_root) => {
                gateway_cmds.insert(ChildOf(ui_root));
            }
            Err(_) => {
                gateway_cmds.remove::<ChildOf>();
            }
        }
    }

    for container in containers.iter() {
        commands.entity(container).despawn();
    }

    commands.insert_resource(TmuxPresence);
    next_mode.set(AppMode::Tmux);
}

/// Tears down the adopted connection when the gateway's child process exits.
///
/// Covers the case where the shell hosting `tmux -CC` itself dies (the shell
/// closed, or the tmux server was killed) rather than a clean `%exit` detach.
fn on_gateway_child_exit(
    ev: On<TerminalChildExit>,
    mut commands: Commands,
    mut connection: NonSendMut<TmuxConnection>,
) {
    if connection.gateway() == Some(ev.entity) {
        teardown(&mut commands, &mut connection);
    }
}

/// Tears down the adopted connection when tmux emits `%exit` (a detach).
///
/// Gated on `TmuxPresence` and ordered after the drive chain so the batch holds
/// this frame's freshly-drained transport events. NOTE: on a detach the gateway
/// shell process SURVIVES, so `TerminalChildExit` never fires for it — this
/// `%exit` scan is the only teardown signal in that path.
fn teardown_on_exit_notification(
    mut commands: Commands,
    mut connection: NonSendMut<TmuxConnection>,
    batch: Res<TmuxEventBatch>,
) {
    if batch_has_exit(batch.events()) {
        teardown(&mut commands, &mut connection);
    }
}

/// Returns whether `events` contains a tmux `%exit` notification.
fn batch_has_exit(events: &[TransportEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Exit { .. }))
        )
    })
}

/// Closes the connection, despawns the gateway, clears the projection, removes
/// [`TmuxPresence`], and triggers [`TmuxConnectionClosed`] (which returns the app
/// to `AppMode::Default`).
///
/// Idempotent: a no-op once the connection is already closed, so the `%exit`
/// scan and the child-exit observer cannot double-tear-down. Despawning the
/// gateway ends its PTY (its `Drop` kills the child); the fresh Default shell
/// appears via `ensure_default_mode_ui` on the return to `AppMode::Default`.
fn teardown(commands: &mut Commands, connection: &mut TmuxConnection) {
    if !connection.is_connected() {
        return;
    }
    if let Some(gateway) = connection.close() {
        commands.entity(gateway).despawn();
    }
    commands.remove_resource::<TmuxPresence>();
    commands.trigger(TmuxConnectionReset);
    commands.trigger(TmuxConnectionClosed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;
    use ozma_tty_engine::AdoptedControlMode;
    use tmux_control_parser::WindowId;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Default);
        app.insert_non_send_resource(TmuxConnection::default());
        app.init_resource::<TmuxEventBatch>();
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(AdoptPlugin);
        app
    }

    fn spawn_gateway_under_container(app: &mut App) -> (Entity, Entity) {
        let ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<UiRoot>>()
            .single(app.world())
            .expect("UiRoot");
        let container = app
            .world_mut()
            .spawn((Node::default(), DefaultModeUi, ChildOf(ui_root)))
            .id();
        let gateway = app
            .world_mut()
            .spawn((
                AdoptedControlMode::default(),
                KeyboardFocused,
                ChildOf(container),
            ))
            .id();
        (container, gateway)
    }

    #[test]
    fn detected_handshake_adopts_and_enters_tmux() {
        let mut app = build_app();
        let (container, gateway) = spawn_gateway_under_container(&mut app);

        app.world_mut()
            .trigger(ControlModeDetected { entity: gateway });
        app.update();

        assert_eq!(
            app.world().non_send_resource::<TmuxConnection>().gateway(),
            Some(gateway),
            "connection adopted the gateway"
        );
        assert!(
            app.world().get_resource::<TmuxPresence>().is_some(),
            "TmuxPresence inserted"
        );
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Tmux,
            "entered AppMode::Tmux"
        );
        assert!(
            app.world().get_entity(container).is_err(),
            "DefaultModeUi container despawned"
        );
        assert!(
            app.world().get_entity(gateway).is_ok(),
            "gateway survived the container despawn (reparented first)"
        );
        let ui_root = app
            .world_mut()
            .query_filtered::<Entity, With<UiRoot>>()
            .single(app.world())
            .expect("UiRoot");
        let gateway_ref = app.world().entity(gateway);
        assert!(
            gateway_ref.get::<KeyboardFocused>().is_none(),
            "KeyboardFocused stripped from the gateway"
        );
        assert_eq!(
            gateway_ref.get::<Node>().map(|n| n.display),
            Some(Display::None),
            "gateway hidden (pure transport)"
        );
        assert_eq!(
            gateway_ref.get::<ChildOf>().map(|c| c.parent()),
            Some(ui_root),
            "gateway reparented to UiRoot"
        );
    }

    #[test]
    fn gateway_exit_tears_down_connection() {
        let mut app = build_app();
        let gateway = app.world_mut().spawn(AdoptedControlMode::default()).id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);
        app.insert_resource(TmuxPresence);

        app.world_mut().trigger(TerminalChildExit {
            entity: gateway,
            code: Some(0),
        });
        app.update();

        assert!(
            !app.world()
                .non_send_resource::<TmuxConnection>()
                .is_connected(),
            "connection torn down on gateway child exit"
        );
        assert!(
            app.world().get_entity(gateway).is_err(),
            "gateway despawned"
        );
        assert!(
            app.world().get_resource::<TmuxPresence>().is_none(),
            "TmuxPresence removed"
        );
    }

    #[test]
    fn non_gateway_exit_does_not_tear_down() {
        let mut app = build_app();
        let gateway = app.world_mut().spawn(AdoptedControlMode::default()).id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);
        app.insert_resource(TmuxPresence);
        let other = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(TerminalChildExit {
            entity: other,
            code: Some(0),
        });
        app.update();

        assert!(
            app.world()
                .non_send_resource::<TmuxConnection>()
                .is_connected(),
            "an unrelated terminal's exit must not tear down the connection"
        );
    }

    #[test]
    fn batch_has_exit_detects_percent_exit() {
        let exit = TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Exit {
            reason: None,
        }));
        assert!(batch_has_exit(std::slice::from_ref(&exit)));

        let non_exit =
            TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::WindowAdd {
                window: WindowId(1),
            }));
        assert!(!batch_has_exit(std::slice::from_ref(&non_exit)));
        assert!(!batch_has_exit(&[]));
    }

    #[test]
    fn exit_notification_runs_teardown_system() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = build_app();
        let gateway = app.world_mut().spawn(AdoptedControlMode::default()).id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);
        app.insert_resource(TmuxPresence);

        app.world_mut()
            .run_system_once(
                move |mut commands: Commands, mut connection: NonSendMut<TmuxConnection>| {
                    let events = [TransportEvent::Protocol(ClientEvent::Notification(
                        ControlEvent::Exit { reason: None },
                    ))];
                    if batch_has_exit(&events) {
                        teardown(&mut commands, &mut connection);
                    }
                },
            )
            .unwrap();

        assert!(
            !app.world()
                .non_send_resource::<TmuxConnection>()
                .is_connected(),
            "connection torn down on %exit"
        );
        assert!(
            app.world().get_entity(gateway).is_err(),
            "gateway despawned on %exit"
        );
        assert!(
            app.world().get_resource::<TmuxPresence>().is_none(),
            "TmuxPresence removed on %exit"
        );
    }

    #[test]
    fn teardown_is_idempotent_when_already_closed() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = build_app();
        app.world_mut()
            .run_system_once(
                |mut commands: Commands, mut connection: NonSendMut<TmuxConnection>| {
                    teardown(&mut commands, &mut connection);
                },
            )
            .unwrap();
        assert!(
            !app.world()
                .non_send_resource::<TmuxConnection>()
                .is_connected(),
            "teardown on an absent connection is a no-op"
        );
    }

    #[test]
    fn re_adoption_after_teardown_re_enters_tmux() {
        let mut app = build_app();
        // Mimic OzmuxTmuxPlugin's connection-closed -> Default handler, which
        // lives in src/tmux.rs (not AdoptPlugin) so it isn't in build_app.
        app.add_observer(
            |_: On<TmuxConnectionClosed>, mut next: ResMut<NextState<AppMode>>| {
                next.set(AppMode::Default);
            },
        );

        // Deferred observer-command chains + state transitions need a few frames
        // to settle (the real app gets them over multiple frames).
        let pump = |app: &mut App| {
            for _ in 0..5 {
                app.update();
            }
        };

        // First adoption.
        let (_c1, g1) = spawn_gateway_under_container(&mut app);
        app.world_mut().trigger(ControlModeDetected { entity: g1 });
        pump(&mut app);
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Tmux,
            "first tmux -CC enters Tmux"
        );

        // Detach: the gateway child exits -> teardown -> back to Default.
        app.world_mut().trigger(TerminalChildExit {
            entity: g1,
            code: Some(0),
        });
        pump(&mut app);
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Default,
            "detach returns to Default"
        );
        assert!(
            !app.world()
                .non_send_resource::<TmuxConnection>()
                .is_connected(),
            "connection closed after detach"
        );

        // Second adoption: a fresh shell runs tmux -CC again.
        let (_c2, g2) = spawn_gateway_under_container(&mut app);
        app.world_mut().trigger(ControlModeDetected { entity: g2 });
        pump(&mut app);
        assert_eq!(
            app.world().non_send_resource::<TmuxConnection>().gateway(),
            Some(g2),
            "second adoption re-adopts the new gateway"
        );
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Tmux,
            "second tmux -CC must re-enter Tmux mode"
        );
    }

    #[test]
    fn re_adopt_while_live_replaces_and_despawns_old_gateway() {
        let mut app = build_app();
        let (_c1, g1) = spawn_gateway_under_container(&mut app);
        app.world_mut().trigger(ControlModeDetected { entity: g1 });
        app.update();
        assert_eq!(
            app.world().non_send_resource::<TmuxConnection>().gateway(),
            Some(g1),
            "first gateway adopted",
        );

        // A second handshake while the first connection is still live (e.g.
        // running `tmux -CC` in the fresh shell of a view-hidden session) must
        // replace it and despawn the old gateway, not orphan its PTY/child.
        let (_c2, g2) = spawn_gateway_under_container(&mut app);
        app.world_mut().trigger(ControlModeDetected { entity: g2 });
        app.update();

        assert_eq!(
            app.world().non_send_resource::<TmuxConnection>().gateway(),
            Some(g2),
            "the new gateway replaces the old",
        );
        assert!(
            app.world().get_entity(g1).is_err(),
            "the previous gateway must be despawned, not leaked",
        );
    }

    #[derive(Resource, Default)]
    struct ResizeLog(Vec<(Entity, u16, u16)>);

    fn install_metrics_and_window(app: &mut App, phys_w: u32, phys_h: u32) {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};
        use ozma_tty_renderer::CellMetrics;

        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 13.0,
                descent_phys: 3.0,
                underline_position_phys: -1.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        });
        let mut window = Window {
            resolution: WindowResolution::new(phys_w, phys_h),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.init_resource::<ResizeLog>();
        app.add_observer(|ev: On<TerminalResize>, mut log: ResMut<ResizeLog>| {
            log.0.push((ev.entity, ev.cols, ev.rows));
        });
    }

    fn adopt_gateway(app: &mut App) -> Entity {
        let gateway = app.world_mut().spawn(AdoptedControlMode::default()).id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);
        app.insert_resource(TmuxPresence);
        gateway
    }

    #[test]
    fn gateway_sized_to_full_window_on_adopt() {
        let mut app = build_app();
        install_metrics_and_window(&mut app, 800, 600);
        let gateway = adopt_gateway(&mut app);

        app.update();

        let log = &app.world().resource::<ResizeLog>().0;
        assert_eq!(
            log.as_slice(),
            // 800/8 = 100 cols, 600/16 = 37 rows — full window, no bar row reserved.
            &[(gateway, 100, 37)],
            "adopt emits one full-window TerminalResize for the gateway",
        );
    }

    #[test]
    fn gateway_size_deduped_when_unchanged() {
        let mut app = build_app();
        install_metrics_and_window(&mut app, 800, 600);
        adopt_gateway(&mut app);

        app.update();
        app.update();
        app.update();

        assert_eq!(
            app.world().resource::<ResizeLog>().0.len(),
            1,
            "an unchanged window/gateway must not re-emit TerminalResize each frame",
        );
    }

    #[test]
    fn gateway_resized_when_window_changes() {
        use bevy::window::{PrimaryWindow, Window};

        let mut app = build_app();
        install_metrics_and_window(&mut app, 800, 600);
        let gateway = adopt_gateway(&mut app);

        app.update();

        let mut window = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>()
            .single_mut(app.world_mut())
            .expect("primary window");
        window.resolution.set_physical_resolution(1600, 600);
        app.update();

        let log = &app.world().resource::<ResizeLog>().0;
        assert_eq!(
            log.as_slice(),
            // 800/8=100 then 1600/8=200 cols; rows unchanged at 37.
            &[(gateway, 100, 37), (gateway, 200, 37)],
            "a window resize re-emits TerminalResize with the new size",
        );
    }

    #[test]
    fn gateway_size_skips_without_presence() {
        let mut app = build_app();
        install_metrics_and_window(&mut app, 800, 600);
        let gateway = app.world_mut().spawn(AdoptedControlMode::default()).id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);

        app.update();

        assert!(
            app.world().resource::<ResizeLog>().0.is_empty(),
            "no TmuxPresence (run_if gate) means no gateway resize",
        );
    }
}
