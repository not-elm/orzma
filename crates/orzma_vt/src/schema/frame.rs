//! Frame vocabulary: what one emit hands to the renderer.

use crate::schema::{
    Cursor, DisplayOffset, GridSize, Hyperlink, Palette, ProjectedPlacement, Row, SelectionRange,
    ViCursor, ViewportLine, VtModes,
};

/// One emitted frame: a full repaint or a differential update.
///
/// Staged [`crate::schema::Damage::Full`] emits a [`Frame::Snapshot`];
/// staged row damage emits a [`Frame::Delta`].
#[derive(Debug)]
pub enum Frame {
    /// A full repaint of the visible viewport.
    Snapshot(FrameSnapshot),
    /// A differential update relative to the prior frame.
    Delta(FrameDelta),
}

/// A full repaint: everything the renderer needs to draw the visible
/// viewport from scratch.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSnapshot {
    /// Wrapping emission sequence number, shared with deltas.
    pub seq: u32,
    /// Grid dimensions; a resize reaches the renderer through this.
    pub size: GridSize,
    /// Full viewport contents, top to bottom.
    pub rows: Vec<Row>,
    /// Cursor state at emit time.
    pub cursor: Cursor,
    /// Lines scrolled back from the live tail.
    pub display_offset: DisplayOffset,
    /// Viewport-projected webview placements at emit time — the
    /// complete list, not a diff. A placement present here is drawn at
    /// its position; one absent is not visible this frame, which is
    /// not an unmount. Every placement-state change stages an
    /// emission, so an otherwise-empty delta still carries the moved
    /// list.
    pub placements: Vec<ProjectedPlacement>,
    /// Absolute terminal-mode state at emit time. Snapshot-only: a
    /// mode flip consumers gate on stages full damage, so no delta
    /// outlives its mode set.
    pub modes: VtModes,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    pub selection: Option<SelectionRange>,
    /// Hyperlinks referenced by `rows`. Reserved: empty until the
    /// hyperlink interner is ported, which is safe while
    /// [`crate::schema::Run::hyperlink_id`] is always `None`.
    pub hyperlinks: Vec<Hyperlink>,
    /// The live palette symbolic colors resolve against. Carried only
    /// by snapshots: a palette override repaints fully, so no delta
    /// ever outlives the table it was rendered with.
    pub palette: Palette,
}

/// A differential update relative to the prior frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameDelta {
    /// Wrapping emission sequence number, shared with snapshots.
    pub seq: u32,
    /// The dirty rows this delta repaints, ascending by line. May be
    /// empty — the metadata below is still current.
    pub dirty_rows: Vec<DirtyRow>,
    /// Cursor state at delta emit time. Always present so cursor-only motion
    /// (arrow keys, character input that doesn't change cell content) is
    /// faithfully tracked without waiting for the next snapshot.
    pub cursor: Cursor,
    /// Lines scrolled back from the live tail.
    pub display_offset: DisplayOffset,
    /// Viewport-projected webview placements at emit time — the
    /// complete list, not a diff. A placement present here is drawn at
    /// its position; one absent is not visible this frame, which is
    /// not an unmount. Every placement-state change stages an
    /// emission, so an otherwise-empty delta still carries the moved
    /// list.
    pub placements: Vec<ProjectedPlacement>,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    pub selection: Option<SelectionRange>,
    /// Hyperlinks referenced by `dirty_rows`. Reserved: empty until
    /// the hyperlink interner is ported, which is safe while
    /// [`crate::schema::Run::hyperlink_id`] is always `None`.
    pub hyperlinks: Vec<Hyperlink>,
}

/// One repainted viewport row inside a [`FrameDelta`].
#[derive(Debug, Clone, PartialEq)]
pub struct DirtyRow {
    /// The viewport row the contents repaint.
    pub line: ViewportLine,
    /// The row contents.
    pub contents: Row,
}
