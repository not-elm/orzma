//! Tokio-free control-plane listener: an accept loop thread plus one reader
//! thread and one writer thread per connection, turning each client line into
//! a `ControlEvent` for the ECS apply system.

use crate::control_plane::ConnectionWriters;
use crate::control_plane::HandleId;
use crate::control_plane::TokenRegistry;
use crate::control_plane::protocol::{ClientMsg, NavAction, RegisterKind, ServerMsg};
use bevy::prelude::Entity;
use bevy_orzma_webview_host::uds::{UnixListener, UnixStream};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::ops::ControlFlow;
#[cfg(unix)]
use std::os::fd::AsRawFd;

/// An event the listener emits to the ECS apply system.
pub(crate) enum ControlEvent {
    /// A `register` from a hello'd connection; the minted handle comes back
    /// on `reply`.
    Register {
        /// The connection the minted registration is owned by.
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
        /// The connection that sent it. Ownership is not checked here.
        connection_id: u64,
        /// The handle to release.
        handle: HandleId,
    },
    /// A `new_instance` naming a handle; the minted instance comes back on
    /// `reply`.
    NewInstance {
        /// The connection that sent it. Ownership is not checked here.
        connection_id: u64,
        /// The handle to mint an additional instance for.
        handle: HandleId,
        /// Where the apply system returns the `ServerMsg` reply.
        reply: Sender<ServerMsg>,
    },
    /// A connection closed; purge all its handles.
    Disconnect {
        /// Connection id.
        connection_id: u64,
    },
    /// A program's reply to an orzma-initiated back-channel `call`.
    Reply {
        /// The global reqId the apply system correlates.
        req_id: String,
        /// Whether the call succeeded.
        ok: bool,
        /// The success value; `null` when `ok` is false.
        value: Value,
        /// The error message when `ok` is false.
        error: Option<String>,
        /// The connection that sent the reply. Ownership is not checked here.
        connection_id: u64,
    },
    /// A program-initiated push to a handle's webviews.
    Emit {
        /// The connection that sent it. Ownership is not checked here.
        connection_id: u64,
        /// The target handle.
        handle: HandleId,
        /// The event name.
        event: String,
        /// The event payload.
        payload: Value,
    },
    /// An app-owned focus set/clear for the connection's surface.
    SetFocus {
        /// The connection that sent it. Ownership is not checked here.
        connection_id: u64,
        /// The surface the connection's token resolved to.
        owner_surface: Entity,
        /// The instance to focus, or `None` to blur.
        instance: Option<String>,
    },
    /// An app-initiated in-place navigation of one mounted placement.
    Navigate {
        /// The connection that sent it. Ownership is not checked here.
        connection_id: u64,
        /// The surface the connection's token resolved to.
        owner_surface: Entity,
        /// The target instance.
        instance: String,
        /// What to do.
        action: NavAction,
    },
    /// A socket `mount` naming an instance.
    Mount {
        /// The connection that sent it. Ownership is not checked here.
        connection_id: u64,
        /// The surface the connection's token resolved to.
        owner_surface: Entity,
        /// The target instance.
        instance: String,
        /// 0-based visible row of the rect's top edge.
        row: u16,
        /// 0-based column of the rect's left edge.
        col: u16,
        /// Rect height in cells.
        rows: u16,
        /// Rect width in cells.
        cols: u16,
    },
    /// A socket `unmount` naming an instance.
    Unmount {
        /// The connection that sent it. Ownership is not checked here.
        connection_id: u64,
        /// The surface the connection's token resolved to.
        owner_surface: Entity,
        /// The target instance.
        instance: String,
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
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if !accepts(&stream) {
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

/// Whether a freshly accepted connection may proceed to the handshake.
///
/// On Unix the peer's UID must equal orzma's own. On Windows it is always
/// true.
#[cfg(unix)]
fn accepts(stream: &UnixStream) -> bool {
    // SAFETY: `getuid` has no preconditions and cannot fail.
    let own_uid = unsafe { libc::getuid() } as u32;
    peer_uid(stream) == Some(own_uid)
}

#[cfg(windows)]
fn accepts(_stream: &UnixStream) -> bool {
    true
}

/// Returns the connecting peer's UID via `getpeereid` (Apple/BSD), or `None` on
/// error.
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

/// Reads one connection: it requires a valid `hello`, spawns a writer thread
/// for all outbound lines (replies and server pushes) and registers it in
/// `writers`, forwards each request line as a `ControlEvent`, then emits
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
    // NOTE: shut the shared socket fd down before joining. Dropping out_tx unblocks
    // a writer parked in recv(), but a writer parked in write_all() (peer stopped
    // reading with a full send buffer) only unblocks when the fd is shut down —
    // otherwise writer.join() hangs forever and this connection's Disconnect (and
    // its registry / in-flight cleanup) is never delivered.
    let _ = stream.shutdown(std::net::Shutdown::Both);
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

/// Dispatches one parsed `ClientMsg`, relaying the register reply through
/// `out_tx`. Returns `Break` when the connection should be torn down.
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
        // NOTE: blocking on reply_rx.recv() before the reader reads the next
        // line is what keeps one connection's replies in request order; the
        // SDK matches them to its pending requests by position.
        ClientMsg::NewInstance { handle } => {
            let (reply_tx, reply_rx) = bounded::<ServerMsg>(1);
            if events
                .send(ControlEvent::NewInstance {
                    connection_id,
                    handle,
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
        ClientMsg::Focus { instance } => {
            let _ = events.send(ControlEvent::SetFocus {
                connection_id,
                owner_surface,
                instance,
            });
        }
        ClientMsg::Navigate { instance, action } => {
            let _ = events.send(ControlEvent::Navigate {
                connection_id,
                owner_surface,
                instance,
                action,
            });
        }
        ClientMsg::Mount {
            instance,
            row,
            col,
            rows,
            cols,
        } => {
            let _ = events.send(ControlEvent::Mount {
                connection_id,
                owner_surface,
                instance,
                row,
                col,
                rows,
                cols,
            });
        }
        ClientMsg::Unmount { instance } => {
            let _ = events.send(ControlEvent::Unmount {
                connection_id,
                owner_surface,
                instance,
            });
        }
    }
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::ConnectionWriters;
    use orzma_vt::prelude::InstanceId;
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
        reply
            .send(ServerMsg::registered("HANDLE1", InstanceId(1)))
            .unwrap();

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
    fn client_emit_line_emits_an_emit_event() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        tokens.insert("tok", Entity::from_bits(1));
        let events = spawn_listener(&sock, tokens, ConnectionWriters::default()).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        writeln!(
            client,
            r#"{{"op":"emit","handle":"H","event":"tick","payload":{{"n":1}}}}"#
        )
        .unwrap();
        client.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(ControlEvent::Emit { handle, event, .. }) =
                events.recv_timeout(Duration::from_millis(50))
            {
                assert_eq!(handle, HandleId::from("H"));
                assert_eq!(event, "tick");
                break;
            }
            assert!(Instant::now() < deadline, "no Emit event within 2s");
        }
    }

    #[test]
    fn client_focus_line_emits_a_set_focus_event() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        let surface = Entity::from_bits(7);
        tokens.insert("tok", surface);
        let events = spawn_listener(&sock, tokens, ConnectionWriters::default()).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        writeln!(client, r#"{{"op":"focus","instance":"3f5a"}}"#).unwrap();
        client.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(ControlEvent::SetFocus {
                owner_surface,
                instance,
                ..
            }) = events.recv_timeout(Duration::from_millis(50))
            {
                assert_eq!(owner_surface, surface);
                assert_eq!(instance.as_deref(), Some("3f5a"));
                break;
            }
            assert!(Instant::now() < deadline, "no SetFocus within 2s");
        }
    }

    /// Asserts that a `mount` line and a following `unmount` line from a
    /// hello'd client become a `ControlEvent::Mount` and a
    /// `ControlEvent::Unmount` bound to the token's surface, the mount
    /// carrying the cell and size verbatim.
    ///
    /// Case: orzmd in a Windows pane sends its first socket `mount`.
    #[test]
    fn client_mount_and_unmount_lines_emit_their_events() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        let surface = Entity::from_bits(7);
        tokens.insert("tok", surface);
        let events = spawn_listener(&sock, tokens, ConnectionWriters::default()).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        writeln!(
            client,
            r#"{{"op":"mount","instance":"3f5a","row":2,"col":3,"rows":12,"cols":48}}"#
        )
        .unwrap();
        writeln!(client, r#"{{"op":"unmount","instance":"3f5a"}}"#).unwrap();
        client.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_mount = false;
        loop {
            match events.recv_timeout(Duration::from_millis(50)) {
                Ok(ControlEvent::Mount {
                    owner_surface,
                    instance,
                    row,
                    col,
                    rows,
                    cols,
                    ..
                }) => {
                    assert_eq!(owner_surface, surface);
                    assert_eq!(instance, "3f5a");
                    assert_eq!((row, col, rows, cols), (2, 3, 12, 48));
                    saw_mount = true;
                }
                Ok(ControlEvent::Unmount {
                    owner_surface,
                    instance,
                    ..
                }) => {
                    assert!(saw_mount, "the mount precedes the unmount");
                    assert_eq!(owner_surface, surface);
                    assert_eq!(instance, "3f5a");
                    break;
                }
                _ => {}
            }
            assert!(Instant::now() < deadline, "no Mount + Unmount within 2s");
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
                r#"{"op":"call","handle":"H","reqId":"g0","method":"m","params":null}"#.into(),
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

    #[test]
    fn navigate_msg_emits_navigate_event() {
        let (ev_tx, ev_rx) = unbounded::<ControlEvent>();
        let (out_tx, _out_rx) = unbounded::<String>();
        let surface = Entity::from_bits(1);

        let flow = handle_client_msg(
            ClientMsg::Navigate {
                instance: "3f5a".into(),
                action: NavAction::Reload,
            },
            7,
            surface,
            &ev_tx,
            &out_tx,
        );

        assert!(matches!(flow, ControlFlow::Continue(())));
        match ev_rx.try_recv().expect("a navigate event") {
            ControlEvent::Navigate {
                connection_id,
                owner_surface,
                instance,
                action,
            } => {
                assert_eq!(connection_id, 7);
                assert_eq!(owner_surface, surface);
                assert_eq!(instance, "3f5a");
                assert_eq!(action, NavAction::Reload);
            }
            _ => panic!("expected Navigate"),
        }
    }

    /// Asserts that the bound socket file inherits the runtime
    /// directory's current-user restriction.
    ///
    /// Case: orzma binds its control socket in its runtime directory under
    /// `%TEMP%` on Windows.
    #[cfg(windows)]
    #[test]
    fn the_bound_socket_file_is_private_to_the_current_user() {
        use bevy_orzma_webview_host::host::RuntimeRoot;
        use bevy_orzma_webview_host::private_dir::{
            canonical_sddl, current_user_sid, security_descriptor_sddl,
        };
        let dir = tempfile::tempdir().unwrap();
        let root = RuntimeRoot::resolve_in(dir.path(), 4244, "control").unwrap();
        let sock = root.socket_path("control");
        let _events = spawn_listener(
            &sock,
            TokenRegistry::default(),
            ConnectionWriters::default(),
        )
        .unwrap();
        let sddl = security_descriptor_sddl(&sock).unwrap();
        let sid = current_user_sid().unwrap();
        let expected = canonical_sddl(&format!("D:(A;;FA;;;{sid})")).unwrap();
        assert_eq!(
            sddl, expected,
            "the socket file must carry exactly the inherited current-user ACE"
        );
    }
}
