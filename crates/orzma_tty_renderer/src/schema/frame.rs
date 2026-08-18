use crate::schema::{
    Cursor, Hyperlink, Palette, ProjectedPlacement, Row, Run, SelectionRange, ViCursor,
};
use bevy::ecs::{entity::Entity, event::EntityEvent};

/// Full snapshot of the visible viewport.
///
/// Carries all data needed to render the screen without prior state.
#[derive(Debug, Clone, PartialEq, EntityEvent)]
pub struct FrameSnapshot {
    #[event_target]
    pub entity: Entity,
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
    /// Cursor state at emit time.
    pub cursor: Cursor,
    /// Row contents (length == rows).
    pub rows_data: Vec<Row<Run>>,
    /// Why this snapshot was emitted (Initial, Reconnect, Resize, Lagged).
    pub reason: SnapshotReason,
    /// Currently active wire modes (e.g. "alt-screen", "mouse-vt200").
    pub modes: Vec<String>,
    /// Hyperlinks referenced by row Runs.
    pub hyperlinks: Vec<Hyperlink>,
    /// Lines scrolled back from the live tail. `0` = at live tail.
    pub display_offset: u32,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    pub selection: Option<SelectionRange>,
    /// Viewport-projected webview placements — the complete list for
    /// this frame. Absence means "not visible", not "unmounted".
    pub placements: Vec<ProjectedPlacement>,
    /// The live palette symbolic colors resolve against. Snapshot-only:
    /// a palette override repaints fully, so no delta outlives the
    /// table it was rendered with.
    pub palette: Palette,
}

/// Differential update relative to the prior frame.
#[derive(Debug, Clone, PartialEq, EntityEvent)]
pub struct FrameDelta {
    pub entity: Entity,
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
    pub display_offset: u32,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    pub selection: Option<SelectionRange>,
    /// Viewport-projected webview placements — the complete list for
    /// this frame. Absence means "not visible", not "unmounted".
    pub placements: Vec<ProjectedPlacement>,
}

/// A dirty row entry inside a `FrameDelta`.
///
/// `runs` represents the entire row (full row replacement, not partial).
#[derive(Debug, Clone, PartialEq)]
pub struct DirtyRow {
    /// Row index, zero-based from the top of the screen.
    pub row: u16,
    /// Full set of runs for the row.
    pub runs: Vec<Run>,
}

/// Reason a snapshot was sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
