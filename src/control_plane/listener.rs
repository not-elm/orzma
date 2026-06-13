//! Tokio-free control-plane listener: an accept loop thread plus one reader
//! thread per connection. Each connection is peer-UID-checked, must `hello`
//! with a valid token (resolved via `TokenRegistry` to its surface), then its
//! `register`/`unregister` lines and its disconnect are emitted as
//! `ControlEvent`s. A bounded reply channel per request carries the minted
//! handle back from the ECS apply system. Mirrors `rpc_client.rs`.

use crate::control_plane::TokenRegistry;
use crate::control_plane::protocol::{ClientMsg, RegisterKind, ServerMsg};
use bevy::prelude::Entity;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};

/// An event the listener emits to the ECS apply system.
pub(crate) enum ControlEvent {
    /// A `register` from a hello'd connection; the apply system mints a handle,
    /// populates the registries, and sends the reply back on `reply`.
    Register {
        /// Connection id (for scoped teardown).
        connection_id: u64,
        /// The surface the connection's token resolved to.
        owner_surface: Entity,
        /// The requested content source + policy.
        kind: RegisterKind,
        /// Where the apply system returns the `ServerMsg` reply.
        reply: Sender<ServerMsg>,
    },
    /// An `unregister` from a connection.
    Unregister {
        /// Connection id (ownership check).
        connection_id: u64,
        /// The handle to release.
        handle: String,
    },
    /// A connection closed; purge all its handles.
    Disconnect {
        /// Connection id.
        connection_id: u64,
    },
}

/// Binds `sock_path`, spawns the accept loop, and returns the receiver of
/// `ControlEvent`s. The accept loop and per-connection readers run on detached
/// threads (process-lifetime; the socket is removed when the runtime dir drops).
pub(crate) fn spawn_listener(
    sock_path: &std::path::Path,
    tokens: TokenRegistry,
) -> std::io::Result<Receiver<ControlEvent>> {
    let _ = std::fs::remove_file(sock_path);
    let listener = UnixListener::bind(sock_path)?;
    let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
    let next_id = AtomicU64::new(1);
    // SAFETY: `getuid` has no preconditions and cannot fail.
    let own_uid = unsafe { libc::getuid() } as u32;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if peer_uid(&stream) != Some(own_uid) {
                continue;
            }
            let connection_id = next_id.fetch_add(1, Ordering::SeqCst);
            let ev_tx = ev_tx.clone();
            let tokens = tokens.clone();
            std::thread::spawn(move || serve_connection(stream, connection_id, tokens, ev_tx));
        }
    });
    Ok(ev_rx)
}

/// Returns the connecting peer's UID via `getpeereid`, or `None` on error.
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `stream` owns a valid connected socket fd for the duration of the
    // call; `uid`/`gid` are valid out-params.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (rc == 0).then_some(uid as u32)
}

/// Reads one connection: requires a valid `hello`, then forwards each
/// `register`/`unregister`, then emits `Disconnect` on EOF.
fn serve_connection(
    mut stream: UnixStream,
    connection_id: u64,
    tokens: TokenRegistry,
    events: Sender<ControlEvent>,
) {
    let read_half = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut lines = BufReader::new(read_half);
    let owner_surface = match read_hello(&mut lines, &tokens) {
        Some(surface) => surface,
        None => return,
    };
    let mut buf = String::new();
    loop {
        buf.clear();
        match lines.read_line(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let line = buf.trim_end_matches(['\n', '\r']);
                let Ok(msg) = serde_json::from_str::<ClientMsg>(line) else {
                    continue;
                };
                match msg {
                    ClientMsg::Register(kind) => {
                        let (reply_tx, reply_rx) = bounded::<ServerMsg>(1);
                        if events
                            .send(ControlEvent::Register {
                                connection_id,
                                owner_surface,
                                kind,
                                reply: reply_tx,
                            })
                            .is_err()
                        {
                            break;
                        }
                        let reply = reply_rx
                            .recv()
                            .unwrap_or_else(|_| ServerMsg::err("internal"));
                        if write_reply(&mut stream, &reply).is_err() {
                            break;
                        }
                    }
                    ClientMsg::Unregister { handle } => {
                        let _ = events.send(ControlEvent::Unregister {
                            connection_id,
                            handle,
                        });
                    }
                    ClientMsg::Hello { .. } => {}
                }
            }
        }
    }
    let _ = events.send(ControlEvent::Disconnect { connection_id });
}

/// Reads the first line, requiring a `hello` whose token resolves to a surface.
fn read_hello(lines: &mut BufReader<UnixStream>, tokens: &TokenRegistry) -> Option<Entity> {
    let mut buf = String::new();
    if matches!(lines.read_line(&mut buf), Ok(0) | Err(_)) {
        return None;
    }
    let msg = serde_json::from_str::<ClientMsg>(buf.trim_end_matches(['\n', '\r'])).ok()?;
    let ClientMsg::Hello { token } = msg else {
        return None;
    };
    tokens.resolve(&token)
}

/// Writes one `ServerMsg` reply line (`\n`-terminated).
fn write_reply(stream: &mut UnixStream, reply: &ServerMsg) -> std::io::Result<()> {
    let line = serde_json::to_string(reply).expect("ServerMsg serializes infallibly");
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    #[test]
    fn hello_then_register_emits_a_register_event_and_replies() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        let surface = Entity::from_bits(11);
        tokens.insert("tok", surface);

        let events = spawn_listener(&sock, tokens).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        writeln!(
            client,
            r#"{{"op":"register","kind":"inline","html":"<h1>x</h1>"}}"#
        )
        .unwrap();
        client.flush().unwrap();

        let ev = events
            .recv_timeout(Duration::from_secs(2))
            .expect("a Register event");
        let reply = match ev {
            ControlEvent::Register {
                owner_surface,
                reply,
                ..
            } => {
                assert_eq!(owner_surface, surface);
                reply
            }
            _ => panic!("expected a Register event"),
        };
        reply.send(ServerMsg::ok("HANDLE1")).unwrap();

        let mut line = String::new();
        BufReader::new(client.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.contains(r#""handle":"HANDLE1""#), "got {line}");
    }

    #[test]
    fn unknown_token_drops_the_connection_without_events() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let events = spawn_listener(&sock, TokenRegistry::default()).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"bogus"}}"#).unwrap();
        client.flush().unwrap();

        assert!(
            events.recv_timeout(Duration::from_millis(300)).is_err(),
            "a bad token must not produce any ControlEvent"
        );
    }

    #[test]
    fn disconnect_emits_a_disconnect_event() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        tokens.insert("tok", Entity::from_bits(1));
        let events = spawn_listener(&sock, tokens).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        client.flush().unwrap();
        drop(client);

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(ControlEvent::Disconnect { .. }) =
                events.recv_timeout(Duration::from_millis(50))
            {
                break;
            }
            assert!(Instant::now() < deadline, "no Disconnect within 2s");
        }
    }
}
