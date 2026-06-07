//! In-process thin-client multiplexer: boots an `ozmuxd` Server on a temp UDS,
//! connects a proto `Client`, and builds the ECS tree from the Welcome snapshot.
//! The pump + render + viewport systems are added in later tasks.

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy_terminal_renderer::prelude::{TerminalDelta, TerminalGrid, TerminalSnapshot};
use ozmux_multiplexer::{
    AttachedWorkspace, MirrorReadCtx, MuxState, SessionSnapshot, WorkspaceCreatedAt, apply_events,
    build_from_snapshot,
};
use ozmux_proto::{Client, Frame, MuxEvent, ServerMessage, SurfaceId, VtEvent};
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};

/// Holds the in-process daemon alive for the app's lifetime (Drop tears it down).
#[derive(Resource)]
pub(crate) struct ThinDaemon(
    // NOTE: held only for its Drop (RAII teardown of the in-process daemon); the
    // field is never read, so the dead-code lint must be silenced explicitly.
    #[expect(dead_code, reason = "RAII guard: kept alive for Drop, never read")]
    pub(crate)  ozmuxd::ServerHandle,
);

/// The wire client. NonSend because `Client` holds a `Box<dyn FnOnce()+Send>`
/// shutdown hook (not `Sync`), and is only touched by the main-thread pump.
pub(crate) struct ThinClientConn(pub(crate) Client<UnixStream>);

/// One-shot focus requests: when a `SpawnSurface` we sent folds back as
/// `SurfaceSpawned { pane }`, focus the new surface. Keyed by pane; first match
/// wins (single in-process client — daemon-initiated spawns aren't distinguished).
#[derive(Resource, Default)]
pub(crate) struct PendingFocus(pub(crate) std::collections::HashSet<ozmux_proto::PaneId>);

/// Sends a command to the in-process daemon, logging (not propagating) send errors.
pub(crate) fn send_cmd(conn: &mut ThinClientConn, msg: ozmux_proto::ClientMessage) {
    if let Err(e) = conn.0.send(msg) {
        error!("thin-client: send {e}");
    }
}

/// Runs the GUI as a read-only thin client over an in-process `ozmuxd`.
pub struct ThinClientMultiplexerPlugin;

impl Plugin for ThinClientMultiplexerPlugin {
    fn build(&self, app: &mut App) {
        let (handle, client, snapshot) =
            boot().expect("thin-client: in-process daemon boot failed");
        let mut state = MuxState::from_snapshot(snapshot.clone());
        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, app.world());
            build_from_snapshot(&mut commands, &mut state, &snapshot);
            stamp_attached_workspace(&mut commands, &state, &snapshot);
        }
        queue.apply(app.world_mut());
        app.insert_resource(state);
        app.init_resource::<PendingFocus>();
        app.insert_resource(ThinDaemon(handle));
        app.insert_non_send_resource(ThinClientConn(client));
        app.add_systems(
            Update,
            pump_thin_client.before(crate::system_set::OzmuxSystems::Input),
        );
        #[cfg(debug_assertions)]
        app.add_systems(Last, debug_assert_ecs_matches_fold);
    }
}

/// Drains all available wire messages each frame: control `Events` fold into the
/// ECS via `apply_events`; `Frame`s become `TerminalSnapshot`/`TerminalDelta`
/// triggers on the surface entity; `SurfaceEvent`s drive title/bell.
fn pump_thin_client(
    mut commands: Commands,
    mut conn: NonSendMut<ThinClientConn>,
    mut state: ResMut<MuxState>,
    mut grids: Query<&mut TerminalGrid>,
    mut pending: ResMut<PendingFocus>,
    read: MirrorReadCtx,
    attached_q: Query<Entity, With<AttachedWorkspace>>,
) {
    let mut budget = 256u32;
    while budget > 0 {
        budget -= 1;
        let msg = match conn.0.try_poll() {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => {
                error!("thin-client: wire poll error: {e}");
                break;
            }
        };
        match msg {
            ServerMessage::Events(batch) => {
                apply_events(&mut commands, &mut state, &read, &batch);
                for ev in &batch {
                    match ev {
                        MuxEvent::SurfaceSpawned { pane, surface, .. }
                            if pending.0.remove(pane) =>
                        {
                            send_cmd(
                                &mut conn,
                                ozmux_proto::ClientMessage::SetActiveSurface {
                                    pane: *pane,
                                    surface: *surface,
                                },
                            );
                        }
                        MuxEvent::WorkspaceSelected { workspace, .. } => {
                            if let Some(ws_ent) = state.workspace_entity(*workspace) {
                                for holder in attached_q.iter() {
                                    commands.entity(holder).remove::<AttachedWorkspace>();
                                }
                                commands.entity(ws_ent).insert(AttachedWorkspace);
                            }
                        }
                        _ => {}
                    }
                }
            }
            ServerMessage::Frame { surface, frame } => {
                if let Some(ent) = state.surface_entity(surface) {
                    match frame {
                        Frame::Snapshot(snapshot) => {
                            commands.trigger(TerminalSnapshot {
                                entity: ent,
                                snapshot,
                            });
                        }
                        Frame::Delta(delta) => {
                            commands.trigger(TerminalDelta { entity: ent, delta });
                        }
                    }
                }
            }
            ServerMessage::SurfaceEvent { surface, event } => {
                handle_surface_event(&mut grids, &state, surface, event);
            }
            ServerMessage::Welcome { .. } => {}
            ServerMessage::Error { message } => error!("thin-client: server error: {message}"),
        }
    }
}

/// Debug-only: assert the ECS tree matches the authoritative fold + no map leaks.
#[cfg(debug_assertions)]
fn debug_assert_ecs_matches_fold(world: &mut World) {
    use ozmux_multiplexer::{assert_no_map_leaks, ecs_matches_fold};
    if let Err(m) = ecs_matches_fold(world) {
        panic!("thin-client mirror drift: {m:?}");
    }
    assert_no_map_leaks(world);
}

/// Read-path VtEvent re-home: folds `ModeChanged` into the surface's
/// `TerminalGrid.modes`. Other variants are no-ops in 4c-1b-1 (cwd arrives via
/// Events::SurfaceCwdChanged; the rest are 4c-1b-2 / 4c-1c).
fn handle_surface_event(
    grids: &mut Query<&mut TerminalGrid>,
    state: &MuxState,
    surface: SurfaceId,
    event: VtEvent,
) {
    let Some(ent) = state.surface_entity(surface) else {
        return;
    };
    match event {
        VtEvent::ModeChanged { added, removed } => {
            if let Ok(mut grid) = grids.get_mut(ent) {
                grid.modes.retain(|m| !removed.contains(m));
                for m in added {
                    if !grid.modes.contains(&m) {
                        grid.modes.push(m);
                    }
                }
            }
        }
        VtEvent::TitleChanged(_title) => {
            // TODO: 4c-1b-2 wire tab-title from page title
        }
        _ => {}
    }
}

/// Boots an in-process `ozmuxd` on a process-unique temp UDS, connects a
/// `Client`, and returns the handle, client, and the Welcome snapshot.
fn boot() -> std::io::Result<(ozmuxd::ServerHandle, Client<UnixStream>, SessionSnapshot)> {
    // NOTE: Socket path must be unique per boot call, not just per process, to
    //       prevent concurrent test instances (even with --test-threads=1, the
    //       static counter makes each plugin instance independently addressable
    //       and avoids any residual socket from a prior run colliding).
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("ozmux-tc-{pid}-{n}.sock"));
    let _ = std::fs::remove_file(&path);
    let handle = ozmuxd::Server::new().serve(&path)?;

    let stream = UnixStream::connect(&path)?;
    let reader = BufReader::new(stream.try_clone()?);
    let shutdown_handle = stream.try_clone()?;
    let viewport = (80u16, 24u16);
    let client = Client::connect_with_shutdown(
        reader,
        stream,
        viewport,
        Some(Box::new(move || {
            let _ = shutdown_handle.shutdown(Shutdown::Both);
        })),
    )?;
    let snapshot = client.mirror().to_snapshot();
    Ok((handle, client, snapshot))
}

/// Stamps the GUI-attach markers on the snapshot's active workspace so the
/// focus/layout systems (`With<AttachedWorkspace>`) work.
fn stamp_attached_workspace(commands: &mut Commands, state: &MuxState, snapshot: &SessionSnapshot) {
    if let Some(ws_id) = snapshot.active_workspace
        && let Some(ws_ent) = state.workspace_entity(ws_id)
    {
        commands
            .entity(ws_ent)
            .insert((AttachedWorkspace, WorkspaceCreatedAt(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a headless `App` with just enough infrastructure for the
    /// `ThinClientMultiplexerPlugin` to boot, build entities, and pump frames.
    fn headless_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.add_plugins(ThinClientMultiplexerPlugin);
        app
    }

    #[test]
    fn thin_client_attaches_workspace_from_welcome() {
        // `build()` already connected + built entities from the Welcome snapshot
        // synchronously, so no `app.update()` is needed before the assertion.
        let mut app = headless_app();
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<AttachedWorkspace>>();
        let n = q.iter(app.world()).count();
        assert_eq!(
            n, 1,
            "exactly one AttachedWorkspace built from the Welcome snapshot"
        );
    }

    #[test]
    fn thin_client_setviewport_mirrors_to_pane_dimensions() {
        let mut app = headless_app();
        // Send SetViewport directly via the client resource (no UI layout needed),
        // then pump frames until the daemon's PaneResized → PaneDimensions lands.
        {
            let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
            conn.0
                .send(ozmux_proto::ClientMessage::SetViewport {
                    cols: 100,
                    rows: 40,
                })
                .expect("send SetViewport");
        }
        let mut found = false;
        for _ in 0..120 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
            let mut q = app
                .world_mut()
                .query::<&ozmux_multiplexer::PaneDimensions>();
            if q.iter(app.world()).any(|d| d.cols == 100 && d.rows == 40) {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "SetViewport → PaneResized Events → PaneDimensions{{100,40}} mirrored"
        );
    }
}
