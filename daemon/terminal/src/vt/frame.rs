//! Wire protocol types for snapshot/delta frames.

use serde::{Deserialize, Serialize};

/// Foreground/background color.
///
/// Wire encoding: `Default` = nil, `Indexed` = uint8, `Rgb` = 3-tuple of uint8.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Color {
    /// Use the terminal's default foreground/background color.
    Default,
    /// Indexed palette color (0-255).
    Indexed(u8),
    /// Direct RGB color.
    Rgb(u8, u8, u8),
}

/// Terminal cursor shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

/// Cursor state at snapshot time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub shape: CursorShape,
    pub visible: bool,
}

/// Style bitmask constants for [`Run::style`].
///
/// Bits 64 and 128 are reserved.
pub mod style {
    /// Bold weight.
    pub const BOLD: u8 = 1;
    /// Italic style.
    pub const ITALIC: u8 = 2;
    /// Underline decoration.
    pub const UNDERLINE: u8 = 4;
    /// Strikethrough decoration.
    pub const STRIKE: u8 = 8;
    /// Reversed foreground/background.
    pub const REVERSE: u8 = 16;
    /// Dim/faint weight.
    pub const DIM: u8 = 32;
}

/// A run of cells sharing identical fg/bg/style attributes.
///
/// Wide-char spacers (alacritty internal) are absorbed server-side and do
/// not appear in `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// Total column span (sum of grapheme cluster widths in `text`).
    pub cols: u16,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Style bitmask (see [`style`]).
    pub style: u8,
    /// UTF-8 text; the client uses Unicode East Asian Width to position each
    /// grapheme cluster within the run.
    pub text: String,
    /// Hyperlink id (OSC 8); always `None` until Phase 3.
    pub hyperlink_id: Option<u32>,
}

/// A row of runs ordered left-to-right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// Runs in left-to-right column order.
    pub runs: Vec<Run>,
}

/// A dirty row entry inside a `FrameDelta`.
///
/// `runs` represents the entire row (full row replacement, not partial).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyRow {
    /// Row index, zero-based from the top of the screen.
    pub row: u16,
    /// Full set of runs for the row.
    pub runs: Vec<Run>,
}

/// Reason a snapshot was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    /// Initial connect.
    Initial,
    /// Reconnect with no replay available.
    Reconnect,
    /// Receiver fell too far behind the broadcast.
    Lagged,
    /// Terminal was resized.
    Resize,
}

/// Full screen state snapshot.
///
/// Sent on connect, reconnect (no replay), lagged, or resize. The `kind`
/// discriminant is serialized via the [`RenderFrame`] tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameSnapshot {
    /// Monotonic frame sequence number.
    pub seq: u32,
    /// Terminal column count.
    pub cols: u16,
    /// Terminal row count.
    pub rows: u16,
    /// Cursor state at snapshot time.
    pub cursor: Cursor,
    /// Row contents, ordered top-to-bottom.
    pub rows_data: Vec<Row>,
    /// Why the snapshot was sent.
    pub reason: SnapshotReason,
    /// Currently-set wire mode names (subset of TRACKED_MODES). Authoritative
    /// for clients that missed a `mode` text sidecar.
    pub modes: Vec<String>,
}

/// Differential update relative to the prior frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameDelta {
    /// Monotonic frame sequence number.
    pub seq: u32,
    /// Cursor state at delta emit time. Always present so cursor-only motion
    /// (arrow keys, character input that doesn't change cell content) is
    /// faithfully tracked client-side without waiting for the next snapshot.
    pub cursor: Cursor,
    /// Entire rows that changed.
    pub dirty_rows: Vec<DirtyRow>,
}

/// Wire-level render frame, dispatched by the `kind` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderFrame {
    /// Full snapshot variant.
    Snapshot(FrameSnapshot),
    /// Differential update variant.
    Delta(FrameDelta),
}

/// Wire mode-change announcement (JSON text frame).
///
/// Emitted when `Term::mode()` changes between two `parser.advance` calls.
/// Carries a global `seq` consuming a slot in the frame sequence space,
/// matching the wire spec's "every server frame has seq" requirement.
/// Not stored in `frame_ring` — clients recover from missed mode frames
/// via the next snapshot's `modes` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeFrame {
    /// Frame discriminator. Always `ModeKind::Mode` (serializes to `"mode"`).
    pub kind: ModeKind,
    /// Monotonic frame sequence number.
    pub seq: u32,
    /// Mode names that transitioned from unset to set.
    pub added: Vec<String>,
    /// Mode names that transitioned from set to unset.
    pub removed: Vec<String>,
}

/// Discriminator value for [`ModeFrame`]. Unit enum so serde emits the
/// literal string `"mode"` on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeKind {
    /// The only valid value — denotes a mode-change announcement.
    Mode,
}

impl ModeFrame {
    /// Constructs a mode frame with the given seq from a transition.
    pub fn new(seq: u32, added: Vec<String>, removed: Vec<String>) -> Self {
        Self {
            kind: ModeKind::Mode,
            seq,
            added,
            removed,
        }
    }
}

/// Encodes a wire value as map-keyed MessagePack so field names are preserved
/// for the frontend's msgpackr decoder.
///
/// `rmp_serde::to_vec` defaults to array-encoded (positional) — compact but
/// not interoperable with the JS decoder, which keys by field name.
pub fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    rmp_serde::to_vec_named(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_round_trip_messagepack() {
        let run = Run {
            cols: 5,
            fg: Color::Indexed(1),
            bg: Color::Default,
            style: 0,
            text: "hello".to_string(),
            hyperlink_id: None,
        };
        let bytes = encode(&run).expect("encode");
        let decoded: Run = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(decoded, run);
    }

    #[test]
    fn color_variants_round_trip() {
        for c in [
            Color::Default,
            Color::Indexed(0),
            Color::Indexed(255),
            Color::Rgb(10, 20, 30),
        ] {
            let bytes = encode(&c).unwrap();
            let decoded: Color = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(decoded, c);
        }
    }

    #[test]
    fn cursor_round_trip() {
        let cur = Cursor {
            x: 5,
            y: 3,
            shape: CursorShape::Bar,
            visible: false,
        };
        let bytes = encode(&cur).unwrap();
        let decoded: Cursor = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, cur);
    }

    #[test]
    fn snapshot_with_two_rows_round_trip() {
        let snap = FrameSnapshot {
            seq: 42,
            cols: 80,
            rows: 24,
            cursor: Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block,
                visible: true,
            },
            rows_data: vec![
                Row {
                    runs: vec![Run {
                        cols: 3,
                        fg: Color::Default,
                        bg: Color::Default,
                        style: 0,
                        text: "abc".into(),
                        hyperlink_id: None,
                    }],
                },
                Row { runs: vec![] },
            ],
            reason: SnapshotReason::Initial,
            modes: vec![],
        };
        let bytes = encode(&snap).unwrap();
        let decoded: FrameSnapshot = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, snap);
    }

    #[test]
    fn delta_with_dirty_rows_round_trip() {
        let delta = FrameDelta {
            seq: 100,
            cursor: Cursor {
                x: 7,
                y: 2,
                shape: CursorShape::Block,
                visible: true,
            },
            dirty_rows: vec![
                DirtyRow {
                    row: 0,
                    runs: vec![],
                },
                DirtyRow {
                    row: 5,
                    runs: vec![Run {
                        cols: 2,
                        fg: Color::Rgb(255, 0, 0),
                        bg: Color::Default,
                        style: style::BOLD,
                        text: "あ".into(),
                        hyperlink_id: None,
                    }],
                },
            ],
        };
        let bytes = encode(&delta).unwrap();
        let decoded: FrameDelta = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, delta);
    }

    #[test]
    fn encode_produces_map_keyed_output() {
        let run = Run {
            cols: 1,
            fg: Color::Default,
            bg: Color::Default,
            style: 0,
            text: "x".into(),
            hyperlink_id: None,
        };
        let bytes = encode(&run).expect("encode");
        assert_eq!(
            bytes[0], 0x86,
            "expected fixmap (0x86) for 6-field map; array-encoded would be 0x96"
        );
        let decoded: Run = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(decoded, run);
    }

    #[test]
    fn render_frame_tagged_dispatch() {
        let snap = RenderFrame::Snapshot(FrameSnapshot {
            seq: 0,
            cols: 1,
            rows: 1,
            cursor: Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block,
                visible: true,
            },
            rows_data: vec![],
            reason: SnapshotReason::Initial,
            modes: vec![],
        });
        let bytes = encode(&snap).unwrap();
        let decoded: RenderFrame = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, snap);
    }

    #[test]
    fn mode_frame_serializes_with_kind_and_seq() {
        let m = ModeFrame::new(
            17,
            vec!["alt-screen".to_string()],
            vec!["mouse-vt200".to_string()],
        );
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""kind":"mode""#));
        assert!(json.contains(r#""seq":17"#));
        assert!(json.contains(r#""added":["alt-screen"]"#));
        assert!(json.contains(r#""removed":["mouse-vt200"]"#));
    }

    #[test]
    fn snapshot_modes_field_round_trips() {
        let snap = FrameSnapshot {
            seq: 0,
            cols: 80,
            rows: 24,
            cursor: Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block,
                visible: true,
            },
            rows_data: vec![],
            reason: SnapshotReason::Initial,
            modes: vec!["alt-screen".to_string(), "bracketed-paste".to_string()],
        };
        let bytes = encode(&snap).unwrap();
        let decoded: FrameSnapshot = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded.modes, ["alt-screen", "bracketed-paste"]);
    }
}
