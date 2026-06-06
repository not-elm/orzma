//! A connected ozmux client: sends `Hello`, builds a `ClientMirror` from the
//! `Welcome` snapshot, then folds streamed events into the mirror. Generic over
//! the reader/writer so `ozmux_proto` stays platform-neutral (the caller splits
//! a `UnixStream` via `try_clone`).

use crate::{
    ClientMessage, ClientMirror, PROTOCOL_VERSION, ServerMessage, read_message, write_message,
};
use std::io::{self, BufRead, Write};

/// A connected ozmux client: the wire connection plus the reconstructed mirror.
pub struct Client<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    mirror: ClientMirror,
}

impl<R: BufRead, W: Write> Client<R, W> {
    /// Connects: sends `Hello{viewport}`, reads the `Welcome`, builds the mirror.
    ///
    /// Errors on EOF before `Welcome`, a protocol-version mismatch, or an
    /// `Error`/unexpected message in place of `Welcome`.
    pub fn connect(mut reader: R, mut writer: W, viewport: (u16, u16)) -> io::Result<Self> {
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
            ServerMessage::Event(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected Welcome, got Event",
                ));
            }
            ServerMessage::Frame { .. } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected Frame before Welcome",
                ));
            }
        };
        Ok(Self {
            reader,
            writer,
            mirror,
        })
    }

    /// Sends a command to the daemon.
    pub fn send(&mut self, msg: ClientMessage) -> io::Result<()> {
        write_message(&mut self.writer, &msg)
    }

    /// Reads the next server message; an `Event` is applied to the mirror before
    /// it is returned. `Ok(None)` at clean EOF.
    pub fn poll(&mut self) -> io::Result<Option<ServerMessage>> {
        let msg = read_message::<_, ServerMessage>(&mut self.reader)?;
        // NOTE: ServerMessage::Frame is passed through to the caller as-is and
        // is NOT applied to the mirror — Frame carries VT pixel data, not mux
        // state, so folding it here would corrupt the mirror.
        if let Some(ServerMessage::Event(ref ev)) = msg {
            self.mirror.apply_event(ev);
        }
        Ok(msg)
    }

    /// The reconstructed session state.
    pub fn mirror(&self) -> &ClientMirror {
        &self.mirror
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
