//! A fake ozmux control server for integration tests.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

/// A fake control server: accepts one client, auto-replies to the first
/// `register` with a fixed handle, forwards every client line over `received`,
/// and pushes lines to the client via `to_client`.
///
/// `start` is non-blocking: it binds the socket and spawns the accept/reader and
/// writer threads, then returns immediately so the client can connect afterward.
///
/// Dropping a `FakeServer` closes the connection: the internal `_close_tx` drops,
/// causing the shutdown-watcher thread to close the cloned stream, which makes the
/// client's reader see EOF.
pub struct FakeServer {
    pub sock_path: std::path::PathBuf,
    received: Receiver<Value>,
    to_client: Sender<Value>,
    _dir: tempfile::TempDir,
    _close_tx: std::sync::mpsc::SyncSender<()>,
}

impl FakeServer {
    /// Binds a temp socket and spawns the server threads (does not block on accept).
    pub fn start(handle: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("control.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let (recv_tx, received) = mpsc::channel::<Value>();
        let (to_client, client_rx) = mpsc::channel::<Value>();
        let reply_tx = to_client.clone();
        let handle = handle.to_owned();
        let (close_tx, close_rx) = mpsc::sync_channel::<()>(0);

        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut write_half = stream.try_clone().unwrap();
            thread::spawn(move || {
                while let Ok(v) = client_rx.recv() {
                    if writeln!(write_half, "{v}").is_err() {
                        break;
                    }
                    let _ = write_half.flush();
                }
            });

            // NOTE: dropping FakeServer drops _close_tx, which causes close_rx.recv()
            // to return Err here; we then shut down the socket, which closes the
            // connection from the server side and causes the client reader to see EOF
            // regardless of how many dup'd fds are still open.
            let stream_for_close = stream.try_clone().unwrap();
            thread::spawn(move || {
                let _ = close_rx.recv();
                let _ = stream_for_close.shutdown(std::net::Shutdown::Both);
            });

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            let mut replied = false;
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                    if !replied && v["op"] == "register" {
                        replied = true;
                        let _ = reply_tx.send(json!({ "ok": true, "handle": handle }));
                    }
                    let _ = recv_tx.send(v);
                }
            }
        });

        Self {
            sock_path,
            received,
            to_client,
            _dir: dir,
            _close_tx: close_tx,
        }
    }

    /// Binds a socket, accepts one client, then closes the connection upon
    /// receiving the first `register` WITHOUT replying — exercises the
    /// reader-thread disconnect / pending-drain path.
    pub fn start_dropping() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("control.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let (_recv_tx, received) = mpsc::channel::<Value>();
        let (to_client, _client_rx) = mpsc::channel::<Value>();
        let (close_tx, _close_rx) = mpsc::sync_channel::<()>(0);

        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<Value>(line.trim())
                    && v["op"] == "register"
                {
                    break;
                }
            }
        });

        Self {
            sock_path,
            received,
            to_client,
            _dir: dir,
            _close_tx: close_tx,
        }
    }

    /// Pushes a raw JSON line to the connected client.
    pub fn send(&self, v: Value) {
        self.to_client.send(v).unwrap();
    }

    /// Blocks for the next post-handshake line the client sent (skips hello/register).
    pub fn next_message(&self) -> Value {
        loop {
            let v = self.received.recv().unwrap();
            if v["op"] != "hello" && v["op"] != "register" {
                return v;
            }
        }
    }
}

/// A pair of fake servers for testing the reconnect path.
///
/// `first` accepts a client connection and auto-replies with `handle1`. Dropping
/// `first` closes the connection from the server side — the client reader sees EOF
/// and sets `disconnected=true`. `second` waits for the reconnect client and
/// auto-replies with `handle2`.
pub struct ReconnectPair {
    pub first: FakeServer,
    pub second: FakeServer,
}

impl ReconnectPair {
    /// Starts two fake servers at separate temp paths and returns them as a pair.
    pub fn start(handle1: &str, handle2: &str) -> Self {
        Self {
            first: FakeServer::start(handle1),
            second: FakeServer::start(handle2),
        }
    }
}
