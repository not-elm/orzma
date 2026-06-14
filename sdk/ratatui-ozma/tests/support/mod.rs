//! A fake ozmux control server for integration tests.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// A fake control server: accepts one client, replies to the first `register`
/// with a fixed handle, and forwards every client line over `received`.
pub struct FakeServer {
    pub sock_path: std::path::PathBuf,
    received: Receiver<Value>,
    server_writer: UnixStream,
    _dir: tempfile::TempDir,
}

impl FakeServer {
    /// Boots on a temp socket, waits for one client, and answers its register.
    pub fn start(handle: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("control.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let (conn_tx, conn_rx) = mpsc::channel::<UnixStream>();
        let (recv_tx, received) = mpsc::channel::<Value>();

        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            conn_tx.send(stream.try_clone().unwrap()).unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                    let _ = recv_tx.send(v);
                }
            }
        });

        let server_writer = conn_rx.recv().unwrap();
        let me = Self {
            sock_path,
            received,
            server_writer,
            _dir: dir,
        };
        me.drain_until_register();
        let mut me = me;
        me.send(json!({ "ok": true, "handle": handle }));
        me
    }

    fn drain_until_register(&self) {
        loop {
            let v = self.received.recv().unwrap();
            if v["op"] == "register" {
                return;
            }
        }
    }

    /// Sends a raw JSON line to the connected client.
    pub fn send(&mut self, v: Value) {
        writeln!(self.server_writer, "{v}").unwrap();
        self.server_writer.flush().unwrap();
    }

    /// Blocks for the next post-registration line the client sent.
    pub fn next_message(&self) -> Value {
        self.received.recv().unwrap()
    }
}
