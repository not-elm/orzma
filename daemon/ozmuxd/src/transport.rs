//! UDS transport for ozmuxd: bind + accept (nonblocking, shutdown-polling) +
//! a reader/writer thread pair per connection feeding the central LoopMsg loop.

use crate::{CLIENT_QUEUE_DEPTH, ClientId, LoopHandle, LoopMsg, Server};
use crossbeam_channel::{Sender, bounded};
use ozmux_mux::SessionSnapshot;
use ozmux_proto::{ClientMessage, ServerMessage, read_message, write_message};
use std::io::{self, BufReader};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

const ACCEPT_POLL: Duration = Duration::from_millis(50);

/// Returns the default daemon socket path under the system temp dir.
///
/// Uses `$TMPDIR` (via `std::env::temp_dir()`) rather than `$XDG_RUNTIME_DIR`
/// because `$XDG_RUNTIME_DIR` is unset on macOS. The leaf path is kept short to
/// stay within the ~104-byte `sun_path` limit on Darwin.
pub fn default_socket_path() -> PathBuf {
    std::env::temp_dir().join("ozmux").join("ozmuxd.sock")
}

/// A running daemon: the listening socket + central loop.
///
/// Dropping this shuts down the central loop and removes the socket file.
pub struct ServerHandle {
    loop_tx: Sender<LoopMsg>,
    // NOTE: never read by name — held purely for its Drop, which sends Shutdown
    // and joins the loop thread. Removing it would silently kill the loop at
    // construction time.
    #[expect(dead_code, reason = "held for Drop side-effect, not for its value")]
    loop_handle: LoopHandle,
    shutdown: Arc<AtomicBool>,
    accept_join: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl Server {
    /// Binds a Unix socket at `path`, starts the accept loop, and returns a
    /// `ServerHandle` that owns the running daemon.
    ///
    /// Any stale socket file at `path` is removed before binding. The parent
    /// directory is created if it does not already exist.
    pub fn serve(self, path: &Path) -> io::Result<ServerHandle> {
        let loop_handle = self.spawn_loop();
        let loop_tx = loop_handle.sender();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(e) = std::fs::remove_file(path)
            && e.kind() != io::ErrorKind::NotFound
        {
            return Err(e);
        }

        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let counter = Arc::new(AtomicU64::new(0));

        let accept_join = {
            let shutdown = Arc::clone(&shutdown);
            let loop_tx = loop_tx.clone();
            std::thread::spawn(move || {
                run_accept(listener, shutdown, loop_tx, counter);
            })
        };

        Ok(ServerHandle {
            loop_tx,
            loop_handle,
            shutdown,
            accept_join: Some(accept_join),
            path: path.to_path_buf(),
        })
    }
}

impl ServerHandle {
    /// Requests the current `SessionSnapshot` from the central loop.
    ///
    /// Returns `None` if the loop has shut down or does not respond within
    /// two seconds.
    pub fn snapshot(&self) -> Option<SessionSnapshot> {
        let (tx, rx) = bounded(1);
        self.loop_tx.send(LoopMsg::Snapshot { reply: tx }).ok()?;
        rx.recv_timeout(Duration::from_secs(2)).ok()
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(j) = self.accept_join.take() {
            let _ = j.join();
        }
        let _ = std::fs::remove_file(&self.path);
        // loop_handle's own Drop sends Shutdown and joins the loop thread.
    }
}

fn run_accept(
    listener: UnixListener,
    shutdown: Arc<AtomicBool>,
    loop_tx: Sender<LoopMsg>,
    counter: Arc<AtomicU64>,
) {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let id = ClientId(counter.fetch_add(1, Ordering::SeqCst));
                if let Err(e) = spawn_conn(stream, id, loop_tx.clone()) {
                    eprintln!("ozmuxd: failed to set up connection {}: {e}", id.0);
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn spawn_conn(stream: UnixStream, client_id: ClientId, loop_tx: Sender<LoopMsg>) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    let reader_stream = stream.try_clone()?;
    let teardown_stream = stream.try_clone()?;
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;

    let (out_tx, out_rx) = bounded::<ServerMessage>(CLIENT_QUEUE_DEPTH);

    std::thread::spawn(move || {
        while let Ok(msg) = out_rx.recv() {
            if write_message(&mut writer, &msg).is_err() {
                break;
            }
        }
    });

    std::thread::spawn(move || {
        match read_message::<_, ClientMessage>(&mut reader) {
            Ok(Some(ClientMessage::Hello {
                protocol_version,
                viewport,
            })) => {
                if loop_tx
                    .send(LoopMsg::Attach {
                        client_id,
                        writer: out_tx,
                        viewport,
                        protocol_version,
                        disconnect: Some(Box::new(move || {
                            let _ = teardown_stream.shutdown(std::net::Shutdown::Both);
                        })),
                    })
                    .is_err()
                {
                    return;
                }
            }
            _ => return,
        }

        loop {
            match read_message::<_, ClientMessage>(&mut reader) {
                Ok(Some(msg)) => {
                    if loop_tx.send(LoopMsg::ClientFrame(client_id, msg)).is_err() {
                        break;
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = loop_tx.send(LoopMsg::Disconnect(client_id));
                    break;
                }
            }
        }
    });

    Ok(())
}
