//! In-process thin-client multiplexer: boots an `ozmuxd` Server on a temp UDS,
//! connects a proto `Client`, and builds the ECS tree from the Welcome snapshot.
//! The pump + render + viewport systems are added in later tasks.

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_terminal::SelectionType;
use bevy_terminal_renderer::prelude::{TerminalDelta, TerminalGrid, TerminalSnapshot};
use ozmux_multiplexer::{
    AttachedWorkspace, MirrorReadCtx, MuxState, SessionSnapshot, WorkspaceCreatedAt,
    apply_events_checked, build_from_snapshot_checked,
};
use ozmux_proto::{Client, Frame, MuxEvent, ServerMessage, SurfaceId, VtEvent};
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Owns the daemon's lifetime relative to the GUI.
#[derive(Resource)]
pub(crate) enum ThinDaemon {
    /// Tests / dev: in-process daemon. `ServerHandle`'s Drop tears it down +
    /// removes its socket. (RAII — kept alive for the side-effect.)
    InProcess(
        #[expect(dead_code, reason = "RAII guard: kept alive for Drop, never read")]
        ozmuxd::ServerHandle,
    ),
    /// Real app: a detached out-of-process daemon. Nothing is held — it persists
    /// past our exit; the next launch re-attaches.
    OutOfProcess,
}

/// Set when the GUI has requested a clean exit (daemon connection lost, or an
/// inconsistent daemon message in Task 5). The debug mirror-drift assert skips
/// while this is set, because a fail-closed teardown can leave the ECS mid-batch.
#[derive(Resource, Default)]
pub(crate) struct ThinClientExiting(pub(crate) bool);

/// The wire client. NonSend because `Client` holds a `Box<dyn FnOnce()+Send>`
/// shutdown hook (not `Sync`), and is only touched by the main-thread pump.
pub(crate) struct ThinClientConn(pub(crate) Client<UnixStream>);

/// One-shot focus requests: when a `SpawnSurface` we sent folds back as
/// `SurfaceSpawned { pane }`, focus the new surface. Keyed by pane; first match
/// wins (single in-process client — daemon-initiated spawns aren't distinguished).
#[derive(Resource, Default)]
pub(crate) struct PendingFocus(pub(crate) std::collections::HashSet<ozmux_proto::PaneId>);

/// Monotonic creation-order counter for workspaces folded after attach. The boot
/// workspace is stamped `WorkspaceCreatedAt(1)` (see `stamp_attached_workspace`),
/// so post-boot workspaces start at 2. Mirrors the local `WorkspaceNameCounter`
/// ordering the local path stamps via `MultiplexerCommands` (gated out here), so
/// `FocusWorkspace`/status-bar sort by creation order instead of all tying at
/// `u32::MAX`.
#[derive(Resource)]
struct ThinWorkspaceSeq(u32);

/// Sends a command to the in-process daemon, logging (not propagating) send errors.
pub(crate) fn send_cmd(conn: &mut ThinClientConn, msg: ozmux_proto::ClientMessage) {
    if let Err(e) = conn.0.send(msg) {
        error!("thin-client: send {e}");
    }
}

/// Sends a `CopyModeOp` for `surface` to the daemon over the wire.
pub(crate) fn send_copy_op(
    conn: &mut ThinClientConn,
    surface: ozmux_proto::SurfaceId,
    op: ozmux_proto::CopyModeOp,
) {
    send_cmd(conn, ozmux_proto::ClientMessage::CopyModeOp { surface, op });
}

/// Converts a `bevy_terminal::SelectionType` to its proto mirror
/// `ozmux_proto::SelectionKind`.
pub(crate) fn selection_type_to_kind(ty: SelectionType) -> ozmux_proto::SelectionKind {
    match ty {
        SelectionType::Simple => ozmux_proto::SelectionKind::Simple,
        SelectionType::Block => ozmux_proto::SelectionKind::Block,
        SelectionType::Lines => ozmux_proto::SelectionKind::Lines,
        SelectionType::Semantic => ozmux_proto::SelectionKind::Semantic,
    }
}

/// Whether the GUI runs an in-process daemon (tests/dev) or attaches to /
/// spawns a separate `ozmuxd` process (the real app). Defaults to `OutOfProcess`
/// (the real app); tests pin `InProcess` explicitly via `headless_app`.
#[derive(Clone, Copy, Default)]
pub(crate) enum DaemonMode {
    #[allow(
        dead_code,
        reason = "constructed only by the #[cfg(test)] headless_app harness; \
                  #[expect] would be unfulfilled in the test build where it IS constructed"
    )]
    InProcess,
    #[default]
    OutOfProcess,
}

/// Runs the GUI as a thin client over an `ozmuxd` daemon (in- or out-of-process).
#[derive(Default)]
pub struct ThinClientMultiplexerPlugin {
    pub(crate) mode: DaemonMode,
}

impl Plugin for ThinClientMultiplexerPlugin {
    fn build(&self, app: &mut App) {
        let (daemon, client, snapshot) = match self.mode {
            DaemonMode::InProcess => {
                let (handle, client, snapshot) =
                    boot_in_process().expect("thin-client: in-process daemon boot failed");
                (ThinDaemon::InProcess(handle), client, snapshot)
            }
            DaemonMode::OutOfProcess => {
                let (client, snapshot) =
                    attach_or_spawn().expect("thin-client: could not start or attach ozmuxd");
                (ThinDaemon::OutOfProcess, client, snapshot)
            }
        };
        let mut state = MuxState::from_snapshot(snapshot.clone());
        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, app.world());
            build_from_snapshot_checked(&mut commands, &mut state, &snapshot)
                .expect("thin-client: daemon sent an inconsistent session snapshot");
            stamp_attached_workspace(&mut commands, &state, &snapshot);
        }
        queue.apply(app.world_mut());
        app.insert_resource(state);
        app.init_resource::<PendingFocus>();
        app.init_resource::<ThinClientExiting>();
        app.insert_resource(ThinWorkspaceSeq(2));
        app.insert_resource(daemon);
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
    mut workspace_seq: ResMut<ThinWorkspaceSeq>,
    mut clipboard: ResMut<crate::clipboard::Clipboard>,
    mut exiting: ResMut<ThinClientExiting>,
    read: MirrorReadCtx,
    attached_q: Query<Entity, With<AttachedWorkspace>>,
    daemon: Res<ThinDaemon>,
    windows: Query<Entity, With<PrimaryWindow>>,
) {
    let mut budget = 256u32;
    while budget > 0 {
        budget -= 1;
        let msg = match conn.0.try_poll() {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => {
                if matches!(*daemon, ThinDaemon::OutOfProcess) {
                    error!("thin-client: daemon connection lost: {e}");
                    request_clean_exit(&mut commands, &mut exiting, &windows);
                } else {
                    error!("thin-client: wire poll error: {e}");
                }
                break;
            }
        };
        match msg {
            ServerMessage::Events(batch) => {
                if apply_events_checked(&mut commands, &mut state, &read, &batch).is_err() {
                    error!("thin-client: inconsistent daemon event — exiting");
                    request_clean_exit(&mut commands, &mut exiting, &windows);
                    break;
                }
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
                        MuxEvent::WorkspaceCreated { workspace, .. } => {
                            if let Some(ws_ent) = state.workspace_entity(*workspace) {
                                commands
                                    .entity(ws_ent)
                                    .insert(WorkspaceCreatedAt(workspace_seq.0));
                                workspace_seq.0 += 1;
                            }
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
            ServerMessage::SelectionCopied { surface: _, text } => {
                if !text.is_empty() {
                    clipboard.write(text);
                }
            }
        }
    }
}

/// Requests a clean GUI exit by despawning the primary window (winit's native
/// close path). Avoids the Bevy 0.18.1 macOS programmatic-`AppExit` freeze
/// (#23313). Idempotent. Sets `ThinClientExiting` so the debug mirror assert
/// skips during teardown.
fn request_clean_exit(
    commands: &mut Commands,
    exiting: &mut ThinClientExiting,
    windows: &Query<Entity, With<PrimaryWindow>>,
) {
    exiting.0 = true;
    for win in windows.iter() {
        commands.entity(win).despawn();
    }
}

/// Debug-only: assert the ECS tree matches the authoritative fold + no map leaks.
#[cfg(debug_assertions)]
fn debug_assert_ecs_matches_fold(world: &mut World) {
    use ozmux_multiplexer::{assert_no_map_leaks, ecs_matches_fold};
    if world
        .get_resource::<ThinClientExiting>()
        .is_some_and(|e| e.0)
    {
        return;
    }
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
fn boot_in_process() -> std::io::Result<(ozmuxd::ServerHandle, Client<UnixStream>, SessionSnapshot)>
{
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

/// Resolves the `ozmuxd` binary to spawn: `OZMUX_DAEMON_BIN` if set, else
/// `ozmuxd` next to the current executable (in dev `target/debug/ozmuxd` sits
/// beside `ozmux-gui`; a co-install shares a dir).
fn daemon_binary_path() -> std::io::Result<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("OZMUX_DAEMON_BIN") {
        return Ok(std::path::PathBuf::from(p));
    }
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "current_exe has no parent dir",
        )
    })?;
    Ok(dir.join("ozmuxd"))
}

/// Spawns a DETACHED `ozmuxd` on `socket_path`. `process_group(0)` (setpgid)
/// puts it in its own process group so a terminal Ctrl-C does not reach it;
/// stdio is nulled. The caller decides the child's fate: dropping the returned
/// `Child` orphans the daemon (no reaping Drop on `std::process::Child`), so it
/// persists past the GUI; calling `kill`/`wait` reaps it (the tests do this).
fn spawn_daemon(socket_path: &std::path::Path) -> std::io::Result<std::process::Child> {
    let bin = daemon_binary_path()?;
    std::process::Command::new(&bin)
        .arg(socket_path)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
}

/// Polls `connect` every 10 ms up to a 2 s deadline (the freshly-spawned daemon
/// needs a moment to bind). Returns the last error if the deadline elapses.
fn connect_with_retry(socket_path: &std::path::Path) -> std::io::Result<UnixStream> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match UnixStream::connect(socket_path) {
            Ok(s) => return Ok(s),
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

/// Attaches to a running daemon at `default_socket_path()` if present, else
/// spawns a detached one and connects-with-retry. The shutdown hook closes only
/// OUR connection on `Client` drop; the daemon survives (detach-on-exit).
fn attach_or_spawn() -> std::io::Result<(Client<UnixStream>, SessionSnapshot)> {
    attach_or_spawn_at(&ozmuxd::default_socket_path())
}

/// `attach_or_spawn`, parameterized on the socket path so tests can use a
/// per-test temp path instead of the shared default. Drops any `Child` we
/// spawned, orphaning the daemon (the persistence design); the real app never
/// reaps it.
fn attach_or_spawn_at(
    path: &std::path::Path,
) -> std::io::Result<(Client<UnixStream>, SessionSnapshot)> {
    let (client, snapshot, _orphaned_child) = attach_or_spawn_at_inner(path)?;
    Ok((client, snapshot))
}

/// Core of `attach_or_spawn_at`, additionally surfacing the spawned `Child`:
/// `Some` only when THIS call spawned a fresh daemon, `None` when it attached to
/// one already running. Production drops the child (orphan); tests reap it on
/// teardown so no daemon leaks.
fn attach_or_spawn_at_inner(
    path: &std::path::Path,
) -> std::io::Result<(
    Client<UnixStream>,
    SessionSnapshot,
    Option<std::process::Child>,
)> {
    let (stream, child) = match UnixStream::connect(path) {
        Ok(s) => (s, None),
        Err(_) => {
            let child = spawn_daemon(path)?;
            (connect_with_retry(path)?, Some(child))
        }
    };
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
    Ok((client, snapshot, child))
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
    use ozmux_multiplexer::ActiveSurface;

    /// Builds a headless `App` with just enough infrastructure for the
    /// `ThinClientMultiplexerPlugin` to boot, build entities, and pump frames.
    fn headless_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.add_plugins(ThinClientMultiplexerPlugin {
            mode: DaemonMode::InProcess,
        });
        app.insert_resource(crate::clipboard::Clipboard::new());
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

    /// Pumps the app, sleeping briefly between frames, until `pred` holds or the
    /// deadline elapses. Returns whether `pred` ever held. Generous by default
    /// because each step crosses a real daemon + UDS round-trip.
    fn pump_until<F>(app: &mut App, deadline: std::time::Duration, mut pred: F) -> bool
    where
        F: FnMut(&mut App) -> bool,
    {
        let start = std::time::Instant::now();
        while start.elapsed() < deadline {
            app.update();
            if pred(app) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // One last update + check after the loop body, in case the final sleep
        // pushed us past the deadline before the last pump landed.
        app.update();
        pred(app)
    }

    /// Returns the `(entity, PaneId)` of the single active pane built from the
    /// Welcome snapshot. Panics if zero or more than one pane exists.
    fn sole_active_pane(app: &mut App) -> (Entity, ozmux_proto::PaneId) {
        let mut q = app
            .world_mut()
            .query_filtered::<(Entity, &ozmux_multiplexer::MuxPaneId), With<ozmux_multiplexer::PaneMarker>>();
        let panes: Vec<_> = q.iter(app.world()).map(|(e, id)| (e, id.0)).collect();
        assert_eq!(panes.len(), 1, "expected exactly one pane from Welcome");
        panes[0]
    }

    fn pane_count(app: &mut App) -> usize {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<ozmux_multiplexer::PaneMarker>>();
        q.iter(app.world()).count()
    }

    fn workspace_count(app: &mut App) -> usize {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<ozmux_multiplexer::WorkspaceMarker>>();
        q.iter(app.world()).count()
    }

    /// (a) A `Split` sent over the wire folds back as `PaneCreated`, growing the
    /// ECS pane count from 1 to 2.
    #[test]
    fn thin_client_split_over_wire_adds_pane() {
        let mut app = headless_app();
        assert_eq!(pane_count(&mut app), 1, "one pane from Welcome");
        let (_, pane) = sole_active_pane(&mut app);
        {
            let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
            conn.0
                .send(ozmux_proto::ClientMessage::Split {
                    pane,
                    orientation: ozmux_proto::SplitOrientation::Horizontal,
                    side: ozmux_proto::Side::After,
                    kind: ozmux_proto::SurfaceKind::Terminal,
                    cwd: None,
                })
                .expect("send Split");
        }
        let grew = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            pane_count(app) >= 2
        });
        assert!(grew, "Split over the wire must fold PaneCreated → 2 panes");
        assert_eq!(pane_count(&mut app), 2, "exactly two panes after one Split");
    }

    /// (b) `CreateWorkspace` folds back as `WorkspaceCreated` (a 2nd workspace)
    /// plus `WorkspaceSelected`, which the pump uses to MOVE the single
    /// `AttachedWorkspace` marker onto the newly-created workspace.
    #[test]
    fn thin_client_create_workspace_restamps_attached() {
        let mut app = headless_app();
        assert_eq!(workspace_count(&mut app), 1, "one workspace from Welcome");
        let original_attached = {
            let mut q = app
                .world_mut()
                .query_filtered::<Entity, With<AttachedWorkspace>>();
            let attached: Vec<_> = q.iter(app.world()).collect();
            assert_eq!(attached.len(), 1, "exactly one AttachedWorkspace at boot");
            attached[0]
        };
        {
            let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
            conn.0
                .send(ozmux_proto::ClientMessage::CreateWorkspace { name: None })
                .expect("send CreateWorkspace");
        }
        let grew = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            workspace_count(app) >= 2
        });
        assert!(grew, "CreateWorkspace must fold a 2nd WorkspaceMarker");
        // The pump's WorkspaceSelected handler must move the lone AttachedWorkspace
        // marker onto a workspace that is NOT the original (the daemon selects the
        // freshly-created one on CreateWorkspace).
        let restamped = pump_until(&mut app, std::time::Duration::from_secs(5), |app| {
            let mut q = app
                .world_mut()
                .query_filtered::<Entity, With<AttachedWorkspace>>();
            let attached: Vec<_> = q.iter(app.world()).collect();
            attached.len() == 1 && attached[0] != original_attached
        });
        assert!(
            restamped,
            "pump's WorkspaceSelected re-stamp must move the single AttachedWorkspace to the new workspace"
        );
        // The pump must also stamp WorkspaceCreatedAt on the folded workspace
        // (the local path does this via MultiplexerCommands, gated out here); the
        // two workspaces must carry DISTINCT creation-order keys, else
        // FocusWorkspace/status-bar sort by an arbitrary u32::MAX tie.
        let mut stamps: Vec<u32> = {
            let mut q = app
                .world_mut()
                .query_filtered::<&WorkspaceCreatedAt, With<ozmux_multiplexer::WorkspaceMarker>>();
            q.iter(app.world()).map(|c| c.0).collect()
        };
        stamps.sort_unstable();
        assert_eq!(
            stamps,
            vec![1, 2],
            "boot workspace keeps WorkspaceCreatedAt(1); the folded one is stamped 2 (distinct), not left unstamped",
        );
    }

    /// (c) `SpawnSurface` with the pane registered in `PendingFocus`: the pump
    /// sees the folded `SurfaceSpawned`, sends `SetActiveSurface`, and the
    /// daemon's `ActiveSurfaceChanged` re-homes the pane's `ActiveSurface` to
    /// the new surface. Proves the full T5 pending-focus round-trip.
    #[test]
    fn thin_client_spawn_surface_pending_focus_changes_active() {
        let mut app = headless_app();
        let (pane_ent, pane_id) = sole_active_pane(&mut app);
        let original_active = app
            .world()
            .get::<ActiveSurface>(pane_ent)
            .expect("pane has ActiveSurface at boot")
            .0;
        app.world_mut()
            .resource_mut::<PendingFocus>()
            .0
            .insert(pane_id);
        {
            let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
            conn.0
                .send(ozmux_proto::ClientMessage::SpawnSurface {
                    pane: pane_id,
                    kind: ozmux_proto::SurfaceKind::Terminal,
                    cwd: None,
                })
                .expect("send SpawnSurface");
        }
        let focused = pump_until(&mut app, std::time::Duration::from_secs(10), |app| {
            let two_surfaces = app
                .world()
                .get::<ozmux_multiplexer::Surfaces>(pane_ent)
                .map(|s| s.iter().count())
                .unwrap_or(0)
                >= 2;
            let active_moved = app
                .world()
                .get::<ActiveSurface>(pane_ent)
                .map(|a| a.0 != original_active)
                .unwrap_or(false);
            two_surfaces && active_moved
        });
        assert!(
            focused,
            "PendingFocus + SpawnSurface must yield 2 surfaces AND a moved ActiveSurface (SetActiveSurface round-trip)"
        );
        assert!(
            app.world().get::<ActiveSurface>(pane_ent).unwrap().0 != original_active,
            "ActiveSurface must point at the newly-spawned surface, not the original"
        );
    }

    /// Builds a headless app and attaches a `TerminalGrid` to the sole active
    /// surface so the pump's `TerminalSnapshot`/`TerminalDelta` triggers (and the
    /// `ModeChanged` fold) have a grid to land on. The UI layer normally does this
    /// via `TerminalRenderBundle`; the headless harness must do it by hand.
    fn headless_app_with_grid() -> (App, ozmux_proto::SurfaceId, Entity) {
        let mut app = headless_app();
        app.add_plugins(bevy_terminal_renderer::TerminalGridPlugin);
        let surface_id = {
            let snap = app
                .world()
                .non_send_resource::<ThinClientConn>()
                .0
                .mirror()
                .to_snapshot();
            snap.workspaces
                .iter()
                .flat_map(|ws| ws.panes.iter())
                .flat_map(|p| p.surfaces.iter())
                .find(|s| matches!(s.kind, ozmux_proto::SurfaceKind::Terminal))
                .expect("a Terminal surface in the Welcome snapshot")
                .surface
        };
        let surface_ent = app
            .world()
            .resource::<MuxState>()
            .surface_entity(surface_id)
            .expect("surface entity built from Welcome");
        app.world_mut()
            .entity_mut(surface_ent)
            .insert(TerminalGrid::default());
        (app, surface_id, surface_ent)
    }

    /// True if any cell text across all rows of the surface's grid joins into a
    /// row string that contains `needle`.
    fn grid_contains(app: &App, surface_ent: Entity, needle: &str) -> bool {
        let Some(grid) = app.world().get::<TerminalGrid>(surface_ent) else {
            return false;
        };
        grid.cells.iter().any(|row| {
            row.iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .contains(needle)
        })
    }

    /// (d) Keyboard echo: `Input` over the wire reaches the PTY, the shell echoes
    /// it, the daemon frames it, the pump triggers a snapshot/delta, and the grid
    /// observers populate `TerminalGrid.cells` with the echoed text.
    #[test]
    fn thin_client_input_echoes_into_grid() {
        let (mut app, surface_id, surface_ent) = headless_app_with_grid();
        // Wait for the bootstrap frame so the grid is populated before we type.
        pump_until(&mut app, std::time::Duration::from_secs(5), |app| {
            app.world()
                .get::<TerminalGrid>(surface_ent)
                .map(|g| !g.cells.is_empty())
                .unwrap_or(false)
        });
        {
            let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
            conn.0
                .send(ozmux_proto::ClientMessage::Input {
                    surface: surface_id,
                    bytes: b"printf ZZ\n".to_vec(),
                })
                .expect("send Input");
        }
        let echoed = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            grid_contains(app, surface_ent, "ZZ")
        });
        assert!(
            echoed,
            "Input → daemon PTY → frame → pump → TerminalGrid must surface the echoed \"ZZ\""
        );
    }

    /// (e) Mode fold: an alt-screen enable sequence (`\x1b[?1049h`) reaches the
    /// PTY; the daemon emits the mode (in the snapshot's `modes` and as a
    /// `ModeChanged` SurfaceEvent), and the grid's `modes` ends up carrying
    /// "alt-screen".
    #[test]
    fn thin_client_alt_screen_mode_folds_into_grid() {
        let (mut app, surface_id, surface_ent) = headless_app_with_grid();
        pump_until(&mut app, std::time::Duration::from_secs(5), |app| {
            app.world()
                .get::<TerminalGrid>(surface_ent)
                .map(|g| !g.cells.is_empty())
                .unwrap_or(false)
        });
        // `printf '\033[?1049h'` makes the shell emit the alt-screen-enter escape.
        {
            let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
            conn.0
                .send(ozmux_proto::ClientMessage::Input {
                    surface: surface_id,
                    bytes: b"printf '\\033[?1049h'\n".to_vec(),
                })
                .expect("send Input");
        }
        let alt_screen = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            app.world()
                .get::<TerminalGrid>(surface_ent)
                .map(|g| g.modes.iter().any(|m| m == "alt-screen"))
                .unwrap_or(false)
        });
        assert!(
            alt_screen,
            "alt-screen-enter escape must fold into TerminalGrid.modes via snapshot/ModeChanged"
        );
    }

    /// Sends a `CopyModeOp` for `surface_id` directly via the wire client.
    fn send_op(app: &mut App, surface_id: ozmux_proto::SurfaceId, op: ozmux_proto::CopyModeOp) {
        let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
        conn.0
            .send(ozmux_proto::ClientMessage::CopyModeOp {
                surface: surface_id,
                op,
            })
            .expect("send CopyModeOp");
    }

    /// Pumps until the surface's `TerminalGrid.cells` is non-empty (the bootstrap
    /// frame has folded), so subsequent ops act on a populated grid.
    fn wait_for_cells(app: &mut App, surface_ent: Entity) {
        pump_until(app, std::time::Duration::from_secs(5), |app| {
            app.world()
                .get::<TerminalGrid>(surface_ent)
                .map(|g| !g.cells.is_empty())
                .unwrap_or(false)
        });
    }

    /// (f) Copy-mode entry over the wire: `CopyModeOp::Enter` enters vi-mode on
    /// the daemon, which emits a frame whose `vi_cursor` is `Some`; the pump folds
    /// it into the surface's `TerminalGrid.vi_cursor`. (Validates the daemon-driven
    /// vi_cursor wire path; `CopyModeState` is GUI-managed and not registered here.)
    #[test]
    fn thin_client_copy_mode_enter_renders_vi_cursor() {
        let (mut app, surface_id, surface_ent) = headless_app_with_grid();
        wait_for_cells(&mut app, surface_ent);
        send_op(&mut app, surface_id, ozmux_proto::CopyModeOp::Enter);
        // A motion forces a fresh frame in case Enter alone coalesces away.
        send_op(
            &mut app,
            surface_id,
            ozmux_proto::CopyModeOp::ViMotion(ozmux_proto::ViMotionKind::Up),
        );
        let has_cursor = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            app.world()
                .get::<TerminalGrid>(surface_ent)
                .map(|g| g.vi_cursor.is_some())
                .unwrap_or(false)
        });
        assert!(
            has_cursor,
            "CopyModeOp::Enter → daemon vi-mode → frame.vi_cursor → pump must set TerminalGrid.vi_cursor"
        );
    }

    /// (g) Keyboard copy-mode selection over the wire: enter copy mode, then
    /// `SelectionStartAt` + `SelectionUpdateTo`; the daemon's selection lands in
    /// the frame and the pump folds it into `TerminalGrid.selection`.
    #[test]
    fn thin_client_keyboard_selection_renders() {
        let (mut app, surface_id, surface_ent) = headless_app_with_grid();
        wait_for_cells(&mut app, surface_ent);
        send_op(&mut app, surface_id, ozmux_proto::CopyModeOp::Enter);
        send_op(
            &mut app,
            surface_id,
            ozmux_proto::CopyModeOp::SelectionStartAt {
                point: ozmux_proto::ViewportPoint { line: 0, col: 0 },
                side: ozmux_proto::CellSide::Left,
                ty: ozmux_proto::SelectionKind::Simple,
            },
        );
        send_op(
            &mut app,
            surface_id,
            ozmux_proto::CopyModeOp::SelectionUpdateTo {
                point: ozmux_proto::ViewportPoint { line: 0, col: 5 },
                side: ozmux_proto::CellSide::Right,
            },
        );
        let has_selection = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            app.world()
                .get::<TerminalGrid>(surface_ent)
                .map(|g| g.selection.is_some())
                .unwrap_or(false)
        });
        assert!(
            has_selection,
            "Enter + SelectionStartAt + SelectionUpdateTo must fold into TerminalGrid.selection"
        );
    }

    /// (h) `y` copy → clipboard over the wire: type known content, build a
    /// whole-screen Lines selection in copy mode, `CopySelection`; the daemon
    /// replies with `SelectionCopied`, the pump writes the `Clipboard`, and a
    /// read-back yields the selected text. Skipped when arboard is unavailable
    /// (headless CI) so the rest of the suite stays green.
    #[test]
    fn thin_client_copy_selection_writes_clipboard() {
        let (mut app, surface_id, surface_ent) = headless_app_with_grid();
        wait_for_cells(&mut app, surface_ent);
        {
            let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
            conn.0
                .send(ozmux_proto::ClientMessage::Input {
                    surface: surface_id,
                    bytes: b"printf COPYME\n".to_vec(),
                })
                .expect("send Input");
        }
        let echoed = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            grid_contains(app, surface_ent, "COPYME")
        });
        assert!(
            echoed,
            "printf COPYME must surface in the grid before copying"
        );
        send_op(&mut app, surface_id, ozmux_proto::CopyModeOp::Enter);
        send_op(
            &mut app,
            surface_id,
            ozmux_proto::CopyModeOp::SelectionStartAt {
                point: ozmux_proto::ViewportPoint { line: 0, col: 0 },
                side: ozmux_proto::CellSide::Left,
                ty: ozmux_proto::SelectionKind::Lines,
            },
        );
        send_op(
            &mut app,
            surface_id,
            ozmux_proto::CopyModeOp::SelectionUpdateTo {
                point: ozmux_proto::ViewportPoint { line: 23, col: 0 },
                side: ozmux_proto::CellSide::Right,
            },
        );
        send_op(&mut app, surface_id, ozmux_proto::CopyModeOp::CopySelection);
        if !app
            .world_mut()
            .resource_mut::<crate::clipboard::Clipboard>()
            .is_available_for_test()
        {
            eprintln!("skipping: arboard unavailable in this environment (e.g. headless CI)");
            return;
        }
        let copied = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            app.world_mut()
                .resource_mut::<crate::clipboard::Clipboard>()
                .read()
                .map(|t| t.contains("COPYME"))
                .unwrap_or(false)
        });
        assert!(
            copied,
            "CopySelection → SelectionCopied → pump → Clipboard must hold the selected \"COPYME\""
        );
    }

    /// Mouse drag-select over the wire: the mouse path emits `SelectionStartAt`
    /// then `SelectionUpdateTo` WITHOUT entering copy mode; the daemon's selection
    /// still folds into `TerminalGrid.selection`.
    #[test]
    fn thin_client_mouse_selection_renders() {
        let (mut app, surface_id, surface_ent) = headless_app_with_grid();
        wait_for_cells(&mut app, surface_ent);
        send_op(
            &mut app,
            surface_id,
            ozmux_proto::CopyModeOp::SelectionStartAt {
                point: ozmux_proto::ViewportPoint { line: 0, col: 0 },
                side: ozmux_proto::CellSide::Left,
                ty: ozmux_proto::SelectionKind::Simple,
            },
        );
        send_op(
            &mut app,
            surface_id,
            ozmux_proto::CopyModeOp::SelectionUpdateTo {
                point: ozmux_proto::ViewportPoint { line: 0, col: 8 },
                side: ozmux_proto::CellSide::Right,
            },
        );
        let has_selection = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            app.world()
                .get::<TerminalGrid>(surface_ent)
                .map(|g| g.selection.is_some())
                .unwrap_or(false)
        });
        assert!(
            has_selection,
            "mouse-path SelectionStartAt + SelectionUpdateTo (no Enter) must fold into TerminalGrid.selection"
        );
    }

    /// Covers the thin `apply_action`'s `ButtonAction` → `CopyModeOp` mapping
    /// end-to-end against the real in-process daemon — the gap the wire-level
    /// `thin_client_mouse_selection_renders` left (that test sends raw ops; this
    /// one drives the gesture machine). An `ArmDrag` press records the anchor,
    /// then a single `UpdateLocalSelection` to a different cell materializes the
    /// drag (sending `SelectionStartAt` for the anchor + `SelectionUpdateTo` for
    /// the new cell); the daemon's selection must round-trip into
    /// `TerminalGrid.selection`.
    #[test]
    fn thin_client_mouse_apply_action_selection_round_trips() {
        use crate::input::mouse_buttons::{MouseSelectionState, apply_action};
        let (mut app, surface_id, surface_ent) = headless_app_with_grid();
        wait_for_cells(&mut app, surface_ent);

        let mut state = MouseSelectionState::default();

        // 1-indexed cell coords, matching what the mouse path projects via
        // `cell_at_local`. Anchor at (1, 1); drag to (5, 1) on first move.
        let anchor = bevy_terminal::CellCoord { col: 1, row: 1 };
        let moved = bevy_terminal::CellCoord { col: 5, row: 1 };

        {
            let mut conn = app.world_mut().non_send_resource_mut::<ThinClientConn>();
            // Left press → ArmDrag: records the anchor, sends SelectionClear.
            apply_action(
                &mut conn,
                &mut state,
                bevy_terminal::ButtonEventKind::Press,
                bevy_terminal::MouseButtonKind::Left,
                bevy_terminal::ButtonAction::ArmDrag {
                    ty: bevy_terminal::SelectionType::Simple,
                    cell: anchor,
                    side: bevy_terminal::Side::Left,
                },
                surface_ent,
                surface_id,
                false,
                false,
            );
            // First inter-cell drag → UpdateLocalSelection: materializes the
            // armed drag, sending SelectionStartAt (anchor) + SelectionUpdateTo.
            apply_action(
                &mut conn,
                &mut state,
                bevy_terminal::ButtonEventKind::Drag,
                bevy_terminal::MouseButtonKind::Left,
                bevy_terminal::ButtonAction::UpdateLocalSelection {
                    cell: moved,
                    side: bevy_terminal::Side::Right,
                },
                surface_ent,
                surface_id,
                false,
                false,
            );
        }

        let has_selection = pump_until(&mut app, std::time::Duration::from_secs(8), |app| {
            app.world()
                .get::<TerminalGrid>(surface_ent)
                .map(|g| g.selection.is_some())
                .unwrap_or(false)
        });
        assert!(
            has_selection,
            "thin apply_action ArmDrag → UpdateLocalSelection must fold into TerminalGrid.selection"
        );
    }

    #[test]
    fn pump_requests_clean_exit_when_daemon_connection_drops() {
        let mut app = headless_app(); // InProcess: live ServerHandle held
        // Drop the in-process daemon (its ServerHandle Drop shuts the loop +
        // removes the socket → the client reader EOFs → the wire closes), then
        // switch the resource to OutOfProcess so the pump takes the exit branch.
        app.world_mut().remove_resource::<ThinDaemon>();
        app.insert_resource(ThinDaemon::OutOfProcess);
        let exited = pump_until(&mut app, std::time::Duration::from_secs(5), |app| {
            app.world().resource::<ThinClientExiting>().0
        });
        assert!(
            exited,
            "a dropped daemon connection must set ThinClientExiting in OutOfProcess mode"
        );
    }

    /// Resolves the built `ozmuxd` for the out-of-process tests: the test exe is
    /// under `target/<profile>/deps/`, so `ozmuxd` is two parents up.
    fn ozmuxd_bin() -> std::path::PathBuf {
        if let Some(p) = std::env::var_os("OZMUX_DAEMON_BIN") {
            return std::path::PathBuf::from(p);
        }
        let exe = std::env::current_exe().expect("current_exe");
        let target_dir = exe.parent().unwrap().parent().unwrap();
        target_dir.join("ozmuxd")
    }

    fn temp_socket(tag: &str) -> std::path::PathBuf {
        let pid = std::process::id();
        std::env::temp_dir().join(format!("ozmux-oop-{tag}-{pid}.sock"))
    }

    /// Reaps a test-spawned `ozmuxd` on Drop (`kill` + `wait`) so the
    /// out-of-process tests leave no orphaned daemon. Production never wraps the
    /// child this way — it drops the bare `Child`, orphaning the daemon.
    struct DaemonReaper(std::process::Child);

    impl Drop for DaemonReaper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn out_of_process_spawn_then_attach() {
        let bin = ozmuxd_bin();
        assert!(bin.exists(), "build ozmuxd first: {}", bin.display());
        // SAFETY: tests run with --test-threads=1 (single-threaded env mutation).
        unsafe { std::env::set_var("OZMUX_DAEMON_BIN", &bin) };
        let path = temp_socket("spawn");
        let _ = std::fs::remove_file(&path);

        let (client, snapshot, child) = attach_or_spawn_at_inner(&path).expect("spawn + connect");
        let _reaper = child.map(DaemonReaper);
        assert!(
            snapshot.active_workspace.is_some(),
            "Welcome snapshot must carry the bootstrap workspace"
        );
        drop(client);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn out_of_process_attach_to_existing_then_reattach() {
        let bin = ozmuxd_bin();
        assert!(bin.exists(), "build ozmuxd first: {}", bin.display());
        // SAFETY: tests run with --test-threads=1 (single-threaded env mutation).
        unsafe { std::env::set_var("OZMUX_DAEMON_BIN", &bin) };
        let path = temp_socket("attach");
        let _ = std::fs::remove_file(&path);

        let (c1, snap1, child) = attach_or_spawn_at_inner(&path).expect("first: spawn + connect");
        let _reaper = child.map(DaemonReaper);
        let panes1 = snap1.workspaces.iter().flat_map(|w| w.panes.iter()).count();
        let (c2, snap2, child2) = attach_or_spawn_at_inner(&path).expect("second: attach");
        assert!(child2.is_none(), "second call must ATTACH, not spawn");
        let panes2 = snap2.workspaces.iter().flat_map(|w| w.panes.iter()).count();
        assert_eq!(panes1, panes2, "the second client sees the same session");
        drop(c2);
        drop(c1);
        let (c3, snap3, child3) = attach_or_spawn_at_inner(&path).expect("reattach");
        assert!(child3.is_none(), "reattach must ATTACH, not spawn");
        assert!(
            snap3.active_workspace.is_some(),
            "session survives client disconnect (persistence)"
        );
        drop(c3);
        let _ = std::fs::remove_file(&path);
    }
}
