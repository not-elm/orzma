//! In-process thin-client multiplexer: boots an `ozmuxd` Server on a temp UDS,
//! connects a proto `Client`, and builds the ECS tree from the Welcome snapshot.
//! The pump + render + viewport systems are added in later tasks.

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy_terminal::SelectionType;
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
        app.insert_resource(ThinWorkspaceSeq(2));
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
    mut workspace_seq: ResMut<ThinWorkspaceSeq>,
    mut clipboard: ResMut<crate::clipboard::Clipboard>,
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
    use ozmux_multiplexer::ActiveSurface;

    /// Builds a headless `App` with just enough infrastructure for the
    /// `ThinClientMultiplexerPlugin` to boot, build entities, and pump frames.
    fn headless_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.add_plugins(ThinClientMultiplexerPlugin);
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
}
