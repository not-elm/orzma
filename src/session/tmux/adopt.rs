//! Adoption lifecycle: bridges a detected `tmux -CC` handshake into a live
//! connection and tears it down again.
//!
//! When a Default-mode shell runs `tmux -CC`, the engine fires
//! [`ControlModeDetected`] on that terminal. [`on_control_mode_detected`] adopts
//! the terminal's PTY as the control-mode gateway by inserting a [`TmuxClient`]
//! component on it, promotes it out of the Default view (despawning the
//! now-empty Default container so a fresh shell is lazily re-spawned on return),
//! and enters [`AppMode::Tmux`]. The drive chain activates on the presence of a
//! [`TmuxClient`] component. Teardown has two paths: `%exit` (a detach, where
//! the gateway shell process survives) restores the gateway as the Default
//! shell via [`ReleaseControlMode`]; the gateway's child process actually
//! exiting despawns it instead. Both return to [`AppMode::Default`].

use crate::app_mode::AppMode;
use crate::input::focus::KeyboardFocused;
use crate::surface::geometry::cells_for;
use crate::ui::UiRoot;
use crate::ui::default_mode::{DefaultModeUi, restore_default_shell};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ozma_tty_engine::{ControlModeDetected, ReleaseControlMode, TerminalChildExit, TerminalResize};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozmux_tmux::{
    ClientEvent, ControlEvent, TmuxClient, TmuxConnectionClosed, TmuxConnectionReset,
    TmuxEventBatch, TmuxProjectionSet, TransportEvent,
};

/// Registers the adoption observer and the teardown systems/observer.
pub(super) struct AdoptPlugin;

/// Ordering label for [`teardown_on_exit_notification`]'s `AdoptPlugin`
/// registration, so [`sync_gateway_size`] can order against it by set rather
/// than by system-function identity (tests register a second, gate-free
/// instance of the bare system, which would make ordering-by-function
/// ambiguous).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct TeardownOnExitSet;

impl Plugin for AdoptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GatewaySize>()
            .add_observer(on_control_mode_detected)
            .add_observer(on_gateway_child_exit)
            .add_systems(
                Update,
                teardown_on_exit_notification
                    .in_set(TeardownOnExitSet)
                    .after(TmuxProjectionSet)
                    .run_if(any_with_component::<TmuxClient>),
            )
            .add_systems(
                Update,
                sync_gateway_size.before(TeardownOnExitSet).run_if(
                    any_with_component::<TmuxClient>
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
/// Runs while connected (a [`TmuxClient`] exists) and covers both the adopt edge
/// (the gateway entity changes, forcing the first emit) and live window resizes
/// (the derived cell size changes). The full-window size (no status-bar row
/// reserved) matches the gateway birth-size policy: `sync_client_size` then pins
/// the active window one row smaller, so reconciliation is always a shrink.
///
/// Emits [`TerminalResize`] only when the gateway or size changed, so it never
/// spams the PTY each frame.
///
// NOTE: must run before `teardown_on_exit_notification` in the same frame.
// The gateway's `TmuxClient` removal (via `ReleaseControlMode`'s deferred
// observers) doesn't take effect until end-of-frame, so this system's
// `any_with_component::<TmuxClient>` gate still passes right after a detach
// fires — if this ran after the detach path's `GatewaySize` reset, it would
// immediately repopulate `last.0`, defeating the reset and wrongly skipping
// the resize on a later re-adoption of the same (surviving) gateway entity.
fn sync_gateway_size(
    mut commands: Commands,
    mut last: ResMut<GatewaySize>,
    gateway: Single<Entity, With<TmuxClient>>,
    metrics: Res<TerminalCellMetricsResource>,
    window: Query<&Window, With<PrimaryWindow>>,
) {
    let gateway = *gateway;
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
/// insert `TmuxAttached` on the gateway entity, and the crate's own attach-edge
/// detection emits the enumeration trigger.
fn on_control_mode_detected(
    ev: On<ControlModeDetected>,
    mut commands: Commands,
    mut next_mode: ResMut<NextState<AppMode>>,
    existing: Query<Entity, With<TmuxClient>>,
    ui_root: Query<Entity, With<UiRoot>>,
    containers: Query<Entity, With<DefaultModeUi>>,
) {
    let gateway = ev.entity;
    // NOTE: replace any already-live connection rather than overwriting it
    // blindly: a second handshake (e.g. running `tmux -CC` again after a
    // view-hide toggle left the connection live) must not orphan the previous
    // gateway's PTY/child or leave its stale window/pane projection on screen.
    if let Ok(old) = existing.single()
        && old != gateway
    {
        commands.entity(old).despawn();
        commands.trigger(TmuxConnectionReset);
    }
    commands.entity(gateway).insert(TmuxClient::new_adopted());

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

    next_mode.set(AppMode::Tmux);
}

/// Tears down the adopted connection when the gateway's child process exits.
///
/// Covers the case where the shell hosting `tmux -CC` itself dies (the shell
/// closed, or the tmux server was killed) rather than a clean `%exit` detach.
fn on_gateway_child_exit(
    ev: On<TerminalChildExit>,
    mut commands: Commands,
    clients: Query<(), With<TmuxClient>>,
) {
    if clients.get(ev.entity).is_ok() {
        teardown_despawn(&mut commands, ev.entity);
    }
}

/// Returns the `%exit` notification's reason if `events` contains one
/// (`Some(None)` for a bare `%exit` with no reason text).
fn batch_exit_reason(events: &[TransportEvent]) -> Option<Option<String>> {
    events.iter().find_map(|event| match event {
        TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Exit { reason })) => {
            Some(reason.clone())
        }
        _ => None,
    })
}

/// Renders the iTerm2-style detach line fed into the restored shell's VT.
///
/// tmux never writes `[detached …]` to the PTY in control mode (that message
/// is the non-control-mode branch of its client), so it is fabricated here
/// from the `%exit` reason.
fn synthesized_detach_line(reason: Option<String>) -> String {
    format!("[{}]\r\n", reason.as_deref().unwrap_or("detached"))
}

/// Restores the adopted connection's gateway to the Default shell when tmux
/// emits `%exit` (a detach — the gateway shell process survives).
///
/// Gated on the presence of a [`TmuxClient`] and ordered after the drive chain
/// so the batch holds this frame's freshly-drained transport events. NOTE: on a
/// detach the gateway shell process SURVIVES, so `TerminalChildExit` never fires
/// for it — this `%exit` scan is the only teardown signal in that path.
fn teardown_on_exit_notification(
    mut commands: Commands,
    mut last: ResMut<GatewaySize>,
    mut clients: Query<(Entity, &mut TmuxClient)>,
    ui_root: Query<Entity, With<UiRoot>>,
    batch: Res<TmuxEventBatch>,
) {
    let Some(reason) = batch_exit_reason(batch.events()) else {
        return;
    };
    let Ok((gateway, mut client)) = clients.single_mut() else {
        return;
    };
    restore_gateway(
        &mut commands,
        &mut last,
        &mut client,
        gateway,
        ui_root.single().ok(),
        reason,
    );
}

/// Restores `gateway` to the Default shell: releases control mode (feeding the
/// synthesized detach line + post-exit residual back into the VT and re-arming
/// the introducer watch), strips the connection components (via the
/// tmux-session `ReleaseControlMode` observer), rebuilds the Default view, and
/// closes the connection.
fn restore_gateway(
    commands: &mut Commands,
    last: &mut GatewaySize,
    client: &mut TmuxClient,
    gateway: Entity,
    ui_root: Option<Entity>,
    reason: Option<String>,
) {
    let mut residual = synthesized_detach_line(reason).into_bytes();
    residual.extend_from_slice(&client.take_residual());
    commands.trigger(ReleaseControlMode {
        entity: gateway,
        residual,
    });
    if let Some(ui_root) = ui_root {
        restore_default_shell(commands, gateway, ui_root);
    }
    last.0 = None;
    commands.trigger(TmuxConnectionReset);
    commands.trigger(TmuxConnectionClosed);
}

/// Tears the connection down by despawning the gateway (the death path: the
/// gateway's child process exited, so there is no shell left to restore).
/// Despawning ends its PTY (its `Drop` kills the child); the fresh Default
/// shell appears via `ensure_default_mode_ui` on the return to
/// `AppMode::Default`.
///
/// Idempotency is guaranteed by the callers' `With<TmuxClient>` checks: once
/// the gateway is despawned (or released) neither teardown path finds a
/// `TmuxClient`, so neither fires again.
fn teardown_despawn(commands: &mut Commands, gateway: Entity) {
    commands.entity(gateway).despawn();
    commands.trigger(TmuxConnectionReset);
    commands.trigger(TmuxConnectionClosed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;
    use ozma_tty_engine::AdoptedControlMode;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.insert_state(AppMode::Default);
        app.init_resource::<TmuxEventBatch>();
        app.world_mut().spawn((Node::default(), UiRoot));
        app.add_plugins(AdoptPlugin);
        app
    }

    fn client_gateway(app: &mut App) -> Option<Entity> {
        app.world_mut()
            .query_filtered::<Entity, With<TmuxClient>>()
            .single(app.world())
            .ok()
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

        assert!(
            app.world().get::<TmuxClient>(gateway).is_some(),
            "connection adopted the gateway (TmuxClient inserted)"
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
        let gateway = app
            .world_mut()
            .spawn((AdoptedControlMode::default(), TmuxClient::new_adopted()))
            .id();

        app.world_mut().trigger(TerminalChildExit {
            entity: gateway,
            code: Some(0),
        });
        app.update();

        assert!(
            client_gateway(&mut app).is_none(),
            "connection torn down on gateway child exit"
        );
        assert!(
            app.world().get_entity(gateway).is_err(),
            "gateway despawned"
        );
    }

    #[test]
    fn non_gateway_exit_does_not_tear_down() {
        let mut app = build_app();
        let gateway = app
            .world_mut()
            .spawn((AdoptedControlMode::default(), TmuxClient::new_adopted()))
            .id();
        let other = app.world_mut().spawn_empty().id();

        app.world_mut().trigger(TerminalChildExit {
            entity: other,
            code: Some(0),
        });
        app.update();

        assert_eq!(
            client_gateway(&mut app),
            Some(gateway),
            "an unrelated terminal's exit must not tear down the connection"
        );
    }

    #[test]
    fn synthesized_detach_line_formats_reason() {
        assert_eq!(
            synthesized_detach_line(Some("detached (from session main)".into())),
            "[detached (from session main)]\r\n"
        );
        assert_eq!(synthesized_detach_line(None), "[detached]\r\n");
    }

    #[test]
    fn batch_exit_reason_extracts_the_reason() {
        let exit = TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Exit {
            reason: Some("detached (from session main)".into()),
        }));
        assert_eq!(
            batch_exit_reason(std::slice::from_ref(&exit)),
            Some(Some("detached (from session main)".into()))
        );
        let bare = TransportEvent::Protocol(ClientEvent::Notification(ControlEvent::Exit {
            reason: None,
        }));
        assert_eq!(batch_exit_reason(std::slice::from_ref(&bare)), Some(None));
        assert_eq!(batch_exit_reason(&[]), None);
    }

    #[test]
    fn exit_notification_restores_gateway_into_default_container() {
        use bevy::ecs::system::RunSystemOnce;
        use ozma_tty_engine::ReleaseControlMode;

        let mut app = build_app();
        app.add_observer(|ev: On<ReleaseControlMode>, mut commands: Commands| {
            commands.entity(ev.entity).remove::<TmuxClient>();
        });
        app.add_observer(
            |_: On<TmuxConnectionClosed>, mut next: ResMut<NextState<AppMode>>| {
                next.set(AppMode::Default);
            },
        );

        let (container, gateway) = spawn_gateway_under_container(&mut app);
        app.world_mut()
            .trigger(ControlModeDetected { entity: gateway });
        for _ in 0..3 {
            app.update();
        }
        assert!(
            app.world().get_entity(container).is_err(),
            "adoption despawned the original container"
        );
        app.world_mut().resource_mut::<GatewaySize>().0 = Some((gateway, 100, 37));

        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      mut last: ResMut<GatewaySize>,
                      mut clients: Query<&mut TmuxClient>,
                      ui_root: Query<Entity, With<UiRoot>>| {
                    let mut client = clients.get_mut(gateway).expect("gateway has a client");
                    restore_gateway(
                        &mut commands,
                        &mut last,
                        &mut client,
                        gateway,
                        ui_root.single().ok(),
                        Some("detached (from session main)".into()),
                    );
                },
            )
            .unwrap();
        for _ in 0..3 {
            app.update();
        }

        assert!(
            app.world().get_entity(gateway).is_ok(),
            "detach must NOT despawn the gateway"
        );
        assert!(
            app.world().get::<TmuxClient>(gateway).is_none(),
            "connection component stripped via ReleaseControlMode"
        );
        assert!(
            app.world().get::<KeyboardFocused>(gateway).is_some(),
            "keyboard focus restored"
        );
        assert_eq!(
            app.world().resource::<GatewaySize>().0,
            None,
            "GatewaySize reset so a re-adoption at the same size re-emits"
        );
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Default,
            "detach returns to Default mode"
        );
        let world = app.world_mut();
        let containers: Vec<Entity> = world
            .query_filtered::<Entity, With<DefaultModeUi>>()
            .iter(world)
            .collect();
        assert_eq!(containers.len(), 1, "exactly one fresh DefaultModeUi");
        let gateway_ref = world.entity(gateway);
        assert_eq!(
            gateway_ref.get::<ChildOf>().map(|c| c.parent()),
            Some(containers[0]),
            "gateway reparented under the fresh container"
        );
        assert_ne!(
            gateway_ref.get::<Node>().map(|n| n.display),
            Some(Display::None),
            "gateway visible again"
        );
    }

    #[test]
    fn readoption_after_restore_reenters_tmux_on_the_same_entity() {
        use ozma_tty_engine::ReleaseControlMode;

        let mut app = build_app();
        app.add_observer(|ev: On<ReleaseControlMode>, mut commands: Commands| {
            commands.entity(ev.entity).remove::<TmuxClient>();
        });
        app.add_observer(
            |_: On<TmuxConnectionClosed>, mut next: ResMut<NextState<AppMode>>| {
                next.set(AppMode::Default);
            },
        );
        let pump = |app: &mut App| {
            for _ in 0..5 {
                app.update();
            }
        };

        let (_c1, gateway) = spawn_gateway_under_container(&mut app);
        app.world_mut()
            .trigger(ControlModeDetected { entity: gateway });
        pump(&mut app);
        {
            use bevy::ecs::system::RunSystemOnce;
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          mut last: ResMut<GatewaySize>,
                          mut clients: Query<&mut TmuxClient>,
                          ui_root: Query<Entity, With<UiRoot>>| {
                        let mut client = clients.get_mut(gateway).expect("client");
                        restore_gateway(
                            &mut commands,
                            &mut last,
                            &mut client,
                            gateway,
                            ui_root.single().ok(),
                            None,
                        );
                    },
                )
                .unwrap();
        }
        pump(&mut app);
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Default
        );

        app.world_mut()
            .trigger(ControlModeDetected { entity: gateway });
        pump(&mut app);
        assert_eq!(
            *app.world().resource::<State<AppMode>>().get(),
            AppMode::Tmux,
            "re-running tmux -CC from the restored shell re-enters Tmux"
        );
        assert!(
            app.world().get::<TmuxClient>(gateway).is_some(),
            "the same entity is the gateway again"
        );
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<DefaultModeUi>>()
                .iter(world)
                .count(),
            0,
            "re-adoption despawned the restored container again"
        );
    }

    #[test]
    fn teardown_is_a_noop_without_a_client() {
        let mut app = build_app();
        app.add_systems(Update, teardown_on_exit_notification);
        // No TmuxClient spawned: the run_if (any_with_component) is bypassed here
        // because teardown_on_exit_notification is registered bare in this test;
        // its own clients.single() guard keeps it a no-op.
        app.update();
        assert!(
            client_gateway(&mut app).is_none(),
            "teardown with no client present is a no-op"
        );
    }

    #[test]
    fn re_adoption_after_teardown_re_enters_tmux() {
        let mut app = build_app();
        // NOTE: mimic on_tmux_connection_closed, which lives in src/session/tmux.rs
        // (registered by TmuxLifecyclePlugin, not AdoptPlugin) so it isn't in build_app.
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
            client_gateway(&mut app).is_none(),
            "connection closed after detach"
        );

        // Second adoption: a fresh shell runs tmux -CC again.
        let (_c2, g2) = spawn_gateway_under_container(&mut app);
        app.world_mut().trigger(ControlModeDetected { entity: g2 });
        pump(&mut app);
        assert_eq!(
            client_gateway(&mut app),
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
        assert_eq!(client_gateway(&mut app), Some(g1), "first gateway adopted",);

        // A second handshake while the first connection is still live (e.g.
        // running `tmux -CC` in the fresh shell of a view-hidden session) must
        // replace it and despawn the old gateway, not orphan its PTY/child.
        let (_c2, g2) = spawn_gateway_under_container(&mut app);
        app.world_mut().trigger(ControlModeDetected { entity: g2 });
        app.update();

        assert_eq!(
            client_gateway(&mut app),
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
        app.world_mut()
            .spawn((AdoptedControlMode::default(), TmuxClient::new_adopted()))
            .id()
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
    fn gateway_size_skips_without_client() {
        let mut app = build_app();
        install_metrics_and_window(&mut app, 800, 600);
        // A gateway with no TmuxClient: the any_with_component gate blocks the
        // system entirely.
        app.world_mut().spawn(AdoptedControlMode::default());

        app.update();

        assert!(
            app.world().resource::<ResizeLog>().0.is_empty(),
            "no TmuxClient (run_if gate) means no gateway resize",
        );
    }
}
