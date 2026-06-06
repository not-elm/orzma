//! NDJSON framing: one JSON value per line. JSON escapes `\n` inside strings,
//! so the newline delimiter is unambiguous.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{self, BufRead, Read, Write};

/// The wire protocol version (bumped on any incompatible message change).
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum bytes in a single NDJSON line (a malformed/huge-frame guard).
pub const MAX_LINE_BYTES: u64 = 1 << 20;

/// Writes `msg` as one NDJSON line and flushes.
pub fn write_message<W: Write, M: Serialize>(w: &mut W, msg: &M) -> io::Result<()> {
    serde_json::to_writer(&mut *w, msg)?;
    w.write_all(b"\n")?;
    w.flush()
}

/// Reads one NDJSON line into `M`. `Ok(None)` at clean EOF; errors on a
/// truncated final line, an over-long line, or invalid JSON.
pub fn read_message<R: BufRead, M: DeserializeOwned>(r: &mut R) -> io::Result<Option<M>> {
    let mut line = String::new();
    let n = r.take(MAX_LINE_BYTES).read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated or over-long NDJSON line",
        ));
    }
    let msg =
        serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::{MAX_LINE_BYTES, read_message, write_message};
    use crate::message::{ClientMessage, ServerMessage};
    use ozmux_mux::{
        MuxEvent, PaneDirection, PaneId, Side, SplitOrientation, SurfaceId, SurfaceKind,
        WorkspaceId,
    };
    use std::io::Cursor;

    #[test]
    fn client_messages_round_trip() {
        let messages = vec![
            ClientMessage::Hello {
                protocol_version: 1,
                viewport: (80, 24),
            },
            ClientMessage::Split {
                pane: PaneId::default(),
                orientation: SplitOrientation::Horizontal,
            },
            ClientMessage::Close {
                pane: PaneId::default(),
            },
            ClientMessage::Navigate {
                pane: PaneId::default(),
                direction: PaneDirection::Right,
            },
            ClientMessage::SetActivePane {
                workspace: WorkspaceId::default(),
                pane: PaneId::default(),
            },
            ClientMessage::SpawnSurface {
                pane: PaneId::default(),
                kind: SurfaceKind::Terminal,
            },
            ClientMessage::BreakSurfaceToPane {
                surface: SurfaceId::default(),
                orientation: SplitOrientation::Vertical,
                side: Side::After,
            },
            ClientMessage::SetViewport {
                cols: 120,
                rows: 40,
            },
        ];

        let mut buf: Vec<u8> = Vec::new();
        for msg in &messages {
            write_message(&mut buf, msg).unwrap();
        }

        let mut cursor = Cursor::new(buf);
        for expected in &messages {
            let got: Option<ClientMessage> = read_message(&mut cursor).unwrap();
            assert_eq!(got.as_ref(), Some(expected));
        }
        let eof: Option<ClientMessage> = read_message(&mut cursor).unwrap();
        assert!(eof.is_none(), "expected clean EOF after all messages");
    }

    #[test]
    fn server_messages_round_trip() {
        let messages = vec![
            ServerMessage::Event(MuxEvent::PaneClosed {
                pane: PaneId::default(),
            }),
            ServerMessage::Error {
                message: "x".into(),
            },
        ];

        let mut buf: Vec<u8> = Vec::new();
        for msg in &messages {
            write_message(&mut buf, msg).unwrap();
        }

        let mut cursor = Cursor::new(buf);
        for expected in &messages {
            let got: Option<ServerMessage> = read_message(&mut cursor).unwrap();
            assert_eq!(got.as_ref(), Some(expected));
        }
        let eof: Option<ServerMessage> = read_message(&mut cursor).unwrap();
        assert!(eof.is_none(), "expected clean EOF after all messages");
    }

    #[test]
    fn over_long_line_errs() {
        // Write MAX_LINE_BYTES + 100 bytes with no newline.
        let payload = vec![b'x'; (MAX_LINE_BYTES + 100) as usize];
        let mut cursor = Cursor::new(payload);
        let result = read_message::<_, ClientMessage>(&mut cursor);
        assert!(result.is_err(), "expected Err for over-long line");
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData,
            "expected InvalidData error kind"
        );
    }
}
