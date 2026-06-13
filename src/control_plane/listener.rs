//! Tokio-free control-plane listener: an accept loop thread plus one reader
//! thread and one writer thread per connection. Each connection is
//! peer-UID-checked, must `hello` with a valid token (resolved via
//! `TokenRegistry` to its surface), then its `register`/`unregister` lines
//! and its disconnect are emitted as `ControlEvent`s. A bounded reply channel
//! per request carries the minted handle back from the ECS apply system; the
//! reply is relayed through the per-connection writer thread so all writes go
//! through a single owner. Mirrors `rpc_client.rs`.

use crate::control_plane::ConnectionWriters;
use crate::control_plane::TokenRegistry;
use crate::control_plane::protocol::{ClientMsg, RegisterKind, ServerMsg};
use bevy::prelude::Entity;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::ops::ControlFlow;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};

/// An event the listener emits to the ECS apply system.
#[allow(dead_code, reason = "Reply/Emit fields are consumed in stage1 task 9")]
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
    /// A program's reply to an ozmux-initiated back-channel `call`.
    Reply {
        /// The global reqId the apply system correlates.
        req_id: String,
        /// Whether the call succeeded.
        ok: bool,
        /// The success value.
        value: Value,
        /// The error message when `ok` is false.
        error: Option<String>,
        /// The connection that sent the reply (for in-flight ownership).
        connection_id: u64,
    },
    /// A program-initiated push to its handle's webviews.
    Emit {
        /// The connection that sent the emit (ownership is checked in apply).
        connection_id: u64,
        /// The target handle.
        handle: String,
        /// The event name.
        event: String,
        /// The event payload.
        payload: Value,
    },
}

/// Binds `sock_path`, spawns the accept loop, and returns the receiver of
/// `ControlEvent`s. The accept loop, per-connection readers, and per-connection
/// writers run on detached threads (process-lifetime; the socket is removed
/// when the runtime dir drops).
pub(crate) fn spawn_listener(
    sock_path: &std::path::Path,
    tokens: TokenRegistry,
    writers: ConnectionWriters,
) -> std::io::Result<Receiver<ControlEvent>> {
    let _ = std::fs::remove_file(sock_path);
    let listener = UnixListener::bind(sock_path)?;
    let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
    let mut next_id: u64 = 1;
    // SAFETY: `getuid` has no preconditions and cannot fail.
    let own_uid = unsafe { libc::getuid() } as u32;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if peer_uid(&stream) != Some(own_uid) {
                continue;
            }
            let connection_id = next_id;
            next_id += 1;
            let ev_tx = ev_tx.clone();
            let tokens = tokens.clone();
            let writers = writers.clone();
            std::thread::spawn(move || {
                serve_connection(stream, connection_id, tokens, ev_tx, writers);
            });
        }
    });
    Ok(ev_rx)
}

/// Returns the connecting peer's UID via `getpeereid` (Apple/BSD), or `None` on
/// error. The `libc` crate exposes `getpeereid` only on these targets; Linux
/// uses the `SO_PEERCRED` variant below.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: `stream` owns a valid connected socket fd for the duration of the
    // call; `uid`/`gid` are valid out-params.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (rc == 0).then_some(uid as u32)
}

/// Returns the connecting peer's UID via the `SO_PEERCRED` socket option
/// (Linux/Android), or `None` on error.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `stream` owns a valid connected socket fd; `getsockopt` writes a
    // `ucred` of `len` bytes into `cred`, and both out-params are valid.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast::<libc::c_void>(),
            &mut len,
        )
    };
    (rc == 0).then_some(cred.uid)
}

/// Reads one connection: requires a valid `hello`, spawns a writer thread for
/// all outbound lines (register replies + future server-push), registers the
/// writer in `writers`, then forwards each `register`/`unregister`, then emits
/// `Disconnect` and removes the writer on EOF.
fn serve_connection(
    stream: UnixStream,
    connection_id: u64,
    tokens: TokenRegistry,
    events: Sender<ControlEvent>,
    writers: ConnectionWriters,
) {
    let read_half = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut write_half = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut lines = BufReader::new(read_half);
    let owner_surface = match read_hello(&mut lines, &tokens) {
        Some(surface) => surface,
        None => return,
    };

    let (out_tx, out_rx) = unbounded::<String>();
    let writer = std::thread::spawn(move || {
        while let Ok(line) = out_rx.recv() {
            if write_half.write_all(line.as_bytes()).is_err()
                || write_half.write_all(b"\n").is_err()
                || write_half.flush().is_err()
            {
                break;
            }
        }
    });
    writers.insert(connection_id, out_tx.clone());

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
                if handle_client_msg(msg, connection_id, owner_surface, &events, &out_tx).is_break()
                {
                    break;
                }
            }
        }
    }

    // NOTE: remove the table's Sender clone BEFORE dropping out_tx — only when the
    // last Sender is gone does out_rx.recv() return Disconnected, letting the writer
    // thread exit so writer.join() below doesn't hang.
    writers.remove(connection_id);
    drop(out_tx);
    let _ = writer.join();
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

/// Dispatches one parsed `ClientMsg`; relays the register reply through `out_tx`
/// so all writes go through the single writer thread. Returns `Break` when the
/// connection should be torn down.
fn handle_client_msg(
    msg: ClientMsg,
    connection_id: u64,
    owner_surface: Entity,
    events: &Sender<ControlEvent>,
    out_tx: &Sender<String>,
) -> ControlFlow<()> {
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
                return ControlFlow::Break(());
            }
            let reply = reply_rx
                .recv()
                .unwrap_or_else(|_| ServerMsg::err("internal"));
            let line = serde_json::to_string(&reply).expect("ServerMsg serializes infallibly");
            if out_tx.send(line).is_err() {
                return ControlFlow::Break(());
            }
        }
        ClientMsg::Unregister { handle } => {
            let _ = events.send(ControlEvent::Unregister {
                connection_id,
                handle,
            });
        }
        ClientMsg::Hello { .. } => {}
        ClientMsg::Reply {
            req_id,
            ok,
            value,
            error,
        } => {
            let _ = events.send(ControlEvent::Reply {
                req_id,
                ok,
                value,
                error,
                connection_id,
            });
        }
        ClientMsg::Emit {
            handle,
            event,
            payload,
        } => {
            let _ = events.send(ControlEvent::Emit {
                connection_id,
                handle,
                event,
                payload,
            });
        }
    }
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::ConnectionWriters;
    use std::time::{Duration, Instant};

    #[test]
    fn hello_then_register_emits_a_register_event_and_replies() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        let surface = Entity::from_bits(11);
        tokens.insert("tok", surface);

        let events = spawn_listener(&sock, tokens, ConnectionWriters::default()).unwrap();

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
        let events = spawn_listener(
            &sock,
            TokenRegistry::default(),
            ConnectionWriters::default(),
        )
        .unwrap();

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
        let events = spawn_listener(&sock, tokens, ConnectionWriters::default()).unwrap();

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

    #[test]
    fn client_reply_line_emits_a_reply_event() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        tokens.insert("tok", Entity::from_bits(1));
        let events = spawn_listener(&sock, tokens, ConnectionWriters::default()).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        writeln!(
            client,
            r#"{{"op":"reply","reqId":"g1","ok":true,"value":7}}"#
        )
        .unwrap();
        client.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(ControlEvent::Reply { req_id, ok, .. }) =
                events.recv_timeout(Duration::from_millis(50))
            {
                assert_eq!(req_id, "g1");
                assert!(ok);
                break;
            }
            assert!(Instant::now() < deadline, "no Reply event within 2s");
        }
    }

    #[test]
    fn server_push_reaches_a_hello_d_client() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        tokens.insert("tok", Entity::from_bits(1));
        let writers = ConnectionWriters::default();
        let _events = spawn_listener(&sock, tokens, writers.clone()).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        client.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if writers.send(
                1,
                r#"{"op":"call","handle":"H","reqId":"g0","method":"m","args":[]}"#.into(),
            ) {
                break;
            }
            assert!(Instant::now() < deadline, "writer never registered");
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut line = String::new();
        BufReader::new(client.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(
            line.contains(r#""op":"call""#) && line.contains(r#""reqId":"g0""#),
            "got {line}"
        );
    }
}
