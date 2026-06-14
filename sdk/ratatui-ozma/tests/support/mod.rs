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
pub struct FakeServer {
    pub sock_path: std::path::PathBuf,
    received: Receiver<Value>,
    to_client: Sender<Value>,
    _dir: tempfile::TempDir,
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
