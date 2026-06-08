//! End-to-end integration tests for copy-mode / selection over the wire:
//! vi-cursor motion, selection start+update, full-scrollback copy extraction,
//! and originator-only `SelectionCopied` routing.

use ozmux_mux::{SurfaceId, SurfaceKind};
use ozmux_proto::{
    CellSide, Client, ClientMessage, CopyModeOp, SelectionKind, ServerMessage, ViMotionKind,
    ViewportPoint,
};
use ozmuxd::Server;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

type ItClient = Client<UnixStream>;

/// A unique-per-test socket path under the temp dir (short leaf for sun_path).
fn sock(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ozmuxd-cm-{name}.sock"));
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

fn frame_vi_cursor_some(frame: &ozmux_vt::frame::Frame) -> bool {
    use ozmux_vt::frame::Frame;
    match frame {
        Frame::Snapshot(s) => s.vi_cursor.is_some(),
        Frame::Delta(d) => d.vi_cursor.is_some(),
    }
}

fn frame_selection_some(frame: &ozmux_vt::frame::Frame) -> bool {
    use ozmux_vt::frame::Frame;
    match frame {
        Frame::Snapshot(s) => s.selection.is_some(),
        Frame::Delta(d) => d.selection.is_some(),
    }
}

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

/// Polls `client` up to `dur` until a `ServerMessage::Frame` matching `pred`
/// arrives. Returns `true` on a match, `false` on timeout.
fn poll_until_frame<F>(client: &mut ItClient, dur: Duration, pred: F) -> bool
where
    F: Fn(&ozmux_vt::frame::Frame) -> bool,
{
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        match client.poll() {
            Ok(Some(ServerMessage::Frame { frame, .. })) => {
                if pred(&frame) {
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

/// Polls `client` up to `dur` until a `ServerMessage::SelectionCopied` arrives;
/// returns the extracted text, or `None` on timeout.
fn poll_until_selection_copied(client: &mut ItClient, dur: Duration) -> Option<String> {
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        match client.poll() {
            Ok(Some(ServerMessage::SelectionCopied { text, .. })) => return Some(text),
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("unexpected poll error: {e}"),
        }
    }
    None
}

#[test]
fn copy_mode_vi_motion_moves_vi_cursor() {
    let path = sock("vimotion");
    let _server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));
    let surface = first_terminal_surface(&client);

    client
        .send(ClientMessage::Input {
            surface,
            bytes: b"printf 'a\\nb\\nc\\n'\n".to_vec(),
        })
        .unwrap();
    assert!(
        poll_until_frame(&mut client, Duration::from_secs(8), |f| {
            frame_contains(f, "c")
        }),
        "shell output must appear before entering copy mode"
    );

    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::Enter,
        })
        .unwrap();
    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::ViMotion(ViMotionKind::Up),
        })
        .unwrap();

    assert!(
        poll_until_frame(&mut client, Duration::from_secs(8), frame_vi_cursor_some),
        "a frame with vi_cursor Some must arrive after Enter + ViMotion(Up)"
    );
}

#[test]
fn selection_start_then_update_sets_selection() {
    let path = sock("selection");
    let _server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));
    let surface = first_terminal_surface(&client);

    client
        .send(ClientMessage::Input {
            surface,
            bytes: b"printf 'HELLO WORLD\\n'\n".to_vec(),
        })
        .unwrap();
    assert!(
        poll_until_frame(&mut client, Duration::from_secs(8), |f| {
            frame_contains(f, "HELLO")
        }),
        "shell output must appear before selecting"
    );

    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::Enter,
        })
        .unwrap();
    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::SelectionStartAt {
                point: ViewportPoint { line: 0, col: 0 },
                side: CellSide::Left,
                ty: SelectionKind::Simple,
            },
        })
        .unwrap();
    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::SelectionUpdateTo {
                point: ViewportPoint { line: 0, col: 3 },
                side: CellSide::Right,
            },
        })
        .unwrap();

    assert!(
        poll_until_frame(&mut client, Duration::from_secs(8), frame_selection_some),
        "a frame with selection Some must arrive after SelectionStartAt + SelectionUpdateTo"
    );
}

#[test]
fn copy_selection_across_scrollback_returns_full_text() {
    let path = sock("scrollback");
    let _server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));
    let surface = first_terminal_surface(&client);

    client
        .send(ClientMessage::Input {
            surface,
            bytes: b"for i in $(seq 1 60); do echo LINE$i; done\n".to_vec(),
        })
        .unwrap();
    assert!(
        poll_until_frame(&mut client, Duration::from_secs(10), |f| {
            frame_contains(f, "LINE60")
        }),
        "the full scrollback range must be produced before copying"
    );

    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::Enter,
        })
        .unwrap();
    // Scroll all the way back so early lines (LINE1..) sit at the top of the
    // viewport, anchor a whole-line selection there, then extend it downward
    // with ViMotion(Down) far enough to cross the scrollback boundary and reach
    // the late lines. A Lines selection spanning the full history proves
    // full-history extraction (not just the visible viewport).
    for _ in 0..6 {
        client
            .send(ClientMessage::CopyModeOp {
                surface,
                op: CopyModeOp::ScrollPageUp,
            })
            .unwrap();
    }
    // Anchor at the top of the scrolled-back viewport (line 0) and pin the vi
    // cursor there too (High snaps the cursor to the top visible row).
    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::ViMotion(ViMotionKind::High),
        })
        .unwrap();
    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::SelectionStart {
                ty: SelectionKind::Lines,
            },
        })
        .unwrap();
    // Extend the selection down across the full output (62+ lines) so the span
    // crosses the scrollback boundary; Down past the live tail is clamped.
    for _ in 0..80 {
        client
            .send(ClientMessage::CopyModeOp {
                surface,
                op: CopyModeOp::ViMotion(ViMotionKind::Down),
            })
            .unwrap();
    }
    client
        .send(ClientMessage::CopyModeOp {
            surface,
            op: CopyModeOp::CopySelection,
        })
        .unwrap();

    let text = poll_until_selection_copied(&mut client, Duration::from_secs(10))
        .expect("a SelectionCopied must arrive after CopySelection within 10 s");
    assert!(
        text.contains("LINE5"),
        "copied text must include an early scrollback line (LINE5); got:\n{text}"
    );
    assert!(
        text.contains("LINE55"),
        "copied text must include a late line (LINE55), proving full-span extraction; got:\n{text}"
    );
}

#[test]
fn selection_copied_routes_to_originating_client_only() {
    let path = sock("routing");
    let _server = Server::new().serve(&path).unwrap();
    let mut a = connect(&path, (80, 24));
    let mut b = connect(&path, (80, 24));
    let surface = first_terminal_surface(&a);

    a.send(ClientMessage::Input {
        surface,
        bytes: b"printf 'ROUTECHECK\\n'\n".to_vec(),
    })
    .unwrap();
    assert!(
        poll_until_frame(&mut a, Duration::from_secs(8), |f| {
            frame_contains(f, "ROUTECHECK")
        }),
        "A must see the shell output before selecting"
    );
    // Drain B's frames so its queue is current.
    let _ = poll_until_frame(&mut b, Duration::from_secs(3), |f| {
        frame_contains(f, "ROUTECHECK")
    });

    a.send(ClientMessage::CopyModeOp {
        surface,
        op: CopyModeOp::Enter,
    })
    .unwrap();
    a.send(ClientMessage::CopyModeOp {
        surface,
        op: CopyModeOp::SelectionStartAt {
            point: ViewportPoint { line: 0, col: 0 },
            side: CellSide::Left,
            ty: SelectionKind::Lines,
        },
    })
    .unwrap();
    a.send(ClientMessage::CopyModeOp {
        surface,
        op: CopyModeOp::ViMotion(ViMotionKind::Up),
    })
    .unwrap();
    a.send(ClientMessage::CopyModeOp {
        surface,
        op: CopyModeOp::CopySelection,
    })
    .unwrap();

    let text = poll_until_selection_copied(&mut a, Duration::from_secs(10))
        .expect("the originating client A must receive SelectionCopied");
    assert!(
        !text.is_empty(),
        "A's copied text must be non-empty (a Lines selection always spans a line)"
    );

    // B must NOT receive a SelectionCopied at all.
    assert!(
        poll_until_selection_copied(&mut b, Duration::from_secs(2)).is_none(),
        "the non-originating client B must NOT receive SelectionCopied"
    );
}
