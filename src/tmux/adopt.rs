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
use ozma_terminal::KeyboardFocused;
use ozma_tty_engine::{ControlModeDetected, TerminalChildExit};
use ozmux_tmux::{
    ClientEvent, ControlEvent, TmuxConnection, TmuxConnectionClosed, TmuxConnectionReset,
    TmuxEventBatch, TmuxPresence, TmuxProjectionSet, TransportEvent,
};

/// Registers the adoption observer and the teardown systems/observer.
pub(crate) struct AdoptPlugin;

impl Plugin for AdoptPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_control_mode_detected)
            .add_observer(on_gateway_child_exit)
            .add_systems(
                Update,
                teardown_on_exit_notification
                    .after(TmuxProjectionSet)
                    .run_if(resource_exists::<TmuxPresence>),
            );
    }
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
    // The pending CommandId for the adopted stream's entry reply is intentionally
    // dropped: it correlates at the ProtocolClient level and is harmless if it
    // never matches an EnumerationState entry.
    connection.adopt(gateway);

    // Reparent the gateway out of the Default view BEFORE despawning the
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
}
