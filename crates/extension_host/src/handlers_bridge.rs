//! Proxies handler/channel frames between the rendered extension UI and the
//! extension's `OZMUX_HANDLERS_SOCK_PATH`. Per surface (`surface_id`), one persistent
//! UDS connection carrying `{surface_id, frame}\n` NDJSON in both directions. Pure
//! transport (std + crossbeam) — the ECS glue drains `outbound()` each frame.

use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;

/// A frame addressed to a surface (`surface_id`) — the JSON `frame` object as a string.
pub type SurfaceIdFrame = (String, String);

/// Owns the per-surface handler connections and a shared outbound channel.
pub struct HandlersBridge {
    conns: Mutex<HashMap<String, Conn>>,
    out_tx: Sender<SurfaceIdFrame>,
    out_rx: Receiver<SurfaceIdFrame>,
}

struct Conn {
    writer: UnixStream,
    reader_thread: Option<std::thread::JoinHandle<()>>,
}

impl HandlersBridge {
    /// Creates an empty bridge.
    pub fn new() -> Self {
        let (out_tx, out_rx) = unbounded();
        Self {
            conns: Mutex::new(HashMap::new()),
            out_tx,
            out_rx,
        }
    }

    /// The channel of `(surface_id, frame)` responses from extensions (drained by ECS).
    pub fn outbound(&self) -> &Receiver<SurfaceIdFrame> {
        &self.out_rx
    }

    /// Opens (idempotently) a connection for `surface_id` to the handlers socket at
    /// `sock`, spawning a reader thread that forwards response envelopes to
    /// `outbound()`.
    pub fn connect(&self, surface_id: String, sock: PathBuf) -> std::io::Result<()> {
        let mut conns = self.conns.lock().unwrap();
        if conns.contains_key(&surface_id) {
            return Ok(());
        }
        let stream = UnixStream::connect(&sock)?;
        let reader = BufReader::new(stream.try_clone()?);
        let out_tx = self.out_tx.clone();
        let reader_thread = std::thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(frame) = split_envelope(&line)
                    && out_tx.send(frame).is_err()
                {
                    break;
                }
            }
        });
        conns.insert(
            surface_id,
            Conn {
                writer: stream,
                reader_thread: Some(reader_thread),
            },
        );
        Ok(())
    }

    /// Writes a client `frame` (JSON object string) to `surface_id`'s connection,
    /// wrapped as a `{surface_id, frame}` envelope. No-op if `surface_id` is not connected.
    pub fn send(&self, surface_id: &str, frame: String) {
        let mut conns = self.conns.lock().unwrap();
        if let Some(conn) = conns.get_mut(surface_id) {
            let line = format!(
                "{{\"surface_id\":{},\"frame\":{}}}\n",
                json_string(surface_id),
                frame
            );
            let _ = conn.writer.write_all(line.as_bytes());
        }
    }

    /// Closes `surface_id`'s connection (dropping it aborts the extension's subs).
    pub fn disconnect(&self, surface_id: &str) {
        let mut conns = self.conns.lock().unwrap();
        if let Some(mut conn) = conns.remove(surface_id) {
            let _ = conn.writer.shutdown(std::net::Shutdown::Both);
            if let Some(t) = conn.reader_thread.take() {
                let _ = t.join();
            }
        }
    }
}

impl Default for HandlersBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HandlersBridge {
    fn drop(&mut self) {
        let mut conns = self.conns.lock().unwrap();
        for (_surface_id, mut conn) in conns.drain() {
            let _ = conn.writer.shutdown(std::net::Shutdown::Both);
            if let Some(t) = conn.reader_thread.take() {
                let _ = t.join();
            }
        }
    }
}

fn split_envelope(line: &str) -> Option<SurfaceIdFrame> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let surface_id = v.get("surface_id")?.as_str()?.to_string();
    let frame = v.get("frame")?;
    Some((surface_id, serde_json::to_string(frame).ok()?))
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("string serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    // A fake handlers-server: echoes a `call` as a `result`, and answers a
    // `sub.open` with two `sub.data` then `sub.complete`.
    fn fake_handlers_server(sock: std::path::PathBuf) -> std::thread::JoinHandle<()> {
        let listener = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 {
                let env: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                let surface_id = env["surface_id"].as_str().unwrap().to_string();
                let frame = &env["frame"];
                let id = frame["id"].as_str().unwrap().to_string();
                match frame["kind"].as_str().unwrap() {
                    "call" => {
                        let out = serde_json::json!({"surface_id":surface_id,"frame":{"kind":"result","id":id,"payload":{"ok":true}}});
                        writeln!(writer, "{out}").unwrap();
                    }
                    "sub.open" => {
                        for n in 0..2 {
                            let out = serde_json::json!({"surface_id":surface_id,"frame":{"kind":"sub.data","id":id,"payload":{"n":n}}});
                            writeln!(writer, "{out}").unwrap();
                        }
                        let done = serde_json::json!({"surface_id":surface_id,"frame":{"kind":"sub.complete","id":id}});
                        writeln!(writer, "{done}").unwrap();
                    }
                    _ => {}
                }
                line.clear();
            }
        })
    }

    #[test]
    fn call_round_trips_through_the_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("memo.handlers.sock");
        let server = fake_handlers_server(sock.clone());
        std::thread::sleep(Duration::from_millis(50));

        let bridge = HandlersBridge::new();
        bridge.connect("aid-1".into(), sock.clone()).unwrap();
        bridge.send(
            "aid-1",
            r#"{"kind":"call","id":"c1","name":"greet","payload":{}}"#.into(),
        );

        let (surface_id, frame) = bridge
            .outbound()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(surface_id, "aid-1");
        let v: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(v["kind"], "result");
        assert_eq!(v["id"], "c1");

        drop(bridge);
        let _ = server.join();
    }

    #[test]
    fn subscription_streams_data_then_complete() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("memo.handlers.sock");
        let server = fake_handlers_server(sock.clone());
        std::thread::sleep(Duration::from_millis(50));

        let bridge = HandlersBridge::new();
        bridge.connect("aid-2".into(), sock.clone()).unwrap();
        bridge.send(
            "aid-2",
            r#"{"kind":"sub.open","id":"s1","name":"clock","params":{}}"#.into(),
        );

        let mut kinds = Vec::new();
        for _ in 0..3 {
            let (_surface_id, frame) = bridge
                .outbound()
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(&frame).unwrap();
            kinds.push(v["kind"].as_str().unwrap().to_string());
        }
        assert_eq!(kinds, vec!["sub.data", "sub.data", "sub.complete"]);

        drop(bridge);
        let _ = server.join();
    }
}
