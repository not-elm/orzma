//! Terminal wire DTOs. Serde-capable and rendering-agnostic.
//! Produced by the daemon and sent over UDS; consumed by the Bevy renderer.

use crate::color::RgbaColor;
use serde::{Deserialize, Serialize};

/// Bit 0 of the packed `cursor_style` u32 — set when the cursor should be drawn.
pub const CURSOR_VISIBLE_BIT: u32 = 1;

/// Vi-mode cursor position in viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViCursor {
    /// Viewport row; `-1` when `in_scrollback` is true.
    pub row: i16,
    /// Viewport column (0-based).
    pub column: u16,
    /// True when the vi cursor is above the viewport (in scrollback).
    pub in_scrollback: bool,
}

/// Cursor state at snapshot time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// Column position (0-based).
    pub x: u16,
    /// Row position (0-based).
    pub y: u16,
    /// Visual shape selected by DECSCUSR.
    pub shape: CursorShape,
    /// True when DECSCUSR selects a blinking variant.
    pub blinking: bool,
    /// True when the cursor should be rendered (DECTCEM AND shape != Hidden).
    pub visible: bool,
}

impl Cursor {
    /// Packs visibility/shape/blink into the GPU `cursor_style` u32.
    pub fn pack_cursor_style(&self) -> u32 {
        let visible = if self.visible { CURSOR_VISIBLE_BIT } else { 0 };
        let shape = match self.shape {
            CursorShape::Block => 0u32,
            CursorShape::Underline => 1,
            CursorShape::Bar => 2,
        };
        let blinking = if self.blinking { 1u32 } else { 0 };
        visible | (shape << 1) | (blinking << 3)
    }
}

/// Terminal cursor shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Block cursor.
    #[default]
    Block,
    /// Underline cursor.
    Underline,
    /// Bar (vertical line) cursor.
    Bar,
}

/// OSC 8 hyperlink: server-assigned wire id → URI mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hyperlink {
    /// Monotonic u32 wire id assigned server-side.
    pub id: HyperlinkId,
    /// The hyperlink target URI.
    pub uri: HyperlinkUri,
}

/// Wire-level monotonic hyperlink id.
///
/// # Invariants
///
/// Callers outside the interner MUST NOT construct `HyperlinkId(0)`; it is the
/// universal "no hyperlink" sentinel.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HyperlinkId(pub u32);

/// OSC 8 hyperlink target URI.
#[derive(Clone, Eq, PartialEq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HyperlinkUri(String);

impl HyperlinkUri {
    /// Wraps a string as a hyperlink URI.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A row of runs ordered left-to-right.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// Runs in left-to-right column order.
    pub runs: Vec<Run>,
}

/// A run of cells sharing identical fg/bg/style attributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Total column span (sum of grapheme cluster widths in `text`).
    pub cols: u16,
    /// Foreground color (sRGB).
    pub fg: RgbaColor,
    /// Background color (sRGB).
    pub bg: RgbaColor,
    /// Style bitmask. rmp-serde picks the smallest msgpack int form per value.
    pub style: u16,
    /// UTF-8 text; clients position graphemes by East Asian Width.
    /// Wide-char spacers (alacritty's internal trailing-cell markers) are
    /// absorbed server-side and do not appear in `text`.
    pub text: String,
    /// Hyperlink id (OSC 8), if any.
    pub hyperlink_id: Option<HyperlinkId>,
}

/// A dirty row entry inside a `FrameDelta` (full row replacement).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirtyRow {
    /// Row index, zero-based from the top of the screen.
    pub row: u16,
    /// Full set of runs for the row.
    pub runs: Vec<Run>,
}

/// Reason a snapshot was sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotReason {
    /// Initial connect.
    #[default]
    Initial,
    /// Reconnect with no replay available.
    Reconnect,
    /// Receiver fell too far behind the broadcast.
    Lagged,
    /// Terminal was resized.
    Resize,
}

/// Active selection range in viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRange {
    /// Anchor point where the selection started.
    pub start: ViewportPoint,
    /// Cursor point where the selection currently ends.
    pub end: ViewportPoint,
    /// Selection geometry (char-wise or line-wise).
    pub kind: SelectionKind,
}

/// Full snapshot of the visible viewport (pure wire type — no `Entity`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameSnapshot {
    /// Monotonic emission sequence number.
    pub seq: u32,
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
    /// Cursor state at emit time.
    pub cursor: Cursor,
    /// Row contents (length == rows).
    pub rows_data: Vec<Row>,
    /// Why this snapshot was emitted.
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
    /// Vi-mode cursor (active only in copy mode). Absent in normal mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionRange>,
}

/// Differential update relative to the prior frame (pure wire type — no `Entity`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameDelta {
    /// Monotonic frame sequence number.
    pub seq: u32,
    /// Cursor state at delta emit time.
    pub cursor: Cursor,
    /// Entire rows that changed.
    pub dirty_rows: Vec<DirtyRow>,
    /// Hyperlinks referenced by this delta's dirty rows.
    pub hyperlinks: Vec<Hyperlink>,
    /// Lines scrolled back from the live tail. `0` = at live tail.
    #[serde(default)]
    pub display_offset: u32,
    /// Vi-mode cursor (active only in copy mode). Absent in normal mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionRange>,
}

/// The return type of a frame emit — either a full snapshot or an incremental delta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    /// Full viewport snapshot.
    Snapshot(FrameSnapshot),
    /// Incremental row-level delta.
    Delta(FrameDelta),
}

/// A point in viewport coordinates.
///
/// Endpoints resolving to scrollback are clamped to `row = -1` (above) or
/// `row = rows` (below) so clients can still draw the visible portion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportPoint {
    /// Viewport row; clamped to `-1` / `rows` for scrollback endpoints.
    pub row: i16,
    /// Viewport column (0-based).
    pub column: u16,
}

/// Selection geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionKind {
    /// Character-wise selection.
    Char,
    /// Line-wise selection.
    Line,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_snapshot_msgpack_round_trip() {
        let s = FrameSnapshot {
            seq: 1,
            cols: 2,
            rows: 1,
            cursor: Cursor::default(),
            rows_data: vec![],
            reason: SnapshotReason::Initial,
            modes: vec![],
            hyperlinks: vec![],
            display_offset: 0,
            history_size: 0,
            vi_cursor: None,
            selection: None,
        };
        let b = rmp_serde::to_vec(&s).unwrap();
        let back: FrameSnapshot = rmp_serde::from_slice(&b).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn run_msgpack_round_trip_preserves_rgba() {
        let run = Run {
            cols: 3,
            fg: RgbaColor::srgb(10, 20, 30),
            bg: RgbaColor::BLACK,
            style: 5,
            text: "abc".to_string(),
            hyperlink_id: Some(HyperlinkId(7)),
        };
        let bytes = rmp_serde::to_vec(&run).expect("encode");
        let back: Run = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(run, back);
    }

    #[test]
    fn pack_cursor_style_sets_visible_bit() {
        let c = Cursor {
            x: 1,
            y: 2,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
        };
        assert_eq!(
            c.pack_cursor_style() & CURSOR_VISIBLE_BIT,
            CURSOR_VISIBLE_BIT
        );
    }

    #[test]
    fn pack_cursor_style_encodes_shape_and_blink() {
        let c = Cursor {
            visible: true,
            shape: CursorShape::Bar,
            blinking: true,
            ..Default::default()
        };
        assert_eq!(c.pack_cursor_style(), 13);
    }
}
