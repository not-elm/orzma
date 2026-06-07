//! In-process thin-client multiplexer: boots an `ozmuxd` Server on a temp UDS,
//! connects a proto `Client`, and builds the ECS tree from the Welcome snapshot.
//! The pump + render + viewport systems are added in later tasks.

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy_terminal_renderer::prelude::{TerminalDelta, TerminalSnapshot};
use ozmux_multiplexer::{
    AttachedWorkspace, MirrorReadCtx, MuxState, SessionSnapshot, WorkspaceCreatedAt, apply_events,
    build_from_snapshot,
};
use ozmux_proto::{Client, Frame, ServerMessage, SurfaceId, VtEvent};
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

/// Holds the in-process daemon alive for the app's lifetime (Drop tears it down).
#[derive(Resource)]
pub(crate) struct ThinDaemon(pub(crate) ozmuxd::ServerHandle);

/// The wire client. NonSend because `Client` holds a `Box<dyn FnOnce()+Send>`
/// shutdown hook (not `Sync`), and is only touched by the main-thread pump.
pub(crate) struct ThinClientConn(pub(crate) Client<UnixStream>);

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
        app.insert_resource(ThinDaemon(handle));
        app.insert_non_send_resource(ThinClientConn(client));
        app.add_systems(Update, pump_thin_client);
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
    read: MirrorReadCtx,
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
                handle_surface_event(&mut commands, &state, surface, event);
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

/// Read-path VtEvent re-home: title/bell. Other variants are no-ops in 4c-1b-1
/// (cwd arrives via Events::SurfaceCwdChanged; the rest are 4c-1b-2 / 4c-1c).
fn handle_surface_event(
    commands: &mut Commands,
    state: &MuxState,
    surface: SurfaceId,
    event: VtEvent,
) {
    let Some(_ent) = state.surface_entity(surface) else {
        return;
    };
    match event {
        VtEvent::TitleChanged(_title) => {
            // TODO: 4c-1b-2 wire tab-title from page title
        }
        _ => {}
    }
    let _ = commands;
}

/// Boots an in-process `ozmuxd` on a process-unique temp UDS, connects a
/// `Client`, and returns the handle, client, and the Welcome snapshot.
fn boot() -> std::io::Result<(ozmuxd::ServerHandle, Client<UnixStream>, SessionSnapshot)> {
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("ozmux-tc-{pid}.sock"));
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
