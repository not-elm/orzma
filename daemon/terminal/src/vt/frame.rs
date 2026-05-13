//! Wire protocol types for snapshot/delta frames.

use serde::{Deserialize, Serialize};

/// Foreground/background color.
/// Wire: Default = nil, Indexed = uint8, Rgb = 3-tuple of uint8.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Color {
    Default,
    Indexed(u8),
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

/// Style bitmask. bold=1, italic=2, underline=4, strike=8, reverse=16, dim=32.
/// bits 64 / 128 are reserved (no current meaning).
#[allow(dead_code)] // consumed by encoder (Task 6+) and tests
pub mod style {
    pub const BOLD: u8 = 1;
    pub const ITALIC: u8 = 2;
    pub const UNDERLINE: u8 = 4;
    pub const STRIKE: u8 = 8;
    pub const REVERSE: u8 = 16;
    pub const DIM: u8 = 32;
}

/// A run of cells sharing identical fg/bg/style attributes.
///
/// `cols` = total column span (sum of grapheme cluster widths from `text`).
/// `text` = UTF-8 string; client uses Unicode East Asian Width to position
/// each grapheme cluster within the run. wide-char spacers (alacritty
/// internal) are absorbed server-side and do NOT appear in `text`.
/// `hyperlink_id` = Phase 1/2 always None; Phase 3 (OSC 8) sets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub cols: u16,
    pub fg: Color,
    pub bg: Color,
    pub style: u8,
    pub text: String,
    pub hyperlink_id: Option<u32>,
}

/// A row of runs (left-to-right).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub runs: Vec<Run>,
}

/// A dirty row entry inside a Delta frame.
/// `runs` represents the entire row (full row replacement, not partial).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyRow {
    pub row: u16,
    pub runs: Vec<Run>,
}

/// Why was a snapshot sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    Initial,
    Reconnect,
    Lagged,
    Resize,
}

/// Full screen state snapshot. Sent on connect, reconnect (no replay), lagged,
/// or resize. `kind` discriminant is serialized via serde `tag` on RenderFrame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameSnapshot {
    pub seq: u32,
    pub cols: u16,
    pub rows: u16,
    pub cursor: Cursor,
    pub rows_data: Vec<Row>,
    pub reason: SnapshotReason,
}

/// Differential update. `dirty_rows` contains entire rows that changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameDelta {
    pub seq: u32,
    pub dirty_rows: Vec<DirtyRow>,
}

/// Render frame tagged union dispatch shape (wire-level `kind` discriminator).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderFrame {
    Snapshot(FrameSnapshot),
    Delta(FrameDelta),
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
        let bytes = rmp_serde::to_vec(&run).expect("encode");
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
            let bytes = rmp_serde::to_vec(&c).unwrap();
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
        let bytes = rmp_serde::to_vec(&cur).unwrap();
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
        };
        let bytes = rmp_serde::to_vec(&snap).unwrap();
        let decoded: FrameSnapshot = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, snap);
    }

    #[test]
    fn delta_with_dirty_rows_round_trip() {
        let delta = FrameDelta {
            seq: 100,
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
        let bytes = rmp_serde::to_vec(&delta).unwrap();
        let decoded: FrameDelta = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, delta);
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
        });
        let bytes = rmp_serde::to_vec(&snap).unwrap();
        let decoded: RenderFrame = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, snap);
    }
}
