//! Multi-client (2-client) integration tests for ozmuxd (P4c-2b-2 T5):
//! the active workspace sizes to the component-wise MIN viewport across attached
//! clients (recomputed on attach/disconnect/setviewport/create-workspace),
//! structural changes (split) resize the underlying drivers (observed via FRAME
//! dimensions, not just the mirror layout), and a freshly-created workspace is
//! seeded at the client min size rather than 0×0. These spawn REAL daemons over
//! Unix sockets and reconstruct state through the client mirror.

use ozmux_mux::{LayoutNode, Side, SplitOrientation, SurfaceId, SurfaceKind};
use ozmux_proto::{Client, ClientMessage, ServerMessage};
use ozmuxd::Server;
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

type ItClient = Client<UnixStream>;

/// A unique-per-test socket path under the temp dir (short leaf for sun_path).
fn sock(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ozmuxd-mc-{name}.sock"));
    let _ = std::fs::remove_file(&p);
    p
}

/// Connects a Client over a real UnixStream (caller splits via try_clone) with a
/// read timeout so `poll` returns promptly when no event is pending.
fn connect(path: &std::path::Path, viewport: (u16, u16)) -> ItClient {
    // The accept thread may not be bound the instant serve() returns; retry briefly.
    let stream = {
        let mut last_err = None;
        let mut s = None;
        for _ in 0..50 {
            match UnixStream::connect(path) {
                Ok(st) => {
                    s = Some(st);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        s.unwrap_or_else(|| panic!("connect failed: {last_err:?}"))
    };
    // NOTE: set_read_timeout after the stream is created but before Client::connect
    // so the blocking Hello/Welcome exchange completes within the timeout window
    // (server sends Welcome immediately on Attach, well within 300ms).
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let reader = BufReader::new(stream.try_clone().unwrap());
    Client::connect(reader, stream, viewport).unwrap()
}

/// Connects like `connect` but wires a shutdown closure so dropping the `Client`
/// fully closes the socket (`Shutdown::Both`). Without it the background reader
/// thread keeps a cloned read-half fd alive, the kernel never sends FIN, and the
/// daemon never observes the disconnect — so the grow-back-on-disconnect path
/// can't be exercised. This mirrors how the real GUI client wires its shutdown
/// (see `Client::connect_with_shutdown`).
fn connect_disconnectable(path: &std::path::Path, viewport: (u16, u16)) -> ItClient {
    let stream = {
        let mut last_err = None;
        let mut s = None;
        for _ in 0..50 {
            match UnixStream::connect(path) {
                Ok(st) => {
                    s = Some(st);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        s.unwrap_or_else(|| panic!("connect failed: {last_err:?}"))
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let reader = BufReader::new(stream.try_clone().unwrap());
    let teardown = stream.try_clone().unwrap();
    let shutdown: Box<dyn FnOnce() + Send> = Box::new(move || {
        let _ = teardown.shutdown(Shutdown::Both);
    });
    Client::connect_with_shutdown(reader, stream, viewport, Some(shutdown)).unwrap()
}

/// Drains all currently-available events into the client mirror until the read
/// times out (quiescent). Panics on any unexpected protocol error so a real
/// bug surfaces rather than being silently swallowed as a quiescent stop.
fn drain(c: &mut ItClient) {
    loop {
        match c.poll() {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(e) => panic!("unexpected protocol error during drain: {e}"),
        }
    }
}

/// Polls `client` for up to `dur` until a `ServerMessage::Frame` matching `pred`
/// arrives. Returns `true` on a match, `false` on timeout.
fn poll_until_frame<F>(client: &mut ItClient, dur: Duration, pred: F) -> bool
where
    F: Fn(&SurfaceId, &ozmux_vt::frame::Frame) -> bool,
{
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        match client.poll() {
            Ok(Some(ServerMessage::Frame { surface, frame })) => {
                if pred(&surface, &frame) {
                    return true;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // read timeout — no data yet, spin
            }
            Err(e) => panic!("unexpected poll error: {e}"),
        }
    }
    false
}

/// The resolved root-pane cols of the *active* workspace (resolved via
/// `active_workspace`, since workspaces are stored in insertion order, not
/// active-first — a newly created workspace lands at the end of the list).
fn active_workspace_pane_cols(c: &ItClient) -> u16 {
    let snap = c.mirror().to_snapshot();
    let active = snap.active_workspace.expect("an active workspace");
    let ws = snap
        .workspaces
        .iter()
        .find(|w| w.workspace == active)
        .expect("active workspace present in snapshot");
    match &ws.layout {
        LayoutNode::Pane { cols, .. } => *cols,
        other => panic!("expected a single Pane layout, got {other:?}"),
    }
}

/// Re-reads `c`'s mirror until the *active* workspace's root pane cols equal
/// `want` or `dur` elapses, draining events between polls.
fn wait_for_active_workspace_cols(c: &mut ItClient, want: u16, dur: Duration) -> u16 {
    let deadline = Instant::now() + dur;
    loop {
        drain(c);
        let got = active_workspace_pane_cols(c);
        if got == want || Instant::now() >= deadline {
            return got;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Polls both clients until their mirror snapshots converge, or the deadline elapses.
/// Returns whether they converged.
fn wait_for_mirrors_converge(a: &mut ItClient, b: &mut ItClient, dur: Duration) -> bool {
    let deadline = Instant::now() + dur;
    loop {
        drain(a);
        drain(b);
        if a.mirror().to_snapshot() == b.mirror().to_snapshot() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Two clients at different viewports: the active workspace sizes to their
/// component-wise MIN, and grows back to the lone client's size on disconnect.
#[test]
fn grid_sizes_to_min_across_clients_and_grows_on_disconnect() {
    let path = sock("min-sizing");
    let server = Server::new().serve(&path).unwrap();
    let mut big = connect(&path, (100, 30));
    drain(&mut big);
    assert_eq!(
        active_workspace_pane_cols(&big),
        100,
        "single client → its own size"
    );

    let mut small = connect_disconnectable(&path, (80, 24));
    assert_eq!(
        wait_for_active_workspace_cols(&mut big, 80, Duration::from_secs(2)),
        80,
        "two clients → min (80)"
    );
    drain(&mut small);
    assert_eq!(
        active_workspace_pane_cols(&small),
        80,
        "both mirrors agree on the min"
    );

    drop(small);
    assert_eq!(
        wait_for_active_workspace_cols(&mut big, 100, Duration::from_secs(2)),
        100,
        "grid grows back to the lone client's size after disconnect"
    );
    drop(server);
}

/// A split is a structural change: the daemon must resize the new + existing
/// drivers' Vt/Pty to the min-resolved size. The mirror layout alone would
/// converge even without the driver resize, so this proves it via a FRAME at the
/// resolved split width (strictly < 80) — the driver was resized, not left at its
/// 80×24 seed.
#[test]
fn split_resizes_drivers_to_min_resolved_size() {
    let path = sock("split-resize");
    let server = Server::new().serve(&path).unwrap();
    let mut a = connect(&path, (100, 30));
    let mut b = connect(&path, (80, 24));
    // The grid must settle at the min (80) before we split.
    assert_eq!(
        wait_for_active_workspace_cols(&mut a, 80, Duration::from_secs(2)),
        80,
        "grid settles at min (80) before split"
    );
    drain(&mut b);

    let pane = a.mirror().to_snapshot().workspaces[0].active_pane.unwrap();
    a.send(ClientMessage::Split {
        pane,
        orientation: SplitOrientation::Horizontal,
        side: Side::After,
        kind: SurfaceKind::Terminal,
        cwd: None,
    })
    .unwrap();

    // The DRIVER (Vt/Pty) must actually be resized to the post-split width — prove
    // it via a Frame::Snapshot at a width strictly between 0 and 80 (a horizontal
    // split of the 80-col grid halves it to 40). The mirror layout alone would
    // converge even WITHOUT the A2 driver-resize, so the FRAME is the load-bearing
    // assertion: it shows the driver was resized, not left at its 80×24 seed.
    // `poll_until_frame` also folds the split's Events into the mirror as it polls,
    // so it advances the mirror AND watches the resize frame in one pass (a
    // separate `drain` would consume the resize frame before we could match it).
    assert!(
        poll_until_frame(&mut a, Duration::from_secs(5), |_s, f| {
            matches!(f, ozmux_vt::frame::Frame::Snapshot(snap) if snap.cols > 0 && snap.cols < 80)
        }),
        "a Frame::Snapshot at the post-split width (< 80) must arrive — the driver was resized, not left at its 80×24 seed"
    );

    // Let `b` catch up, then assert both mirrors converged to the same split.
    assert!(
        wait_for_mirrors_converge(&mut a, &mut b, Duration::from_secs(2)),
        "both client mirrors must converge to the same layout after the split"
    );

    // The converged layout is a Split whose children both resolve to a width
    // strictly less than the 80-col grid. (With Side::After the new pane is
    // `second`; both halves resolve to 40, so we accept any leaf < 80 rather than
    // assuming first-vs-second ordering.)
    let snap = a.mirror().to_snapshot();
    match &snap.workspaces[0].layout {
        LayoutNode::Split { first, second, .. } => {
            let pane_cols = |node: &LayoutNode| match node {
                LayoutNode::Pane { cols, .. } => Some(*cols),
                _ => None,
            };
            let leaf = pane_cols(first)
                .into_iter()
                .chain(pane_cols(second))
                .find(|&c| c > 0 && c < 80);
            assert!(
                leaf.is_some(),
                "the converged split must have a leaf width in (0, 80), got {snap:?}"
            );
        }
        other => panic!("expected a Split after splitting, got {other:?}"),
    }
    drop(server);
}

/// Creating a workspace while two clients are attached must size the new
/// workspace to the client min (80), not to a degenerate 0×0.
#[test]
fn new_workspace_is_sized_to_client_min_not_zero() {
    let path = sock("ws-switch");
    let server = Server::new().serve(&path).unwrap();
    let mut a = connect(&path, (100, 30));
    let mut b = connect(&path, (80, 24));
    assert_eq!(
        wait_for_active_workspace_cols(&mut a, 80, Duration::from_secs(2)),
        80,
        "grid settles at min (80) before create-workspace"
    );
    drain(&mut b);

    // Record the surfaces that exist before the new workspace, so we can match the
    // *new* terminal's seed frame (a surface outside this set).
    let pre_surfaces: Vec<SurfaceId> = a
        .mirror()
        .to_snapshot()
        .workspaces
        .iter()
        .flat_map(|ws| ws.panes.iter())
        .flat_map(|p| p.surfaces.iter().map(|s| s.surface))
        .collect();

    a.send(ClientMessage::CreateWorkspace { name: None })
        .unwrap();

    // The created workspace is a real, driver-backed terminal seeded at the min
    // size: a Frame::Snapshot at cols == 80 for a brand-new surface must arrive.
    // This is the load-bearing proof that the new workspace is seeded at the
    // resolved min, not at a degenerate 0×0. `poll_until_frame` also folds the
    // CreateWorkspace Events into the mirror as it polls (so the layout converges
    // without a `drain` that would consume the seed frame first).
    assert!(
        poll_until_frame(&mut a, Duration::from_secs(5), |s, f| {
            !pre_surfaces.contains(s)
                && matches!(f, ozmux_vt::frame::Frame::Snapshot(snap) if snap.cols == 80)
        }),
        "the new workspace's terminal must seed a Frame at the min size (80), not 0×0"
    );

    // The new workspace becomes active and lands at the END of the (insertion-
    // ordered) workspace list, so assert on the *active* workspace, not [0].
    assert_eq!(
        wait_for_active_workspace_cols(&mut a, 80, Duration::from_secs(2)),
        80,
        "new (active) workspace sized to the client min in the mirror layout"
    );
    drain(&mut b);
    assert_eq!(
        active_workspace_pane_cols(&b),
        80,
        "b's mirror agrees on the new workspace's size"
    );
    drop(server);
}
