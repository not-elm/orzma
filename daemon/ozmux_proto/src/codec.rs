//! Length-prefixed framing: `[u32 BE body_len][serde_json body]`. Handles large
//! frame snapshots the old NDJSON line cap would reject, and is binary-ready.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{self, Read, Write};

/// The wire protocol version (bumped on any incompatible change).
pub const PROTOCOL_VERSION: u32 = 4;

/// Maximum body bytes per message (malformed-frame guard + the eager-allocation
/// ceiling on read). 8 MiB clears the ~1.83 MB worst-case decorated-TUI snapshot
/// with headroom while bounding the attacker-controlled `vec![0u8; len]` alloc.
pub const MAX_MESSAGE_BYTES: u32 = 8 * 1024 * 1024;

/// Writes `msg` as `[u32 BE len][serde_json body]` and flushes. Caps on write
/// (symmetric with read) so an oversized frame fails at the source.
pub fn write_message<W: Write, M: Serialize>(w: &mut W, msg: &M) -> io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_MESSAGE_BYTES as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message exceeds maximum",
        ));
    }
    let len = body.len() as u32;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

/// Reads one length-prefixed message. `Ok(None)` at clean EOF (zero bytes before
/// the prefix); errors on a torn prefix, a truncated body, an over-cap length,
/// or invalid JSON.
pub fn read_message<R: Read, M: DeserializeOwned>(r: &mut R) -> io::Result<Option<M>> {
    let mut first = [0u8; 1];
    if r.read(&mut first)? == 0 {
        return Ok(None); // clean EOF
    }
    let mut rest = [0u8; 3];
    r.read_exact(&mut rest)?; // torn prefix (1-3 bytes then EOF) → UnexpectedEof
    let len = u32::from_be_bytes([first[0], rest[0], rest[1], rest[2]]);
    if len > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message length exceeds maximum",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?; // truncated body → UnexpectedEof
    let msg =
        serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::{MAX_MESSAGE_BYTES, read_message, write_message};
    use crate::message::{ClientMessage, ServerMessage};
    use ozmux_mux::{
        MuxEvent, PaneDirection, PaneId, Side, SplitOrientation, SurfaceId, SurfaceKind,
        WorkspaceId,
    };
    use ozmux_vt::color::RgbaColor;
    use ozmux_vt::event::VtEvent;
    use ozmux_vt::frame::{Cursor, Frame, FrameDelta, FrameSnapshot, Row, Run, SnapshotReason};
    use std::io::Cursor as IoCursor;
    use std::path::PathBuf;

    #[test]
    fn client_messages_round_trip() {
        let messages = vec![
            ClientMessage::Hello {
                protocol_version: 3,
                viewport: (80, 24),
            },
            ClientMessage::Split {
                pane: PaneId::default(),
                orientation: SplitOrientation::Horizontal,
                side: Side::After,
                kind: SurfaceKind::Terminal,
                cwd: None,
            },
            ClientMessage::SetActiveSurface {
                pane: PaneId::default(),
                surface: SurfaceId::default(),
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
                cwd: None,
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
            ClientMessage::Input {
                surface: SurfaceId::default(),
                bytes: vec![1, 2, 3],
            },
            ClientMessage::Scroll {
                surface: SurfaceId::default(),
                delta: 3,
            },
        ];

        let mut buf: Vec<u8> = Vec::new();
        for msg in &messages {
            write_message(&mut buf, msg).unwrap();
        }

        let mut cursor = IoCursor::new(buf);
        for expected in &messages {
            let got: Option<ClientMessage> = read_message(&mut cursor).unwrap();
            assert_eq!(got.as_ref(), Some(expected));
        }
        let eof: Option<ClientMessage> = read_message(&mut cursor).unwrap();
        assert!(eof.is_none(), "expected clean EOF after all messages");
    }

    #[test]
    fn server_messages_round_trip() {
        let minimal_snapshot = FrameSnapshot {
            seq: 1,
            cols: 80,
            rows: 24,
            cursor: Cursor::default(),
            rows_data: vec![Row { runs: vec![] }],
            reason: SnapshotReason::Initial,
            modes: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 0,
            vi_cursor: None,
            selection: None,
        };
        let minimal_delta = FrameDelta {
            seq: 2,
            cursor: Cursor::default(),
            dirty_rows: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            vi_cursor: None,
            selection: None,
        };
        let messages = vec![
            ServerMessage::Events(vec![MuxEvent::PaneClosed {
                pane: PaneId::default(),
            }]),
            ServerMessage::SurfaceEvent {
                surface: SurfaceId::default(),
                event: VtEvent::TitleChanged(Some("x".into())),
            },
            ServerMessage::Error {
                message: "x".into(),
            },
            ServerMessage::Frame {
                surface: SurfaceId::default(),
                frame: Frame::Snapshot(minimal_snapshot),
            },
            ServerMessage::Frame {
                surface: SurfaceId::default(),
                frame: Frame::Delta(minimal_delta),
            },
        ];

        let mut buf: Vec<u8> = Vec::new();
        for msg in &messages {
            write_message(&mut buf, msg).unwrap();
        }

        let mut cursor = IoCursor::new(buf);
        for expected in &messages {
            let got: Option<ServerMessage> = read_message(&mut cursor).unwrap();
            assert_eq!(got.as_ref(), Some(expected));
        }
        let eof: Option<ServerMessage> = read_message(&mut cursor).unwrap();
        assert!(eof.is_none(), "expected clean EOF after all messages");
    }

    #[test]
    fn over_cap_length_errs() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_MESSAGE_BYTES + 1).to_be_bytes());
        let mut cur = IoCursor::new(buf);
        let r = read_message::<_, ClientMessage>(&mut cur);
        assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn clean_eof_none_torn_prefix_errs() {
        let mut empty = IoCursor::new(Vec::<u8>::new());
        assert!(
            read_message::<_, ClientMessage>(&mut empty)
                .unwrap()
                .is_none()
        );
        let mut torn = IoCursor::new(vec![0u8, 0u8]); // 2 bytes then EOF
        assert_eq!(
            read_message::<_, ClientMessage>(&mut torn)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn truncated_body_errs() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&100u32.to_be_bytes()); // claims 100 bytes
        buf.extend_from_slice(b"short"); // far fewer
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ClientMessage>(&mut cur)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn large_snapshot_round_trips() {
        // Build a FrameSnapshot whose JSON exceeds the old 1 MiB NDJSON cap.
        let run = Run {
            cols: 80,
            fg: RgbaColor::WHITE,
            bg: RgbaColor::BLACK,
            style: 0,
            text: "x".repeat(80),
            hyperlink_id: None,
        };
        let row = Row { runs: vec![run; 8] };
        // Each row serializes to ~1-2 KB; 2000 rows comfortably exceeds 1 MiB.
        let rows_data = vec![row; 2000];
        let big = FrameSnapshot {
            seq: 1,
            cols: 80,
            rows: 2000,
            cursor: Cursor::default(),
            rows_data,
            reason: SnapshotReason::Initial,
            modes: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 0,
            vi_cursor: None,
            selection: None,
        };
        let msg = ServerMessage::Frame {
            surface: SurfaceId::default(),
            frame: Frame::Snapshot(big),
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        assert!(
            buf.len() > (1 << 20),
            "payload must exceed the old 1 MiB cap, got {}",
            buf.len()
        );
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ServerMessage>(&mut cur).unwrap(),
            Some(msg)
        );
    }

    #[test]
    fn spawn_surface_with_cwd_round_trips() {
        let msg = ClientMessage::SpawnSurface {
            pane: PaneId::default(),
            kind: SurfaceKind::Terminal,
            cwd: Some(PathBuf::from("/tmp")),
        };
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ClientMessage>(&mut cur).unwrap(),
            Some(msg)
        );
    }

    #[test]
    fn split_with_cwd_round_trips() {
        let msg = ClientMessage::Split {
            pane: PaneId::default(),
            orientation: SplitOrientation::Horizontal,
            side: Side::After,
            kind: SurfaceKind::Terminal,
            cwd: Some(PathBuf::from("/home/user/project")),
        };
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ClientMessage>(&mut cur).unwrap(),
            Some(msg)
        );
    }

    #[test]
    fn create_workspace_named_round_trips() {
        let msg = ClientMessage::CreateWorkspace {
            name: Some("proj".into()),
        };
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ClientMessage>(&mut cur).unwrap(),
            Some(msg)
        );
    }

    #[test]
    fn create_workspace_unnamed_round_trips() {
        let msg = ClientMessage::CreateWorkspace { name: None };
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ClientMessage>(&mut cur).unwrap(),
            Some(msg)
        );
    }

    #[test]
    fn select_workspace_round_trips() {
        let msg = ClientMessage::SelectWorkspace {
            workspace: WorkspaceId::default(),
        };
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ClientMessage>(&mut cur).unwrap(),
            Some(msg)
        );
    }

    #[test]
    fn scroll_round_trips() {
        let msg = ClientMessage::Scroll {
            surface: SurfaceId::default(),
            delta: 3,
        };
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ClientMessage>(&mut cur).unwrap(),
            Some(msg)
        );
    }

    #[test]
    fn surface_spawned_with_cwd_round_trips() {
        let msg = ServerMessage::Events(vec![MuxEvent::SurfaceSpawned {
            pane: PaneId::default(),
            surface: SurfaceId::default(),
            kind: SurfaceKind::Terminal,
            cwd: PathBuf::from("/x"),
        }]);
        let mut buf: Vec<u8> = Vec::new();
        write_message(&mut buf, &msg).unwrap();
        let mut cur = IoCursor::new(buf);
        assert_eq!(
            read_message::<_, ServerMessage>(&mut cur).unwrap(),
            Some(msg)
        );
    }
}
