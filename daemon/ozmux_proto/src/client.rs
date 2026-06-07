//! A connected ozmux client: sends `Hello`, builds a `ClientMirror` from the
//! `Welcome` snapshot, then delivers streamed events via a background reader
//! thread. Generic only over the writer so the caller can pass any `Write`.

use crate::{
    ClientMessage, ClientMirror, PROTOCOL_VERSION, ServerMessage, read_message, write_message,
};
use crossbeam_channel::{Receiver, RecvTimeoutError, TryRecvError, unbounded};
use std::io::{self, BufRead, Write};
use std::time::Duration;

/// `poll()`'s blocking budget — matches the 300 ms stream read-timeout the
/// daemon drain-loop tests rely on, so a quiescence `poll` returns promptly.
const POLL_TIMEOUT: Duration = Duration::from_millis(300);

/// A connected ozmux client: the wire writer + a background-read channel + the mirror.
pub struct Client<W: Write> {
    writer: W,
    rx: Receiver<io::Result<ServerMessage>>,
    mirror: ClientMirror,
    // NOTE: the handle prevents the reader thread from being silently detached;
    // dropping `Client` drops `rx`, the thread's `send` then errors, and it exits.
    _reader: std::thread::JoinHandle<()>,
}

impl<W: Write> Client<W> {
    /// Connects: sends `Hello{viewport}`, reads the `Welcome` **synchronously**
    /// (before the reader thread starts), builds the mirror, then spawns the
    /// background reader.
    ///
    /// Errors on EOF before `Welcome`, a protocol-version mismatch, or an
    /// `Error`/unexpected message in place of `Welcome`.
    pub fn connect<R: BufRead + Send + 'static>(
        mut reader: R,
        mut writer: W,
        viewport: (u16, u16),
    ) -> io::Result<Self> {
        write_message(
            &mut writer,
            &ClientMessage::Hello {
                protocol_version: PROTOCOL_VERSION,
                viewport,
            },
        )?;
        let welcome = read_message::<_, ServerMessage>(&mut reader)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no Welcome"))?;
        let mirror = match welcome {
            ServerMessage::Welcome {
                protocol_version,
                snapshot,
            } => {
                if protocol_version != PROTOCOL_VERSION {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "protocol version mismatch",
                    ));
                }
                ClientMirror::from_snapshot(snapshot)
            }
            ServerMessage::Error { message } => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, message));
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("expected Welcome, got {other:?}"),
                ));
            }
        };
        let (tx, rx) = unbounded::<io::Result<ServerMessage>>();
        let reader_thread = std::thread::spawn(move || loop {
            match read_message::<_, ServerMessage>(&mut reader) {
                Ok(Some(msg)) => {
                    if tx.send(Ok(msg)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                // NOTE: a stream read-timeout (tests set 300 ms) surfaces as
                // WouldBlock/TimedOut — that is quiescence, not a fatal error;
                // retry so the thread keeps listening for the next real message.
                Err(ref e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        });
        Ok(Self {
            writer,
            rx,
            mirror,
            _reader: reader_thread,
        })
    }

    /// Sends a command to the daemon.
    pub fn send(&mut self, msg: ClientMessage) -> io::Result<()> {
        write_message(&mut self.writer, &msg)
    }

    /// Blocks up to `POLL_TIMEOUT` for the next message; returns
    /// `Err(WouldBlock)` on quiescence (back-compat: daemon drain loops treat
    /// that as "done"). Returns `Ok(None)` at clean EOF (reader thread ended).
    pub fn poll(&mut self) -> io::Result<Option<ServerMessage>> {
        match self.rx.recv_timeout(POLL_TIMEOUT) {
            Ok(Ok(msg)) => Ok(Some(self.fold(msg))),
            Ok(Err(e)) => Err(e),
            Err(RecvTimeoutError::Timeout) => {
                Err(io::Error::new(io::ErrorKind::WouldBlock, "poll timeout"))
            }
            Err(RecvTimeoutError::Disconnected) => Ok(None),
        }
    }

    /// Non-blocking poll; returns `Ok(None)` when the channel is empty or the
    /// reader thread has ended.
    pub fn try_poll(&mut self) -> io::Result<Option<ServerMessage>> {
        match self.rx.try_recv() {
            Ok(Ok(msg)) => Ok(Some(self.fold(msg))),
            Ok(Err(e)) => Err(e),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Ok(None),
        }
    }

    /// The reconstructed session state.
    pub fn mirror(&self) -> &ClientMirror {
        &self.mirror
    }

    fn fold(&mut self, msg: ServerMessage) -> ServerMessage {
        if let ServerMessage::Event(ref ev) = msg {
            self.mirror.apply_event(ev);
        }
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_mux::Mux;
    use std::io::{BufReader, Cursor};

    #[test]
    fn connect_builds_mirror_from_welcome_and_sends_hello() {
        let mux = Mux::new();
        let session = mux.sessions()[0];
        let snapshot = mux.snapshot(session).unwrap();

        let mut server_bytes = Vec::new();
        write_message(
            &mut server_bytes,
            &ServerMessage::Welcome {
                protocol_version: PROTOCOL_VERSION,
                snapshot: snapshot.clone(),
            },
        )
        .unwrap();

        let reader = BufReader::new(Cursor::new(server_bytes));
        let writer: Vec<u8> = Vec::new();
        let client = Client::connect(reader, writer, (80, 24)).unwrap();

        assert_eq!(client.mirror().to_snapshot(), snapshot);
    }

    #[test]
    fn connect_errors_on_version_mismatch() {
        let mux = Mux::new();
        let snapshot = mux.snapshot(mux.sessions()[0]).unwrap();
        let mut server_bytes = Vec::new();
        write_message(
            &mut server_bytes,
            &ServerMessage::Welcome {
                protocol_version: PROTOCOL_VERSION + 1,
                snapshot,
            },
        )
        .unwrap();
        let reader = BufReader::new(Cursor::new(server_bytes));
        assert!(Client::connect(reader, Vec::<u8>::new(), (80, 24)).is_err());
    }
}
