use crate::schema::{Cursor, Hyperlink, Row, Run, SelectionRange, ViCursor};
use bevy::ecs::{entity::Entity, event::EntityEvent};
use serde::{Deserialize, Serialize};

/// Full snapshot of the visible viewport at a given seq.
///
/// Carries all data needed to render the screen without prior state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, EntityEvent)]
pub struct FrameSnapshot {
    #[event_target]
    pub entity: Entity,
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
    /// Cumulative lines trimmed from the top of scrollback (monotonic;
    /// advances only on history-destroying folds — spec §3).
    #[serde(default)]
    pub history_base: u64,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionRange>,
    /// Terminal default background color from `term.colors()[NamedColor::Background]`
    /// (OSC 11). `[0, 0, 0]` when no override is present.
    #[serde(default)]
    pub default_bg: [u8; 3],
}

/// Differential update relative to the prior frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, EntityEvent)]
pub struct FrameDelta {
    pub entity: Entity,
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
    /// Total scrollback history line count (upper bound for display_offset).
    #[serde(default)]
    pub history_size: u32,
    /// Cumulative lines trimmed from the top of scrollback (monotonic;
    /// advances only on history-destroying folds — spec §3).
    #[serde(default)]
    pub history_base: u64,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionRange>,
}

/// A dirty row entry inside a `FrameDelta`.
///
/// `runs` represents the entire row (full row replacement, not partial).
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
