//! End-to-end integration tests proving the daemon drives terminals and delivers
//! VT frames over the wire: echo round-trip, resize reflow, and surface-close
//! lifecycle.

use ozmux_mux::{Side, SplitOrientation, SurfaceId, SurfaceKind};
use ozmux_proto::{Client, ClientMessage, ServerMessage};
use ozmuxd::Server;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

type ItClient = Client<UnixStream>;

/// A unique-per-test socket path under the temp dir (short leaf for sun_path).
fn sock(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ozmuxd-fp-{name}.sock"));
    let _ = std::fs::remove_file(&p);
    p
}

/// Connects a Client with a generous read timeout so `poll` blocks briefly
/// while waiting for frames without hanging forever.
fn connect(path: &std::path::Path, viewport: (u16, u16)) -> ItClient {
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
    Client::connect(reader, stream, viewport).unwrap()
}

/// Walks the client mirror's snapshot for the first Terminal surface id.
fn first_terminal_surface(client: &ItClient) -> SurfaceId {
    let snap = client.mirror().to_snapshot();
    for ws in &snap.workspaces {
        for pane in &ws.panes {
            for surf in &pane.surfaces {
                if surf.kind == SurfaceKind::Terminal {
                    return surf.surface;
                }
            }
        }
    }
    panic!("no Terminal surface found in mirror snapshot");
}

/// Scans a `Frame`'s rows/runs for `needle` (both Snapshot.rows_data and Delta.dirty_rows).
fn frame_contains(frame: &ozmux_vt::frame::Frame, needle: &str) -> bool {
    use ozmux_vt::frame::{Frame, FrameDelta, FrameSnapshot};
    match frame {
        Frame::Snapshot(FrameSnapshot { rows_data, .. }) => rows_data
            .iter()
            .any(|r| r.runs.iter().any(|run| run.text.contains(needle))),
        Frame::Delta(FrameDelta { dirty_rows, .. }) => dirty_rows
            .iter()
            .any(|dr| dr.runs.iter().any(|run| run.text.contains(needle))),
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

/// THE MILESTONE: echo round-trip over the wire.
///
/// Connects a client, waits for the bootstrap snapshot, sends `printf ZZ\n`
/// to the shell, then asserts that `ZZ` appears in a subsequent frame.
#[test]
fn terminal_echo_round_trips_over_the_wire() {
    let path = sock("echo");
    let _server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));
    let surface = first_terminal_surface(&client);

    // 1. The bootstrap snapshot must arrive first.
    assert!(
        poll_until_frame(&mut client, Duration::from_secs(5), |s, f| {
            *s == surface && matches!(f, ozmux_vt::frame::Frame::Snapshot(_))
        }),
        "no bootstrap snapshot for the terminal surface within 5 s"
    );

    // 2. Send input.
    client
        .send(ClientMessage::Input {
            surface,
            bytes: b"printf ZZ\n".to_vec(),
        })
        .unwrap();

    // 3. The echoed `ZZ` must appear in a subsequent frame.
    assert!(
        poll_until_frame(&mut client, Duration::from_secs(8), |_s, f| {
            frame_contains(f, "ZZ")
        }),
        "echo 'ZZ' never appeared in any frame within 8 s"
    );
}

/// Sends `SetViewport` to 40×12 and asserts a `Frame::Snapshot` with `cols == 40`
/// arrives, proving the PTY was resized and a full snapshot was re-emitted.
#[test]
fn resize_reflows_the_pty() {
    let path = sock("resize");
    let _server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));
    let surface = first_terminal_surface(&client);

    // Drain the bootstrap snapshot.
    assert!(
        poll_until_frame(&mut client, Duration::from_secs(5), |s, f| {
            *s == surface && matches!(f, ozmux_vt::frame::Frame::Snapshot(_))
        }),
        "no bootstrap snapshot within 5 s"
    );

    // Request a resize.
    client
        .send(ClientMessage::SetViewport { cols: 40, rows: 12 })
        .unwrap();

    // A snapshot with cols == 40 must arrive after the resize.
    assert!(
        poll_until_frame(&mut client, Duration::from_secs(5), |_s, f| {
            matches!(f, ozmux_vt::frame::Frame::Snapshot(s) if s.cols == 40)
        }),
        "no Snapshot with cols == 40 arrived after SetViewport within 5 s"
    );
}

/// Splits (creating a second surface/driver), closes that pane, then asserts
/// that the server is still responsive for the original surface — no panic,
/// no hang.
#[test]
fn closing_a_surface_keeps_the_server_responsive() {
    let path = sock("lifecycle");
    let _server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));

    // Drain the bootstrap snapshot.
    let surface = first_terminal_surface(&client);
    assert!(
        poll_until_frame(&mut client, Duration::from_secs(5), |s, f| {
            *s == surface && matches!(f, ozmux_vt::frame::Frame::Snapshot(_))
        }),
        "no bootstrap snapshot within 5 s"
    );

    // Set a viewport so pane sizes are resolved (required for Split to produce
    // a valid geometry).
    client
        .send(ClientMessage::SetViewport { cols: 80, rows: 24 })
        .unwrap();
    // Give the server a moment to apply the viewport.
    std::thread::sleep(Duration::from_millis(100));

    // Split the active pane to create a second driver.
    let pane = client
        .mirror()
        .to_snapshot()
        .workspaces
        .first()
        .and_then(|ws| ws.active_pane)
        .expect("active pane");
    client
        .send(ClientMessage::Split {
            pane,
            orientation: SplitOrientation::Horizontal,
            side: Side::After,
            kind: SurfaceKind::Terminal,
        })
        .unwrap();

    // Drain events until mirror is updated with the new pane.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match client.poll() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(e) => panic!("unexpected poll error after Split: {e}"),
        }
    }

    // Find the new pane (the one that is not `pane`).
    let snap = client.mirror().to_snapshot();
    let all_panes: Vec<_> = snap
        .workspaces
        .iter()
        .flat_map(|ws| ws.panes.iter().map(|p| p.pane))
        .collect();
    assert!(
        all_panes.len() >= 2,
        "expected at least 2 panes after split, got {}",
        all_panes.len()
    );
    let new_pane = *all_panes.iter().find(|&&p| p != pane).unwrap();

    // Close the new pane.
    client
        .send(ClientMessage::Close { pane: new_pane })
        .unwrap();

    // The server must still be responsive: the original surface's driver should
    // still emit frames. Send a command to the original surface and poll.
    client
        .send(ClientMessage::Input {
            surface,
            bytes: b"printf QQ\n".to_vec(),
        })
        .unwrap();

    assert!(
        poll_until_frame(&mut client, Duration::from_secs(8), |_s, f| {
            frame_contains(f, "QQ")
        }),
        "server became unresponsive after closing the second surface: 'QQ' never appeared"
    );
}

/// A slow client that never reads its frames is marked lagged and has frames
/// dropped, but is NOT disconnected. After the flood ends, it catches up via
/// the resync snapshot path and keeps receiving frames.
///
/// The deterministic lag-mark, resync, throttle, and snapshot-reuse cases are
/// covered by unit tests in `surface_io.rs`; this test exercises the full
/// socket + driver path. Marked `#[ignore]` because the flood relies on the
/// kernel socket buffer and bounded frame channel filling up before `slow`
/// drains them, which is timing-dependent and can be flaky in CI.
// NOTE: The unit tests `fan_out_marks_full_client_lagged_and_drops`,
// `retry_catches_up_a_lagged_client_with_room`, `retry_throttles_snapshot_builds`,
// and `snapshot_reuse_clears_lag_at_emit` in `surface_io.rs` cover the QoS
// invariants deterministically; use those for regression. Run this test
// manually with `cargo test -p ozmuxd a_slow_client -- --ignored` on a quiet machine.
#[test]
#[ignore]
fn a_slow_client_lags_but_is_not_disconnected() {
    let path = sock("qos");
    let _server = Server::new().serve(&path).unwrap();
    let mut fast = connect(&path, (80, 24));
    let mut slow = connect(&path, (80, 24));
    let surface = first_terminal_surface(&fast);

    assert!(
        poll_until_frame(&mut fast, Duration::from_secs(3), |s, f| {
            *s == surface && matches!(f, ozmux_vt::frame::Frame::Snapshot(_))
        }),
        "fast: no bootstrap snapshot within 3 s"
    );
    assert!(
        poll_until_frame(&mut slow, Duration::from_secs(3), |s, f| {
            *s == surface && matches!(f, ozmux_vt::frame::Frame::Snapshot(_))
        }),
        "slow: no bootstrap snapshot within 3 s"
    );

    // Flood: `fast` drains continuously; `slow` does NOT read (socket +
    // bounded frame channel back up → lagged + dropped).
    fast.send(ClientMessage::Input {
        surface,
        bytes: b"for i in $(seq 1 20000); do echo line$i; done\n".to_vec(),
    })
    .unwrap();

    assert!(
        poll_until_frame(&mut fast, Duration::from_secs(15), |_s, f| {
            frame_contains(f, "line19000")
        }),
        "fast stays healthy through the flood"
    );

    // `slow` now reads: must still receive frames (NOT disconnected) and
    // eventually see output or a resync snapshot.
    let caught_up = poll_until_frame(&mut slow, Duration::from_secs(15), |s, f| {
        *s == surface
            && (matches!(f, ozmux_vt::frame::Frame::Snapshot(_)) || frame_contains(f, "line"))
    });
    assert!(
        caught_up,
        "slow client catches up via lossy drop + resync, not disconnect"
    );
}
