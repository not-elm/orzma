use alacritty_terminal::vte::ansi::Hyperlink;

use crate::{
    cursor::{Cursor, ViCursor},
    damage::DirtyRows,
};

pub enum Frame {
    Snapshot(),
    Delta(),
}

#[derive(Debug)]
pub struct FrameSnapshot {}

/// Differential update relative to the prior frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, EntityEvent)]
pub struct FrameDelta {
    /// Monotonic frame sequence number.
    pub seq: u32,
    /// Cursor state at delta emit time. Always present so cursor-only motion
    /// (arrow keys, character input that doesn't change cell content) is
    /// faithfully tracked client-side without waiting for the next snapshot.
    pub cursor: Cursor,
    /// Entire rows that changed.
    pub dirty_rows: DirtyRows,
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

impl FrameDelta {
    pub fn dirty_rows(&self) -> Vec<_> {
        self.dirty_rows
    }

    pub fn dirty_rows_mut(&mut self) -> &mut Vec<DirtyRow> {
        &mut self.dirty_rows
    }
}
