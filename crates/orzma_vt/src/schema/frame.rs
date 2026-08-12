//! Frame vocabulary: what one emit hands to the renderer.

use crate::schema::{Cursor, Damage, DisplayOffset, Hyperlink, SelectionRange, ViCursor};

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
    /// faithfully tracked without waiting for the next snapshot.
    pub cursor: Cursor,
    /// Damage this delta repaints.
    pub damage: Damage,
    /// Hyperlinks referenced by this delta's damaged rows. The consumer
    /// merges these cumulatively into its own hyperlink map — this field
    /// itself is NOT cumulative; only the ids referenced by this delta's
    /// damaged rows are included.
    pub hyperlinks: Vec<Hyperlink>,
    /// Lines scrolled back from the live tail.
    pub display_offset: DisplayOffset,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    pub selection: Option<SelectionRange>,
}
