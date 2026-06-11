//! Tokio-free NDJSON RPC client to the single Node host: a long-lived
//! `UnixStream` with a writer thread draining outbound request lines and a
//! reader thread pumping inbound NDJSON reply lines onto a crossbeam channel.

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

const WRITER_POLL: Duration = Duration::from_millis(100);

/// A connected NDJSON RPC client to the single Node host. Outbound
/// `{reqId, ns, method, args}` request lines are queued via
/// [`HostRpcClient::send_line`]; inbound `{reqId, ok, …}` reply lines are read
/// via [`HostRpcClient::try_recv_response`]. The writer/reader threads are
/// joined on drop.
pub struct HostRpcClient {
    outbound: Sender<String>,
    responses: Receiver<String>,
    shutdown: Arc<AtomicBool>,
    stream: UnixStream,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
}

impl HostRpcClient {
    /// Connects to the host RPC socket and starts the writer + reader threads.
    pub fn connect(sock: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(sock)?;
        let mut write_half = stream.try_clone()?;
        let read_half = stream.try_clone()?;
        let (out_tx, out_rx) = unbounded::<String>();
        let (in_tx, in_rx) = unbounded::<String>();
        let shutdown = Arc::new(AtomicBool::new(false));

        let writer = {
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                loop {
                    match out_rx.recv_timeout(WRITER_POLL) {
                        Ok(line) => {
                            if write_half.write_all(line.as_bytes()).is_err()
                                || write_half.write_all(b"\n").is_err()
                                || write_half.flush().is_err()
                            {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if shutdown.load(Ordering::SeqCst) {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
        };

        let reader = std::thread::spawn(move || {
            let mut lines = BufReader::new(read_half);
            let mut buf = String::new();
            loop {
                buf.clear();
                match lines.read_line(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = buf.trim_end_matches(['\n', '\r']).to_string();
                        if in_tx.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Self {
            outbound: out_tx,
            responses: in_rx,
            shutdown,
            stream,
            writer: Some(writer),
            reader: Some(reader),
        })
    }

    /// Queues an NDJSON request line for the host (the writer appends `\n`).
    pub fn send_line(&self, line: String) {
        let _ = self.outbound.send(line);
    }

    /// Pops the next NDJSON reply line, or `None` if none is queued.
    pub fn try_recv_response(&self) -> Option<String> {
        self.responses.try_recv().ok()
    }
}

impl Drop for HostRpcClient {
    fn drop(&mut self) {
        // NOTE: signal shutdown, then shut the stream so the reader's blocking
        // read_line returns; the writer exits within one WRITER_POLL even though
        // a sender clone may still live in the HostRpc resource.
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::time::Duration;

    #[test]
    fn round_trips_one_ndjson_request_and_reply() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("rpc.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(
                line.contains("\"reqId\":\"7\""),
                "server saw the request line"
            );
            let mut w = stream;
            w.write_all(b"{\"reqId\":\"7\",\"ok\":true,\"value\":42}\n")
                .unwrap();
            w.flush().unwrap();
        });

        let client = HostRpcClient::connect(&sock).unwrap();
        client.send_line(
            "{\"reqId\":\"7\",\"ns\":\"fs\",\"method\":\"read\",\"args\":[]}".to_string(),
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let reply = loop {
            if let Some(line) = client.try_recv_response() {
                break line;
            }
            assert!(std::time::Instant::now() < deadline, "no reply within 2s");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(reply, "{\"reqId\":\"7\",\"ok\":true,\"value\":42}");
        server.join().unwrap();
    }
}
