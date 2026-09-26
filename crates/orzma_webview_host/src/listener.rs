//! The control socket's listener: an accept loop thread, plus a reader and
//! a writer thread per connection, turning each client line into a
//! `ControlEvent` for the host.

use crate::control_socket::{ConnectionId, ControlEvent};
use crate::error::WebviewHostResult;
use crate::host::ValidatedRegistration;
use crate::protocol::{ClientMsg, ServerMsg};
use crate::uds::{UnixListener, UnixStream};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::ops::ControlFlow;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How long the accept loop waits after a failed accept before it accepts
/// again.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Binds `sock_path` (replacing a stale socket file there), spawns the
/// accept loop, and returns the receiver of the events its connections
/// produce. The listener threads are detached and live as long as the
/// process.
///
/// # Errors
///
/// Returns an I/O error when the socket cannot be bound or the accept
/// thread cannot be started.
pub(crate) fn spawn_listener(sock_path: &Path) -> WebviewHostResult<Receiver<ControlEvent>> {
    let _ = std::fs::remove_file(sock_path);
    let listener = UnixListener::bind(sock_path)?;
    let (events_tx, events) = unbounded();
    thread::Builder::new()
        .name("orzma-control-accept".to_string())
        .spawn(move || accept_loop(listener, events_tx))?;
    Ok(events)
}

/// Accepts connections forever, serving each authenticated one on its own
/// thread under a fresh connection id. A failed accept is retried after
/// [`ACCEPT_RETRY_DELAY`], and only the first failure of a run is logged. A
/// connection whose thread cannot be started is closed.
fn accept_loop(listener: UnixListener, events: Sender<ControlEvent>) {
    let mut next_id: u64 = 1;
    let mut failing = false;
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                if !failing {
                    tracing::warn!(%error, "the control socket failed to accept a connection; retrying");
                }
                failing = true;
                thread::sleep(ACCEPT_RETRY_DELAY);
                continue;
            }
        };
        failing = false;
        if !accepts(&stream) {
            continue;
        }
        let connection = ConnectionId::new(next_id);
        next_id = next_id.wrapping_add(1);
        let events = events.clone();
        let spawned = thread::Builder::new()
            .name("orzma-control-reader".to_string())
            .spawn(move || serve_connection(stream, connection, events));
        if let Err(error) = spawned {
            tracing::warn!(%error, ?connection, "a control connection was closed because its thread could not start");
        }
    }
}

/// Serves one connection: requires a `hello` first and hands it to the host,
/// relays each later line as a `ControlEvent` until the peer closes, then
/// tears the connection down and reports `Disconnect`. A connection whose
/// writer thread cannot be started is closed before its `hello` reaches the
/// host.
fn serve_connection(stream: UnixStream, connection: ConnectionId, events: Sender<ControlEvent>) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(read_half);
    let Some(token) = read_hello(&mut lines) else {
        return;
    };
    let (out_tx, out_rx) = unbounded::<String>();
    let writer = match spawn_writer(write_half, out_rx) {
        Ok(writer) => writer,
        Err(error) => {
            tracing::warn!(%error, ?connection, "a control connection was closed because its writer thread could not start");
            return;
        }
    };
    let (answer_tx, answer_rx) = bounded::<bool>(1);
    let hello = ControlEvent::Hello {
        connection,
        token,
        writer: out_tx.clone(),
        reply: answer_tx,
    };
    if events.send(hello).is_err() || !answer_rx.recv().unwrap_or(false) {
        // NOTE: the host keeps no clone of `out_tx` for a `hello` it refused
        // or dropped unanswered; if it kept one, this join would never
        // return and the connection would never close.
        drop(out_tx);
        let _ = writer.join();
        return;
    }
    read_requests(&mut lines, connection, &events, &out_tx);
    // NOTE: the host holds a clone of `out_tx`, so dropping ours does not end
    // the writer thread; only the host dropping its clone when it applies
    // `Disconnect` does. Shut the socket down first (a writer parked in
    // `write_all` on a peer that stopped reading only wakes then), send
    // `Disconnect`, and only then join: joining before `Disconnect` would wait
    // forever on a writer parked in `recv`, and the host would never purge the
    // connection.
    let _ = stream.shutdown(Shutdown::Both);
    let _ = events.send(ControlEvent::Disconnect { connection });
    drop(out_tx);
    let _ = writer.join();
}

/// Starts the thread that writes each line `lines` yields to `stream`,
/// newline-terminated, until every sender of `lines` is gone or a write
/// fails.
///
/// # Errors
///
/// Returns the OS error when the thread cannot be started.
fn spawn_writer(mut stream: UnixStream, lines: Receiver<String>) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("orzma-control-writer".to_string())
        .spawn(move || {
            while let Ok(line) = lines.recv() {
                if stream.write_all(line.as_bytes()).is_err()
                    || stream.write_all(b"\n").is_err()
                    || stream.flush().is_err()
                {
                    break;
                }
            }
        })
}

/// Reads the first line and returns its token when it is a `hello`.
fn read_hello(lines: &mut BufReader<UnixStream>) -> Option<String> {
    let mut buf = String::new();
    if matches!(lines.read_line(&mut buf), Ok(0) | Err(_)) {
        return None;
    }
    match serde_json::from_str::<ClientMsg>(buf.trim_end_matches(['\n', '\r'])) {
        Ok(ClientMsg::Hello { token }) => Some(token),
        _ => None,
    }
}

/// Relays request lines until the peer closes, a line cannot be read, or
/// the host or the writer is gone. A line that does not parse is skipped.
fn read_requests(
    lines: &mut BufReader<UnixStream>,
    connection: ConnectionId,
    events: &Sender<ControlEvent>,
    out_tx: &Sender<String>,
) {
    let mut buf = String::new();
    loop {
        buf.clear();
        match lines.read_line(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let line = buf.trim_end_matches(['\n', '\r']);
                let Ok(msg) = serde_json::from_str::<ClientMsg>(line) else {
                    continue;
                };
                if relay(msg, connection, events, out_tx).is_break() {
                    return;
                }
            }
        }
    }
}

/// Hands one parsed line to the host. Returns `Break` when the host or the
/// writer is gone.
fn relay(
    msg: ClientMsg,
    connection: ConnectionId,
    events: &Sender<ControlEvent>,
    out_tx: &Sender<String>,
) -> ControlFlow<()> {
    let event = match msg {
        ClientMsg::Hello { .. } => return ControlFlow::Continue(()),
        ClientMsg::Register(kind) => {
            let registration = ValidatedRegistration::try_from(kind);
            return request(events, out_tx, |reply| ControlEvent::Register {
                connection,
                registration,
                reply,
            });
        }
        ClientMsg::NewInstance { handle } => {
            return request(events, out_tx, |reply| ControlEvent::NewInstance {
                connection,
                handle,
                reply,
            });
        }
        ClientMsg::Unregister { handle } => ControlEvent::Unregister { connection, handle },
        ClientMsg::Reply {
            req_id,
            ok,
            value,
            error,
        } => ControlEvent::Reply {
            connection,
            req_id,
            ok,
            value,
            error,
        },
        ClientMsg::Emit {
            handle,
            event,
            payload,
        } => ControlEvent::Emit {
            connection,
            handle,
            event,
            payload,
        },
        ClientMsg::Focus { instance } => ControlEvent::Focus {
            connection,
            instance,
        },
        ClientMsg::Navigate { instance, action } => ControlEvent::Navigate {
            connection,
            instance,
            action,
        },
        ClientMsg::Mount {
            instance,
            row,
            col,
            rows,
            cols,
        } => ControlEvent::Mount {
            connection,
            instance,
            row,
            col,
            rows,
            cols,
        },
        ClientMsg::Unmount { instance } => ControlEvent::Unmount {
            connection,
            instance,
        },
        ClientMsg::SetForwardKeys { handle, keys } => ControlEvent::SetForwardKeys {
            connection,
            handle,
            keys,
        },
    };
    if events.send(event).is_err() {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
}

/// Sends the request `build` makes with a fresh reply channel, waits for
/// the host's reply, and writes it to the connection; a host that drops the
/// reply channel is answered with `internal`. Returns `Break` when the host
/// or the writer is gone or the reply cannot be serialized.
fn request(
    events: &Sender<ControlEvent>,
    out_tx: &Sender<String>,
    build: impl FnOnce(Sender<ServerMsg>) -> ControlEvent,
) -> ControlFlow<()> {
    // NOTE: blocking on the reply before the reader reads the next line is
    // what keeps one connection's replies in request order; the SDK matches
    // them to its pending requests by position.
    let (reply_tx, reply_rx) = bounded::<ServerMsg>(1);
    if events.send(build(reply_tx)).is_err() {
        return ControlFlow::Break(());
    }
    let reply = reply_rx
        .recv()
        .unwrap_or_else(|_| ServerMsg::err("internal"));
    let line = match serde_json::to_string(&reply) {
        Ok(line) => line,
        Err(error) => {
            tracing::warn!(%error, "a control reply failed to serialize");
            return ControlFlow::Break(());
        }
    };
    if out_tx.send(line).is_err() {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
}

/// Whether a freshly accepted connection may proceed to the handshake.
///
/// On Unix the peer's UID must equal orzma's own. On Windows it is always
/// true: the socket directory's DACL already limits it to the current user.
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

/// Returns the connecting peer's UID via `getpeereid` (Apple/BSD), or `None`
/// on error.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boundary::{ForwardChord, HandleId};
    use crate::error::RegisterError;
    use crate::protocol::NavAction;
    use orzma_vt::prelude::InstanceId;
    use std::io::{ErrorKind, Read};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// How long a test client's read waits before it fails.
    const READ_TIMEOUT: Duration = Duration::from_secs(2);

    /// A listener on a socket in a fresh temp directory.
    fn listen() -> (TempDir, PathBuf, Receiver<ControlEvent>) {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let events = spawn_listener(&sock).expect("the socket binds");
        (dir, sock, events)
    }

    /// A client of `sock` whose reads fail after `READ_TIMEOUT` instead of
    /// blocking.
    fn connect(sock: &Path) -> UnixStream {
        let client = UnixStream::connect(sock).unwrap();
        client.set_read_timeout(Some(READ_TIMEOUT)).unwrap();
        client
    }

    fn next_event(events: &Receiver<ControlEvent>) -> ControlEvent {
        events
            .recv_timeout(Duration::from_secs(2))
            .expect("an event within 2s")
    }

    fn send_line(client: &mut UnixStream, line: &str) {
        writeln!(client, "{line}").unwrap();
        client.flush().unwrap();
    }

    /// Connects, sends a `hello` with `tok`, and answers it with `accept`,
    /// returning the writer channel the listener handed over.
    fn hello(
        events: &Receiver<ControlEvent>,
        client: &mut UnixStream,
        accept: bool,
    ) -> Sender<String> {
        send_line(client, r#"{"op":"hello","token":"tok"}"#);
        match next_event(events) {
            ControlEvent::Hello {
                token,
                writer,
                reply,
                ..
            } => {
                assert_eq!(token, "tok");
                reply.send(accept).unwrap();
                writer
            }
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    fn read_line(client: &UnixStream) -> String {
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        line
    }

    /// Reads `client` until the listener closes it and returns what it read.
    /// A reset counts as a close; a read that times out fails the test.
    fn read_until_closed(client: &mut UnixStream) -> Vec<u8> {
        let mut rest = Vec::new();
        if let Err(error) = client.read_to_end(&mut rest) {
            assert!(
                !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
                "the listener kept the connection open"
            );
        }
        rest
    }

    /// Asserts that a `hello` reaches the host with the connection's writer,
    /// and that a line the host sends through it reaches the program.
    ///
    /// Case: a program connects, and the host later calls it on behalf of
    /// its page.
    #[test]
    fn a_hello_hands_the_host_the_connections_writer() {
        let (_dir, sock, events) = listen();
        let mut client = connect(&sock);
        let writer = hello(&events, &mut client, true);
        writer
            .send(r#"{"op":"call","handle":"H","reqId":"0","method":"m","params":null}"#.into())
            .unwrap();
        let line = read_line(&client);
        assert!(line.contains(r#""reqId":"0""#), "got {line}");
    }

    /// Asserts that a refused `hello` closes the connection and produces no
    /// further event.
    ///
    /// Case: a stray process guesses a token.
    #[test]
    fn a_refused_hello_closes_the_connection() {
        let (_dir, sock, events) = listen();
        let mut client = connect(&sock);
        drop(hello(&events, &mut client, false));
        let _ = writeln!(client, r#"{{"op":"unregister","handle":"h"}}"#);
        assert!(events.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(read_until_closed(&mut client).is_empty());
    }

    /// Asserts that a first line other than `hello` closes the connection
    /// without any event.
    ///
    /// Case: a program built against a broken SDK starts with `register`.
    #[test]
    fn a_first_line_other_than_hello_produces_no_event() {
        let (_dir, sock, events) = listen();
        let mut client = connect(&sock);
        send_line(
            &mut client,
            r#"{"op":"register","kind":"inline","html":"x"}"#,
        );
        assert!(events.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(read_until_closed(&mut client).is_empty());
    }

    /// Asserts that a `register` reaches the host already validated and that
    /// the host's reply is written back, and that an invalid payload arrives
    /// as its validation failure.
    ///
    /// Case: a program registers an inline page, then a relative root
    /// directory.
    #[test]
    fn a_register_is_validated_and_its_reply_relayed() {
        let (_dir, sock, events) = listen();
        let mut client = connect(&sock);
        let _writer = hello(&events, &mut client, true);
        send_line(
            &mut client,
            r#"{"op":"register","kind":"inline","html":"<h1>x</h1>"}"#,
        );
        let ControlEvent::Register {
            registration,
            reply,
            ..
        } = next_event(&events)
        else {
            panic!("expected Register");
        };
        assert!(registration.is_ok());
        reply
            .send(ServerMsg::registered("h1", InstanceId(1)))
            .unwrap();
        assert!(read_line(&client).contains(r#""handle":"h1""#));
        send_line(
            &mut client,
            r#"{"op":"register","kind":"dir","root":"relative","entry":"index.html"}"#,
        );
        let ControlEvent::Register {
            registration,
            reply,
            ..
        } = next_event(&events)
        else {
            panic!("expected Register");
        };
        assert_eq!(registration.err(), Some(RegisterError::InvalidRoot));
        reply.send(ServerMsg::err("invalid_root")).unwrap();
        assert!(read_line(&client).contains("invalid_root"));
    }

    /// Asserts that every request line after the `hello` becomes its event,
    /// in order, carrying the connection that sent it.
    ///
    /// Case: a program replies to a call, emits, focuses, navigates, mounts,
    /// unmounts, replaces its forward keys, and unregisters.
    #[test]
    fn each_request_line_becomes_its_event() {
        let (_dir, sock, events) = listen();
        let mut client = connect(&sock);
        let _writer = hello(&events, &mut client, true);
        for line in [
            r#"{"op":"reply","reqId":"g1","ok":true,"value":7}"#,
            r#"{"op":"emit","handle":"H","event":"tick","payload":{"n":1}}"#,
            r#"{"op":"focus","instance":"3f5a"}"#,
            r#"{"op":"navigate","instance":"3f5a","action":"reload"}"#,
            r#"{"op":"mount","instance":"3f5a","row":2,"col":3,"rows":12,"cols":48}"#,
            r#"{"op":"unmount","instance":"3f5a"}"#,
            r#"{"op":"set_forward_keys","handle":"H","keys":[{"mods":[],"key":"esc"}]}"#,
            r#"{"op":"unregister","handle":"H"}"#,
        ] {
            send_line(&mut client, line);
        }
        let connection = ConnectionId::new(1);
        assert!(
            matches!(next_event(&events), ControlEvent::Reply { connection: c, ref req_id, ok: true, .. } if c == connection && req_id == "g1")
        );
        assert!(
            matches!(next_event(&events), ControlEvent::Emit { connection: c, ref event, .. } if c == connection && event == "tick")
        );
        assert!(
            matches!(next_event(&events), ControlEvent::Focus { connection: c, instance: Some(ref i) } if c == connection && i == "3f5a")
        );
        assert!(matches!(
            next_event(&events),
            ControlEvent::Navigate {
                connection: c,
                action: NavAction::Reload,
                ..
            } if c == connection
        ));
        assert!(matches!(
            next_event(&events),
            ControlEvent::Mount {
                connection: c,
                row: 2,
                col: 3,
                rows: 12,
                cols: 48,
                ..
            } if c == connection
        ));
        assert!(
            matches!(next_event(&events), ControlEvent::Unmount { connection: c, ref instance } if c == connection && instance == "3f5a")
        );
        assert!(matches!(
            next_event(&events),
            ControlEvent::SetForwardKeys { connection: c, ref keys, .. } if c == connection && *keys == vec![ForwardChord::new(vec![], "esc")]
        ));
        assert!(matches!(
            next_event(&events),
            ControlEvent::Unregister { connection: c, ref handle } if c == connection && *handle == HandleId::from("H")
        ));
    }

    /// Asserts that closing the connection delivers `Disconnect` while the
    /// host still holds the connection's writer channel, and that the writer
    /// thread then stops, so the host's pushes fail instead of queueing.
    ///
    /// Case: a program exits, and the host has not dropped anything for it
    /// yet.
    #[test]
    fn a_disconnect_arrives_while_the_host_holds_the_writer() {
        let (_dir, sock, events) = listen();
        let mut client = connect(&sock);
        let writer = hello(&events, &mut client, true);
        drop(client);
        assert!(matches!(
            next_event(&events),
            ControlEvent::Disconnect { .. }
        ));
        let deadline = Instant::now() + Duration::from_secs(5);
        while writer.send("probe".into()).is_ok() {
            assert!(
                Instant::now() < deadline,
                "the writer thread outlived its connection"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Asserts that `Disconnect` arrives when the peer stops sending, even
    /// while the writer thread is blocked on that peer because it never
    /// reads.
    ///
    /// Case: a program stops reading its socket while its page floods it
    /// with events, then closes its sending side and hangs without exiting.
    #[test]
    fn a_disconnect_arrives_while_the_writer_is_blocked() {
        let (_dir, sock, events) = listen();
        let mut client = connect(&sock);
        let writer = hello(&events, &mut client, true);
        let chunk = "x".repeat(64 * 1024);
        for _ in 0..64 {
            writer.send(chunk.clone()).unwrap();
        }
        thread::sleep(Duration::from_millis(100));
        client.shutdown(Shutdown::Write).unwrap();
        assert!(matches!(
            next_event(&events),
            ControlEvent::Disconnect { .. }
        ));
        drop(client);
        drop(writer);
    }

    /// Asserts that the bound socket file inherits the runtime directory's
    /// current-user restriction.
    ///
    /// Case: orzma binds its control socket in its runtime directory under
    /// `%TEMP%` on Windows.
    #[cfg(windows)]
    #[test]
    fn the_bound_socket_file_is_private_to_the_current_user() {
        use crate::control_socket::ControlSocket;
        use crate::private_dir::{canonical_sddl, current_user_sid, security_descriptor_sddl};
        let dir = tempfile::tempdir().unwrap();
        let socket = ControlSocket::bind(dir.path(), 4244).expect("the socket binds");
        let sddl = security_descriptor_sddl(socket.sock_path()).unwrap();
        let sid = current_user_sid().unwrap();
        let expected = canonical_sddl(&format!("D:(A;;FA;;;{sid})")).unwrap();
        assert_eq!(sddl, expected);
    }
}
