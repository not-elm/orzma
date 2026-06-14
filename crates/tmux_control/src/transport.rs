//! I/O transport: owns a `tmux -CC` process and pumps its output through a
//! [`crate::ProtocolClient`].

use crate::error::{TmuxError, TmuxResult};
use crate::protocol::{ClientEvent, CommandId, ProtocolClient};
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

/// Builder for launching `tmux -CC`; the `-CC` flag is always injected.
pub struct TmuxBuilder {
    program: String,
    socket_name: Option<String>,
    socket_path: Option<String>,
    subcommand: Vec<String>,
}

impl TmuxBuilder {
    /// Returns a builder defaulting to the `tmux` binary on `PATH`.
    pub fn new() -> Self {
        Self {
            program: "tmux".to_string(),
            socket_name: None,
            socket_path: None,
            subcommand: Vec::new(),
        }
    }

    /// Overrides the tmux binary path.
    pub fn program(mut self, path: &str) -> Self {
        self.program = path.to_string();
        self
    }

    /// Sets the server socket name (`-L`).
    pub fn socket_name(mut self, name: &str) -> Self {
        self.socket_name = Some(name.to_string());
        self
    }

    /// Sets the server socket path (`-S`).
    pub fn socket_path(mut self, path: &str) -> Self {
        self.socket_path = Some(path.to_string());
        self
    }

    /// Launches `tmux -CC new-session`.
    pub fn new_session(mut self) -> TmuxResult<TmuxClient> {
        self.subcommand = vec!["new-session".to_string()];
        self.spawn()
    }

    /// Launches `tmux -CC attach-session -t <name>`.
    pub fn attach(mut self, name: &str) -> TmuxResult<TmuxClient> {
        self.subcommand = vec![
            "attach-session".to_string(),
            "-t".to_string(),
            name.to_string(),
        ];
        self.spawn()
    }

    fn build_argv(&self) -> Vec<String> {
        let mut argv = Vec::new();
        if let Some(name) = &self.socket_name {
            argv.push("-L".to_string());
            argv.push(name.clone());
        }
        if let Some(path) = &self.socket_path {
            argv.push("-S".to_string());
            argv.push(path.clone());
        }
        argv.push("-CC".to_string());
        argv.extend(self.subcommand.iter().cloned());
        argv
    }

    fn spawn(self) -> TmuxResult<TmuxClient> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TmuxError::Spawn(std::io::Error::other(e.to_string())))?;

        let mut cmd = CommandBuilder::new(&self.program);
        for arg in self.build_argv() {
            cmd.arg(arg);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| TmuxError::Spawn(std::io::Error::other(e.to_string())))?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| TmuxError::Spawn(std::io::Error::other(e.to_string())))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| TmuxError::Spawn(std::io::Error::other(e.to_string())))?;

        let mut protocol = ProtocolClient::new();
        protocol.register_pending();
        let inner = Arc::new(Mutex::new(Inner { protocol, writer }));

        let (tx, rx) = unbounded();
        let pump_inner = inner.clone();
        let reader_thread = std::thread::spawn(move || pump(reader, pump_inner, tx));

        Ok(TmuxClient {
            events: rx,
            handle: TmuxHandle { inner },
            child,
            _master: pair.master,
            reader_thread: Some(reader_thread),
        })
    }
}

impl Default for TmuxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

struct Inner {
    protocol: ProtocolClient,
    writer: Box<dyn Write + Send>,
}

/// Cloneable handle for sending commands to tmux.
///
/// The protocol state and the writer live behind one mutex, shared with the
/// reader thread, so a `send` registers and writes in a single critical section.
#[derive(Clone)]
pub struct TmuxHandle {
    inner: Arc<Mutex<Inner>>,
}

impl TmuxHandle {
    /// Encodes and writes `command` to tmux, returning its [`CommandId`].
    ///
    /// On write failure the pending registration is rolled back so later
    /// replies stay correctly correlated.
    pub fn send(&self, command: &str) -> TmuxResult<CommandId> {
        let mut inner = self.inner.lock().expect("tmux inner mutex poisoned");
        let id = inner.protocol.send(command)?;
        let bytes = inner.protocol.take_outgoing();
        match inner
            .writer
            .write_all(&bytes)
            .and_then(|()| inner.writer.flush())
        {
            Ok(()) => Ok(id),
            Err(e) => {
                inner.protocol.rollback_last_pending(id);
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

fn pump<R: Read>(mut reader: R, inner: Arc<Mutex<Inner>>, sender: Sender<TransportEvent>) {
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
                    let mut guard = inner.lock().expect("tmux inner mutex poisoned");
                    guard.protocol.feed(&buf[..n])
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

    fn make_inner(writer: Box<dyn Write + Send>) -> (Arc<Mutex<Inner>>, TmuxHandle) {
        let inner = Arc::new(Mutex::new(Inner {
            protocol: ProtocolClient::new(),
            writer,
        }));
        let handle = TmuxHandle {
            inner: inner.clone(),
        };
        (inner, handle)
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
    fn build_argv_new_session() {
        let mut b = TmuxBuilder::new();
        b.subcommand = vec!["new-session".to_string()];
        assert_eq!(b.build_argv(), argv(&["-CC", "new-session"]));
    }

    #[test]
    fn build_argv_attach() {
        let mut b = TmuxBuilder::new();
        b.subcommand = vec!["attach-session".into(), "-t".into(), "work".into()];
        assert_eq!(
            b.build_argv(),
            argv(&["-CC", "attach-session", "-t", "work"])
        );
    }

    #[test]
    fn build_argv_socket_name() {
        let b = TmuxBuilder::new().socket_name("foo");
        assert_eq!(b.build_argv(), argv(&["-L", "foo", "-CC"]));
    }

    #[test]
    fn build_argv_default_program_and_cc() {
        let b = TmuxBuilder::new();
        assert_eq!(b.program, "tmux");
        assert_eq!(b.build_argv(), argv(&["-CC"]));
    }

    #[test]
    fn send_writes_command_bytes() {
        let cap = CaptureWriter::default();
        let sink = cap.data.clone();
        let (_inner, handle) = make_inner(Box::new(cap));
        handle.send("list-panes").unwrap();
        assert_eq!(&*sink.lock().unwrap(), b"list-panes\n");
    }

    #[test]
    fn send_rollback_on_write_failure() {
        let (inner, handle) = make_inner(Box::new(ErroringWriter));
        let err = handle.send("x").unwrap_err();
        assert!(matches!(err, TmuxError::Io(_)));
        assert_eq!(inner.lock().unwrap().protocol.pending_len(), 0);
    }

    #[test]
    fn pump_emits_command_complete() {
        let (inner, handle) = make_inner(Box::new(CaptureWriter::default()));
        let id = handle.send("list-panes").unwrap();
        let (tx, rx) = unbounded();
        let reader = ScriptedReader {
            chunks: vec![b"%begin 1 0 0\nout\n%end 1 0 0\n".to_vec()].into(),
        };
        pump(reader, inner, tx);
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
    fn pump_eof_emits_closed() {
        let (inner, _h) = make_inner(Box::new(CaptureWriter::default()));
        let (tx, rx) = unbounded();
        pump(
            ScriptedReader {
                chunks: VecDeque::new(),
            },
            inner,
            tx,
        );
        let events: Vec<_> = rx.try_iter().collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], TransportEvent::Closed { .. }));
    }

    #[test]
    fn pump_chunk_boundaries() {
        let (inner, handle) = make_inner(Box::new(CaptureWriter::default()));
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
        pump(reader, inner, tx);
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
        let (inner, _h) = make_inner(Box::new(CaptureWriter::default()));
        let (tx, rx) = unbounded();
        let reader = ScriptedReader {
            chunks: vec![b"%window-add @1\n%window-close @1\n".to_vec()].into(),
        };
        pump(reader, inner, tx);
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
        let (inner, _h) = make_inner(Box::new(CaptureWriter::default()));
        let (tx, rx) = unbounded();
        pump(BoomReader, inner, tx);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(matches!(events[0], TransportEvent::Closed { .. }));
    }

    #[test]
    fn handle_is_send_sync_and_clonable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TmuxHandle>();
        let (_inner, handle) = make_inner(Box::new(CaptureWriter::default()));
        let h2 = handle.clone();
        let t = std::thread::spawn(move || {
            let _ = h2.send("from-thread");
        });
        t.join().unwrap();
    }

    #[test]
    #[ignore = "requires a real tmux binary and a controlling PTY"]
    fn real_tmux_roundtrip() {
        use std::time::{Duration, Instant};

        let socket = format!("ozmux-test-{}", std::process::id());
        let client = TmuxBuilder::new()
            .socket_name(&socket)
            .new_session()
            .expect("spawn tmux -CC new-session");

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
}
