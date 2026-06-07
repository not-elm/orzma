//! Real-UDS integration tests for ozmuxd: a client over a Unix socket
//! reconstructs the daemon state; broadcast, version-mismatch, and disconnect
//! are exercised end-to-end.

use ozmux_mux::{PaneDirection, Side, SplitOrientation, SurfaceKind};
use ozmux_proto::{
    Client, ClientMessage, PROTOCOL_VERSION, ServerMessage, read_message, write_message,
};
use ozmuxd::Server;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// A unique-per-test socket path under the temp dir (short leaf for sun_path).
fn sock(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ozmuxd-it-{name}.sock"));
    let _ = std::fs::remove_file(&p);
    p
}

type ItClient = Client<BufReader<UnixStream>, UnixStream>;

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

#[test]
fn attach_receives_bootstrap_snapshot() {
    let path = sock("attach");
    let server = Server::new().serve(&path).unwrap();
    let client = connect(&path, (80, 24));
    let snap = client.mirror().to_snapshot();
    assert_eq!(snap.workspaces.len(), 1);
    assert!(snap.workspaces[0].active_pane.is_some());
    drop(server);
}

#[test]
fn command_broadcast_reconstructs_server_snapshot() {
    let path = sock("broadcast");
    let server = Server::new().serve(&path).unwrap();
    let mut client = connect(&path, (80, 24));
    drain(&mut client);

    let pane = client.mirror().to_snapshot().workspaces[0]
        .active_pane
        .unwrap();
    client
        .send(ClientMessage::Split {
            pane,
            orientation: SplitOrientation::Horizontal,
            side: Side::After,
            kind: SurfaceKind::Terminal,
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    drain(&mut client);

    let server_snap = server.snapshot().expect("server snapshot");
    assert_eq!(client.mirror().to_snapshot(), server_snap);
    drop(server);
}

#[test]
fn two_clients_converge() {
    let path = sock("twoclient");
    let server = Server::new().serve(&path).unwrap();
    let mut c1 = connect(&path, (80, 24));
    let mut c2 = connect(&path, (80, 24));
    drain(&mut c1);
    drain(&mut c2);

    let pane = c1.mirror().to_snapshot().workspaces[0].active_pane.unwrap();
    c1.send(ClientMessage::Split {
        pane,
        orientation: SplitOrientation::Vertical,
        side: Side::After,
        kind: SurfaceKind::Terminal,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    drain(&mut c1);
    drain(&mut c2);

    assert_eq!(c1.mirror().to_snapshot(), c2.mirror().to_snapshot());
    drop(server);
}

#[test]
fn version_mismatch_errs() {
    let path = sock("version");
    let server = Server::new().serve(&path).unwrap();
    // Raw connection: send a Hello with a bad version, expect an Error reply.
    let mut stream = {
        let mut s = None;
        for _ in 0..50 {
            if let Ok(st) = UnixStream::connect(&path) {
                s = Some(st);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        s.unwrap()
    };
    write_message(
        &mut stream,
        &ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION + 1,
            viewport: (80, 24),
        },
    )
    .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let reply: ServerMessage = read_message(&mut reader).unwrap().unwrap();
    assert!(
        matches!(reply, ServerMessage::Error { .. }),
        "expected Error, got {reply:?}"
    );
    drop(server);
}

#[test]
fn disconnect_survives() {
    let path = sock("disconnect");
    let server = Server::new().serve(&path).unwrap();
    let c1 = connect(&path, (80, 24));
    let mut c2 = connect(&path, (80, 24));
    drain(&mut c2);
    drop(c1);
    std::thread::sleep(Duration::from_millis(200));

    // c2 still works after c1 disconnected.
    let pane = c2.mirror().to_snapshot().workspaces[0].active_pane.unwrap();
    c2.send(ClientMessage::Navigate {
        pane,
        direction: PaneDirection::Right,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    drain(&mut c2);
    assert_eq!(c2.mirror().to_snapshot(), server.snapshot().unwrap());
    drop(server);
}
