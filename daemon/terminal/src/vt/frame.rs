//! Wire protocol types for snapshot/delta frames.

use crate::vt::hyperlink::{HyperlinkUri, HyperlinkWireId};
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
    /// Column position (0-based).
    pub x: u16,
    /// Row position (0-based).
    pub y: u16,
    /// Visual shape selected by DECSCUSR.
    pub shape: CursorShape,
    /// True when DECSCUSR selects a blinking variant. Steady variants
    /// (`\033[2 q`, `\033[4 q`, `\033[6 q`) set this to false.
    pub blinking: bool,
    /// True when the cursor should be rendered. Gated by DECTCEM
    /// (`TermMode::SHOW_CURSOR`) AND DECSCUSR shape != Hidden.
    pub visible: bool,
}

/// OSC 8 hyperlink: server-assigned wire id → URI mapping.
///
/// Wire id is a monotonic u32 assigned by `crate::vt::hyperlink::HyperlinkInterner`
/// keyed by `(alacritty_id, uri)`. Cells reference these via [`Run::hyperlink_id`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hyperlink {
    /// Monotonic u32 wire id assigned server-side.
    pub id: HyperlinkWireId,
    /// The hyperlink target URI.
    pub uri: HyperlinkUri,
}

/// Style bitmask constants for [`Run::style`].
///
/// Bits 64 and 128 are reserved.
pub(super) mod style {
    /// Bold weight.
    pub(in crate::vt) const BOLD: u8 = 1;
    /// Italic style.
    pub(in crate::vt) const ITALIC: u8 = 2;
    /// Underline decoration.
    pub(in crate::vt) const UNDERLINE: u8 = 4;
    /// Strikethrough decoration.
    pub(in crate::vt) const STRIKE: u8 = 8;
    /// Reversed foreground/background.
    pub(in crate::vt) const REVERSE: u8 = 16;
    /// Dim/faint weight.
    pub(in crate::vt) const DIM: u8 = 32;
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
    /// Style bitmask (see the `style` module).
    pub style: u8,
    /// UTF-8 text; the client uses Unicode East Asian Width to position each
    /// grapheme cluster within the run.
    pub text: String,
    /// Hyperlink id (OSC 8); always `None` until Phase 3.
    pub hyperlink_id: Option<HyperlinkWireId>,
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

/// Full snapshot of the visible viewport at a given seq.
///
/// Carries all data needed to render the screen without prior state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameSnapshot {
    /// Monotonic emission sequence; matches the encoded `seq` in the ring.
    pub seq: u32,
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
    /// Cursor state at emit time.
    pub cursor: Cursor,
    /// Row contents (length == rows).
    pub rows_data: Vec<Row>,
    /// Why this snapshot was emitted (Initial, Reconnect, Resize, Lagged).
    pub reason: SnapshotReason,
    /// Currently active wire modes (e.g. "alt-screen", "mouse-vt200").
    pub modes: Vec<String>,
    /// Hyperlinks referenced by row Runs.
    pub hyperlinks: Vec<Hyperlink>,
    /// Lines scrolled back from the live tail. `0` = at live tail.
    #[serde(default)]
    pub display_offset: u32,
    /// Total scrollback history line count (upper bound for display_offset).
    #[serde(default)]
    pub history_size: u32,
    /// Optional wall-clock epoch microseconds when this frame was produced
    /// by the bridge. Filled when `OZMUX_PERF_PRODUCED_AT=1`. Tail-optional
    /// to keep existing wire fixtures byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_at_us: Option<u64>,
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
    /// Hyperlinks referenced by this delta's dirty rows. Clients merge
    /// cumulatively into their hyperlink Map. NOT cumulative on the server —
    /// only the ids referenced by this delta's dirty rows are included.
    pub hyperlinks: Vec<Hyperlink>,
    /// Lines scrolled back from the live tail. `0` = at live tail.
    #[serde(default)]
    pub display_offset: u32,
    /// Optional wall-clock epoch microseconds when this frame was produced
    /// by the bridge. Filled when `OZMUX_PERF_PRODUCED_AT=1`. Tail-optional
    /// to keep existing wire fixtures byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_at_us: Option<u64>,
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
            blinking: false,
            visible: false,
        };
        let bytes = encode(&cur).unwrap();
        let decoded: Cursor = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, cur);
    }

    #[test]
    fn cursor_blinking_field_round_trip() {
        let cur = Cursor {
            x: 1,
            y: 2,
            shape: CursorShape::Underline,
            blinking: true,
            visible: true,
        };
        let bytes = encode(&cur).unwrap();
        let decoded: Cursor = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, cur);
    }

    #[test]
    fn hyperlink_round_trip() {
        let h = Hyperlink {
            id: HyperlinkWireId(42),
            uri: HyperlinkUri::new("https://example.com"),
        };
        let bytes = encode(&h).unwrap();
        let decoded: Hyperlink = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, h);
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
                blinking: false,
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
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 0,
            produced_at_us: None,
        };
        let bytes = encode(&snap).unwrap();
        let decoded: FrameSnapshot = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, snap);
    }

    #[test]
    fn snapshot_hyperlinks_field_round_trip() {
        let snap = FrameSnapshot {
            seq: 1,
            cols: 1,
            rows: 1,
            cursor: Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block,
                blinking: false,
                visible: true,
            },
            rows_data: vec![Row { runs: vec![] }],
            reason: SnapshotReason::Initial,
            modes: vec![],
            hyperlinks: vec![Hyperlink {
                id: HyperlinkWireId(7),
                uri: HyperlinkUri::new("https://ozmux.example"),
            }],
            display_offset: 0,
            history_size: 0,
            produced_at_us: None,
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
                blinking: false,
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
            hyperlinks: vec![],
            display_offset: 0,
            produced_at_us: None,
        };
        let bytes = encode(&delta).unwrap();
        let decoded: FrameDelta = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, delta);
    }

    #[test]
    fn delta_hyperlinks_field_round_trip() {
        let delta = FrameDelta {
            seq: 2,
            cursor: Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block,
                blinking: false,
                visible: true,
            },
            dirty_rows: vec![],
            hyperlinks: vec![Hyperlink {
                id: HyperlinkWireId(1),
                uri: HyperlinkUri::new("https://example.org"),
            }],
            display_offset: 0,
            produced_at_us: None,
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
                blinking: false,
                visible: true,
            },
            rows_data: vec![],
            reason: SnapshotReason::Initial,
            modes: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 0,
            produced_at_us: None,
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
                blinking: false,
                visible: true,
            },
            rows_data: vec![],
            reason: SnapshotReason::Initial,
            modes: vec!["alt-screen".to_string(), "bracketed-paste".to_string()],
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 0,
            produced_at_us: None,
        };
        let bytes = encode(&snap).unwrap();
        let decoded: FrameSnapshot = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded.modes, ["alt-screen", "bracketed-paste"]);
    }

    #[test]
    fn delta_encodes_display_offset() {
        let delta = FrameDelta {
            seq: 7,
            cursor: Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block,
                blinking: false,
                visible: true,
            },
            dirty_rows: vec![],
            hyperlinks: vec![],
            display_offset: 12,
            produced_at_us: None,
        };
        let bytes = encode(&RenderFrame::Delta(delta.clone())).unwrap();
        let decoded: RenderFrame = rmp_serde::from_slice(&bytes).unwrap();
        let RenderFrame::Delta(out) = decoded else {
            panic!("expected Delta")
        };
        assert_eq!(out.display_offset, 12);
    }

    #[test]
    fn snapshot_encodes_display_offset_and_history_size() {
        let snap = FrameSnapshot {
            seq: 1,
            cols: 1,
            rows: 1,
            cursor: Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block,
                blinking: false,
                visible: true,
            },
            rows_data: vec![],
            reason: SnapshotReason::Initial,
            modes: vec![],
            hyperlinks: vec![],
            display_offset: 5,
            history_size: 100,
            produced_at_us: None,
        };
        let bytes = encode(&RenderFrame::Snapshot(snap.clone())).unwrap();
        let decoded: RenderFrame = rmp_serde::from_slice(&bytes).unwrap();
        let RenderFrame::Snapshot(out) = decoded else {
            panic!("expected Snapshot")
        };
        assert_eq!(out.display_offset, 5);
        assert_eq!(out.history_size, 100);
    }

    fn sample_snapshot() -> FrameSnapshot {
        FrameSnapshot {
            seq: 0,
            cols: 80,
            rows: 24,
            cursor: Cursor {
                x: 0,
                y: 0,
                shape: CursorShape::Block,
                blinking: false,
                visible: true,
            },
            rows_data: vec![],
            reason: SnapshotReason::Initial,
            modes: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 0,
            produced_at_us: None,
        }
    }

    #[test]
    fn frame_snapshot_round_trip_with_produced_at() {
        let mut snap = sample_snapshot();
        snap.produced_at_us = Some(1_700_000_000_000_000);
        let bytes = encode(&snap).unwrap();
        let back: FrameSnapshot = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back.produced_at_us, Some(1_700_000_000_000_000));
    }

    #[test]
    fn frame_snapshot_round_trip_without_produced_at_is_byte_identical() {
        let snap = sample_snapshot();
        assert!(snap.produced_at_us.is_none());
        let bytes_a = encode(&snap).unwrap();
        let back: FrameSnapshot = rmp_serde::from_slice(&bytes_a).unwrap();
        let bytes_b = encode(&back).unwrap();
        assert_eq!(bytes_a, bytes_b, "skip_serializing_if must omit None field");
    }
}
