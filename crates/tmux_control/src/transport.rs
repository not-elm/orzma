//! I/O transport: owns a `tmux -CC` process and pumps its output through a
//! [`crate::ProtocolClient`].

use crate::error::{TmuxError, TmuxResult};
use crate::protocol::{ClientEvent, CommandId, ProtocolClient};
use crate::session::{LIST_FORMAT, SessionInfo};
use crossbeam_channel::{Receiver, Sender, unbounded};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A Layer-2 event: a Layer-1 protocol event, or transport termination.
#[derive(Debug)]
pub enum TransportEvent {
    /// A protocol event from Layer 1 (kept I/O-agnostic).
    Protocol(ClientEvent),
    /// The transport closed (process exit / EOF / reader I/O error).
    Closed {
        /// Human-readable reason (EOF marker or the I/O error text).
        reason: String,
    },
}

/// Which tmux server socket to talk to.
#[derive(Clone)]
enum Socket {
    Default,
    Name(String),
    Path(String),
}

/// A reusable handle to a tmux server on a given socket (config only; holds no
/// connection or threads). Lists sessions and opens control connections.
#[derive(Clone)]
pub struct TmuxServer {
    program: String,
    socket: Socket,
}

impl TmuxServer {
    /// Returns a server targeting the default socket via the `tmux` binary on `PATH`.
    pub fn new() -> Self {
        Self {
            program: "tmux".to_string(),
            socket: Socket::Default,
        }
    }

    /// Overrides the tmux binary path.
    pub fn program(mut self, path: &str) -> Self {
        self.program = path.to_string();
        self
    }

    /// Targets the named server socket (`-L`). Overrides any prior socket choice.
    pub fn socket_name(mut self, name: &str) -> Self {
        self.socket = Socket::Name(name.to_string());
        self
    }

    /// Targets the server socket path (`-S`). Overrides any prior socket choice.
    pub fn socket_path(mut self, path: &str) -> Self {
        self.socket = Socket::Path(path.to_string());
        self
    }

    /// Opens a control connection by attaching to `name`
    /// (`tmux -CC attach-session -t name`).
    pub fn attach(&self, name: &str) -> TmuxResult<TmuxClient> {
        self.spawn(&["attach-session", "-t", name])
    }

    /// Opens a control connection on a fresh session (`tmux -CC new-session`).
    pub fn new_session(&self) -> TmuxResult<TmuxClient> {
        self.spawn(&["new-session"])
    }

    /// Lists attachable sessions (`tmux [..] list-sessions -F ..`, plain pipe, no
    /// control mode). Returns `Ok(vec![])` when no server is running.
    pub fn list_sessions(&self) -> TmuxResult<Vec<SessionInfo>> {
        let output = std::process::Command::new(&self.program)
            .args(self.list_sessions_argv())
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(TmuxError::Spawn)?;
        classify_list_result(output.status.success(), &output.stdout, &output.stderr)
    }

    /// The argv (after the program) for the list-sessions query, for callers that
    /// own their own I/O (e.g. running it off the Bevy main thread) and then call
    /// [`SessionInfo::parse_list`].
    pub fn list_sessions_argv(&self) -> Vec<String> {
        let mut argv = self.socket_args();
        argv.push("list-sessions".to_string());
        argv.push("-F".to_string());
        argv.push(LIST_FORMAT.to_string());
        argv
    }

    fn socket_args(&self) -> Vec<String> {
        match &self.socket {
            Socket::Default => Vec::new(),
            Socket::Name(name) => vec!["-L".to_string(), name.clone()],
            Socket::Path(path) => vec!["-S".to_string(), path.clone()],
        }
    }

    fn connect_argv(&self, subcommand: &[&str]) -> Vec<String> {
        let mut argv = self.socket_args();
        argv.push("-CC".to_string());
        argv.extend(subcommand.iter().map(|s| s.to_string()));
        argv
    }

    fn spawn(&self, subcommand: &[&str]) -> TmuxResult<TmuxClient> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(spawn_err)?;

        // NOTE: disable PTY echo before tmux starts. Otherwise our command
        // writes are echoed back into the read stream during tmux's startup
        // window (before tmux puts its tty in raw mode), and those echoed
        // non-`%` lines corrupt the control-mode parse and close the stream.
        #[cfg(unix)]
        if let Some(fd) = pair.master.as_raw_fd() {
            disable_pty_echo(fd);
        }

        let mut cmd = CommandBuilder::new(&self.program);
        for arg in self.connect_argv(subcommand) {
            cmd.arg(arg);
        }
        let child = pair.slave.spawn_command(cmd).map_err(spawn_err)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().map_err(spawn_err)?;
        let writer = pair.master.take_writer().map_err(spawn_err)?;

        let mut protocol = ProtocolClient::new();
        protocol.register_pending();
        let protocol = Arc::new(Mutex::new(protocol));
        let writer = Arc::new(Mutex::new(writer));

        let (tx, rx) = unbounded();
        let pump_protocol = protocol.clone();
        let reader_thread = std::thread::spawn(move || pump(reader, pump_protocol, tx));

        Ok(TmuxClient {
            events: rx,
            handle: TmuxHandle { protocol, writer },
            child,
            _master: pair.master,
            reader_thread: Some(reader_thread),
        })
    }
}

impl Default for TmuxServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Cloneable handle for sending commands to tmux.
///
/// The protocol state and the writer have independent mutexes. `send` takes the
/// writer lock while still holding the protocol lock (so writes happen in
/// registration order), then releases the protocol lock before the blocking
/// write so the reader thread — which only needs the protocol lock — can keep
/// draining the PTY. Holding one lock across the write would deadlock when the
/// PTY buffer fills.
#[derive(Clone)]
pub struct TmuxHandle {
    protocol: Arc<Mutex<ProtocolClient>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl TmuxHandle {
    /// Encodes and writes `command` to tmux, returning its [`CommandId`].
    ///
    /// On write failure the pending registration is rolled back so later
    /// replies stay correctly correlated.
    pub fn send(&self, command: &str) -> TmuxResult<CommandId> {
        let mut protocol = self.protocol.lock().expect("tmux protocol mutex poisoned");
        let id = protocol.send(command)?;
        let bytes = protocol.take_outgoing();
        let mut writer = self.writer.lock().expect("tmux writer mutex poisoned");
        drop(protocol);
        match writer.write_all(&bytes).and_then(|()| writer.flush()) {
            Ok(()) => Ok(id),
            Err(e) => {
                drop(writer);
                self.protocol
                    .lock()
                    .expect("tmux protocol mutex poisoned")
                    .rollback_pending(id);
                Err(TmuxError::Io(e))
            }
        }
    }
}

/// Owns a `tmux -CC` process and pumps its output through a [`ProtocolClient`].
pub struct TmuxClient {
    events: Receiver<TransportEvent>,
    handle: TmuxHandle,
    child: Box<dyn Child + Send + Sync>,
    _master: Box<dyn MasterPty + Send>,
    reader_thread: Option<JoinHandle<()>>,
}

impl TmuxClient {
    /// Returns the channel of transport events (protocol events + close).
    pub fn events(&self) -> &Receiver<TransportEvent> {
        &self.events
    }

    /// Returns a cloneable send handle.
    pub fn handle(&self) -> TmuxHandle {
        self.handle.clone()
    }

    /// Signals tmux to exit (idempotent).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for TmuxClient {
    fn drop(&mut self) {
        // NOTE: kill() is idempotent (already-exited ok); we kill so the reader's
        // blocking read() returns EOF and the pump thread exits before we join it.
        let _ = self.child.kill();
        if let Some(thread) = self.reader_thread.take() {
            let _ = thread.join();
        }
    }
}

fn spawn_err(e: impl std::fmt::Display) -> TmuxError {
    TmuxError::Spawn(std::io::Error::other(e.to_string()))
}

fn classify_list_result(ok: bool, stdout: &[u8], stderr: &[u8]) -> TmuxResult<Vec<SessionInfo>> {
    if ok {
        return SessionInfo::parse_list(stdout);
    }
    let stderr = String::from_utf8_lossy(stderr);
    let no_server = stdout.is_empty()
        && (stderr.contains("no server running")
            || (stderr.contains("error connecting")
                && stderr.contains("No such file or directory")));
    if no_server {
        Ok(Vec::new())
    } else {
        let message = stderr.trim();
        let message = if message.is_empty() {
            "tmux list-sessions failed"
        } else {
            message
        };
        Err(TmuxError::Spawn(std::io::Error::other(message.to_string())))
    }
}

#[cfg(unix)]
fn disable_pty_echo(fd: std::os::unix::io::RawFd) {
    // SAFETY: `fd` is the live PTY master fd owned by the `MasterPty` kept in
    // `TmuxClient` for the duration of the call; tcgetattr fully initializes the
    // termios before any field is read. Failures are non-fatal (tmux sets raw
    // mode itself).
    unsafe {
        let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
        if libc::tcgetattr(fd, termios.as_mut_ptr()) == 0 {
            let mut termios = termios.assume_init();
            termios.c_lflag &= !(libc::ECHO | libc::ICANON);
            let _ = libc::tcsetattr(fd, libc::TCSANOW, &termios);
        }
    }
}

fn pump<R: Read>(
    mut reader: R,
    protocol: Arc<Mutex<ProtocolClient>>,
    sender: Sender<TransportEvent>,
) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                let _ = sender.send(TransportEvent::Closed {
                    reason: "eof".to_string(),
                });
                return;
            }
            Ok(n) => {
                let result = {
                    let mut guard = protocol.lock().expect("tmux protocol mutex poisoned");
                    guard.feed(&buf[..n])
                };
                match result {
                    Ok(events) => {
                        for event in events {
                            if sender.send(TransportEvent::Protocol(event)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = sender.send(TransportEvent::Closed {
                            reason: e.to_string(),
                        });
                        return;
                    }
                }
            }
            Err(e) => {
                let _ = sender.send(TransportEvent::Closed {
                    reason: e.to_string(),
                });
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ControlEvent;
    use crate::SessionId;
    use std::collections::VecDeque;
    use tmux_control_parser::WindowId;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    struct ScriptedReader {
        chunks: VecDeque<Vec<u8>>,
    }

    impl Read for ScriptedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            // NOTE: assumes each scripted chunk fits in `buf` (>= 4096 in pump);
            // test chunks are small, so no chunk is ever split here.
            match self.chunks.pop_front() {
                Some(chunk) => {
                    let n = chunk.len().min(buf.len());
                    buf[..n].copy_from_slice(&chunk[..n]);
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }

    #[derive(Clone, Default)]
    struct CaptureWriter {
        data: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.data.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct ErroringWriter;

    impl Write for ErroringWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn make_handle(writer: Box<dyn Write + Send>) -> (Arc<Mutex<ProtocolClient>>, TmuxHandle) {
        let protocol = Arc::new(Mutex::new(ProtocolClient::new()));
        let handle = TmuxHandle {
            protocol: protocol.clone(),
            writer: Arc::new(Mutex::new(writer)),
        };
        (protocol, handle)
    }

    fn protocol_events(rx: &Receiver<TransportEvent>) -> Vec<ClientEvent> {
        rx.try_iter()
            .filter_map(|e| match e {
                TransportEvent::Protocol(p) => Some(p),
                TransportEvent::Closed { .. } => None,
            })
            .collect()
    }

    #[test]
    fn connect_argv_new_session() {
        assert_eq!(
            TmuxServer::new().connect_argv(&["new-session"]),
            argv(&["-CC", "new-session"])
        );
    }

    #[test]
    fn connect_argv_attach() {
        assert_eq!(
            TmuxServer::new().connect_argv(&["attach-session", "-t", "work"]),
            argv(&["-CC", "attach-session", "-t", "work"])
        );
    }

    #[test]
    fn connect_argv_socket_name() {
        assert_eq!(
            TmuxServer::new()
                .socket_name("foo")
                .connect_argv(&["new-session"]),
            argv(&["-L", "foo", "-CC", "new-session"])
        );
    }

    #[test]
    fn connect_argv_socket_path() {
        assert_eq!(
            TmuxServer::new()
                .socket_path("/tmp/foo")
                .connect_argv(&["new-session"]),
            argv(&["-S", "/tmp/foo", "-CC", "new-session"])
        );
    }

    #[test]
    fn connect_argv_default_program_and_cc() {
        let server = TmuxServer::new();
        assert_eq!(server.program, "tmux");
        assert_eq!(server.connect_argv(&[]), argv(&["-CC"]));
    }

    #[test]
    fn send_writes_command_bytes() {
        let cap = CaptureWriter::default();
        let sink = cap.data.clone();
        let (_protocol, handle) = make_handle(Box::new(cap));
        handle.send("list-panes").unwrap();
        assert_eq!(&*sink.lock().unwrap(), b"list-panes\n");
    }

    #[test]
    fn send_rollback_on_write_failure() {
        let (protocol, handle) = make_handle(Box::new(ErroringWriter));
        let err = handle.send("x").unwrap_err();
        assert!(matches!(err, TmuxError::Io(_)));
        assert_eq!(protocol.lock().unwrap().pending_len(), 0);
    }

    #[test]
    fn pump_emits_command_complete() {
        let (protocol, handle) = make_handle(Box::new(CaptureWriter::default()));
        let id = handle.send("list-panes").unwrap();
        let (tx, rx) = unbounded();
        let reader = ScriptedReader {
            chunks: vec![b"%begin 1 0 0\nout\n%end 1 0 0\n".to_vec()].into(),
        };
        pump(reader, protocol, tx);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(matches!(
            &events[0],
            TransportEvent::Protocol(ClientEvent::CommandComplete { id: cid, .. }) if *cid == id
        ));
        assert!(matches!(
            events.last().unwrap(),
            TransportEvent::Closed { .. }
        ));
    }

    #[test]
    fn pump_emits_error_reply() {
        let (protocol, handle) = make_handle(Box::new(CaptureWriter::default()));
        let id = handle.send("bogus").unwrap();
        let (tx, rx) = unbounded();
        let reader = ScriptedReader {
            chunks: vec![b"%begin 1 0 0\nbad command\n%error 1 0 0\n".to_vec()].into(),
        };
        pump(reader, protocol, tx);
        assert_eq!(
            protocol_events(&rx),
            vec![ClientEvent::CommandComplete {
                id,
                number: 0,
                ok: false,
                output: vec!["bad command".to_string()],
            }]
        );
    }

    #[test]
    fn pump_eof_emits_closed() {
        let (protocol, _h) = make_handle(Box::new(CaptureWriter::default()));
        let (tx, rx) = unbounded();
        pump(
            ScriptedReader {
                chunks: VecDeque::new(),
            },
            protocol,
            tx,
        );
        let events: Vec<_> = rx.try_iter().collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], TransportEvent::Closed { .. }));
    }

    #[test]
    fn pump_chunk_boundaries() {
        let (protocol, handle) = make_handle(Box::new(CaptureWriter::default()));
        let id = handle.send("x").unwrap();
        let (tx, rx) = unbounded();
        let reader = ScriptedReader {
            chunks: vec![
                b"%begin 1 0 0\n".to_vec(),
                b"bo".to_vec(),
                b"dy\n%end ".to_vec(),
                b"1 0 0\n".to_vec(),
            ]
            .into(),
        };
        pump(reader, protocol, tx);
        assert_eq!(
            protocol_events(&rx),
            vec![ClientEvent::CommandComplete {
                id,
                number: 0,
                ok: true,
                output: vec!["body".into()],
            }]
        );
    }

    #[test]
    fn pump_notifications_in_order() {
        let (protocol, _h) = make_handle(Box::new(CaptureWriter::default()));
        let (tx, rx) = unbounded();
        let reader = ScriptedReader {
            chunks: vec![b"%window-add @1\n%window-close @1\n".to_vec()].into(),
        };
        pump(reader, protocol, tx);
        assert_eq!(
            protocol_events(&rx),
            vec![
                ClientEvent::Notification(ControlEvent::WindowAdd {
                    window: WindowId(1)
                }),
                ClientEvent::Notification(ControlEvent::WindowClose {
                    window: WindowId(1)
                }),
            ]
        );
    }

    #[test]
    fn pump_reader_error_emits_closed() {
        struct BoomReader;
        impl Read for BoomReader {
            fn read(&mut self, _b: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("io"))
            }
        }
        let (protocol, _h) = make_handle(Box::new(CaptureWriter::default()));
        let (tx, rx) = unbounded();
        pump(BoomReader, protocol, tx);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(matches!(events[0], TransportEvent::Closed { .. }));
    }

    #[test]
    fn handle_is_send_sync_and_clonable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TmuxHandle>();
        let (_protocol, handle) = make_handle(Box::new(CaptureWriter::default()));
        let h2 = handle.clone();
        let t = std::thread::spawn(move || {
            let _ = h2.send("from-thread");
        });
        t.join().unwrap();
    }

    #[test]
    fn list_sessions_argv_default() {
        assert_eq!(
            TmuxServer::new().list_sessions_argv(),
            argv(&["list-sessions", "-F", LIST_FORMAT])
        );
    }

    #[test]
    fn list_sessions_argv_socket_name() {
        assert_eq!(
            TmuxServer::new().socket_name("foo").list_sessions_argv(),
            argv(&["-L", "foo", "list-sessions", "-F", LIST_FORMAT])
        );
    }

    #[test]
    fn list_sessions_argv_socket_path() {
        assert_eq!(
            TmuxServer::new()
                .socket_path("/tmp/foo")
                .list_sessions_argv(),
            argv(&["-S", "/tmp/foo", "list-sessions", "-F", LIST_FORMAT])
        );
    }

    #[test]
    fn list_format_uses_real_tab_and_name_last() {
        assert!(LIST_FORMAT.as_bytes().contains(&b'\t'));
        assert!(!LIST_FORMAT.contains("\\t"));
        assert!(LIST_FORMAT.ends_with("#{session_name}"));
    }

    #[test]
    fn classify_ok_parses_stdout() {
        let out = b"$0\t1\t0\t1\tmain\n";
        assert_eq!(
            classify_list_result(true, out, b"").unwrap(),
            vec![SessionInfo {
                id: SessionId(0),
                name: "main".to_string(),
                windows: 1,
                attached: false,
                created: 1,
            }]
        );
    }

    #[test]
    fn classify_no_server_running_is_empty() {
        let stderr = b"no server running on /tmp/tmux-501/foo\n";
        assert_eq!(classify_list_result(false, b"", stderr).unwrap(), vec![]);
    }

    #[test]
    fn classify_socket_missing_is_empty() {
        let stderr = b"error connecting to /tmp/tmux-501/foo (No such file or directory)\n";
        assert_eq!(classify_list_result(false, b"", stderr).unwrap(), vec![]);
    }

    #[test]
    fn classify_real_error_is_err() {
        let stderr = b"error connecting to /tmp/tmux-501/foo (Operation not permitted)\n";
        assert!(classify_list_result(false, b"", stderr).is_err());
    }

    #[test]
    fn classify_blank_stderr_still_has_message() {
        let err = classify_list_result(false, b"", b"").unwrap_err();
        let TmuxError::Spawn(io) = err else {
            panic!("expected Spawn");
        };
        assert!(!io.to_string().is_empty());
    }

    #[test]
    #[ignore = "requires a real tmux binary and a controlling PTY"]
    fn real_tmux_roundtrip() {
        use std::time::{Duration, Instant};

        let socket = format!("ozmux-test-{}", std::process::id());
        let server = TmuxServer::new().socket_name(&socket);
        let client = server.new_session().expect("spawn tmux -CC new-session");

        let id = client.handle().send("list-panes").expect("send list-panes");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut observed = false;
        while Instant::now() < deadline {
            match client.events().recv_timeout(Duration::from_millis(200)) {
                Ok(TransportEvent::Protocol(ClientEvent::CommandComplete {
                    id: cid,
                    ok,
                    output,
                    ..
                })) if cid == id => {
                    assert!(ok, "list-panes should succeed");
                    assert!(!output.is_empty(), "list-panes should report a pane");
                    observed = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        assert!(observed, "did not observe CommandComplete for list-panes");

        let _ = client.handle().send("kill-session");
    }

    #[test]
    #[ignore = "requires a real tmux binary"]
    fn real_tmux_list_sessions() {
        use std::time::Duration;

        let socket = format!("ozmux-ls-{}", std::process::id());
        let server = TmuxServer::new().socket_name(&socket);

        assert_eq!(server.list_sessions().expect("list (no server)"), vec![]);

        let client = server.new_session().expect("spawn tmux -CC new-session");
        let name = format!("ozmux-ls-sess-{}", std::process::id());
        client
            .handle()
            .send(&format!("rename-session {name}"))
            .expect("rename-session");
        std::thread::sleep(Duration::from_millis(500));

        let sessions = server.list_sessions().expect("list");
        let found = sessions
            .iter()
            .find(|s| s.name == name)
            .expect("created session listed by name");
        assert!(found.attached, "the -CC client is attached");
        assert!(found.windows >= 1);

        let _ = client.handle().send("kill-server");
        drop(client);
    }
}
