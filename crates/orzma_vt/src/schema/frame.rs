use crate::schema::{Cursor, DirtyRows, Hyperlink, SelectionRange, ViCursor};

pub enum Frame {
    Snapshot(FrameSnapshot),
    Delta(FrameDelta),
}

#[derive(Debug)]
pub struct FrameSnapshot {}

/// Differential update relative to the prior frame.
#[derive(Debug, Clone, PartialEq)]
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
    pub display_offset: u32,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    pub selection: Option<SelectionRange>,
}
