//! Frame assembly: turning device state into what one emit hands the
//! renderer.
//!
//! The pieces of a frame come from three places — the active screen's
//! rows and cursor, the placement store's projection, and the device's
//! palette — and they are read through one shared borrow so a frame
//! describes a single instant. Turning cells into runs is
//! [`Row::to_runs`](crate::screen::grid::row::Row); deciding whether a
//! frame is a snapshot belongs to the damage that produced it.

use crate::schema::{
    Cursor, GridSize, Hyperlink, Palette, ProjectedPlacement, Row, Run, SelectionRange, ViCursor,
    ViewportLine,
};
use crate::screen::Screen;
use crate::screen::viewport::DisplayOffset;

/// One emitted frame: a full repaint or a differential update.
///
/// Staged [`crate::damage::Damage::Full`] emits a [`Frame::Snapshot`];
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
    /// Grid dimensions; a resize reaches the renderer through this.
    pub size: GridSize,
    /// Full viewport contents, top to bottom.
    pub rows: Vec<Row<Run>>,
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
    pub contents: Row<Run>,
}

// NOTE: `#[expect]` is impractical here — the tests below call the
// constructor, so `dead_code` fires in the lib build but not in the test
// build, leaving the expectation unfulfilled there.
#[allow(
    dead_code,
    reason = "`Frame::emit` reaches the constructor once the delta path lands"
)]
impl FrameSnapshot {
    /// Builds a full repaint of the visible viewport.
    fn new(screen: &Screen, placements: Vec<ProjectedPlacement>, palette: Palette) -> Self {
        let size = screen.grid_size();
        Self {
            size,
            rows: (0..size.rows)
                .map(|line| screen.viewport_row(ViewportLine(line)).to_runs())
                .collect(),
            cursor: screen.cursor(),
            display_offset: screen.display_offset(),
            placements,
            vi_cursor: None,
            selection: None,
            hyperlinks: Vec::new(),
            palette,
        }
    }
}

#[cfg(test)]
mod tests {
    mod snapshot {
        use crate::device::DeviceState;
        use crate::frame::FrameSnapshot;
        use crate::placement::PlacementStore;
        use crate::schema::{Color, CursorShape, GridColumn, GridLine, GridSize, Palette};

        fn device() -> DeviceState {
            DeviceState::new(GridSize { cols: 4, rows: 3 }, 10)
        }

        fn snapshot(device: &DeviceState, placements: &PlacementStore) -> FrameSnapshot {
            FrameSnapshot::new(
                device.active(),
                placements.project(
                    device.modes().active_screen,
                    device.display_offset(),
                    device.grid_size(),
                ),
                device.palette(),
            )
        }

        /// Asserts that a snapshot carries every viewport row, each spanning
        /// the full width.
        ///
        /// Case: the renderer draws a terminal from scratch and has nothing
        /// but this frame to paint the whole window from.
        #[test]
        fn a_snapshot_covers_every_viewport_row_at_full_width() {
            let mut device = device();
            device.active_mut().print('a');
            let snap = snapshot(&device, &PlacementStore::new());
            assert_eq!(snap.size, GridSize { cols: 4, rows: 3 });
            assert_eq!(snap.rows.len(), 3);
            for row in &snap.rows {
                assert_eq!(row.iter().map(|run| u32::from(run.cols)).sum::<u32>(), 4);
            }
            assert_eq!(snap.rows[0][0].text, "a   ");
        }

        /// Asserts that a snapshot carries the write cursor and the live
        /// palette.
        ///
        /// Case: a shell prints its prompt, and the frame has to place the
        /// caret after it and resolve the prompt's indexed colors.
        #[test]
        fn a_snapshot_carries_the_cursor_and_the_live_palette() {
            let mut device = device();
            device.active_mut().print('a');
            let snap = snapshot(&device, &PlacementStore::new());
            assert_eq!(snap.cursor.point.line, GridLine(0));
            assert_eq!(snap.cursor.point.column, GridColumn(1));
            assert_eq!(snap.cursor.shape, CursorShape::Block);
            assert!(snap.cursor.visible);
            assert_eq!(snap.palette, Palette::default());
            assert_eq!(snap.display_offset, device.display_offset());
        }

        /// Asserts that the fields whose features have not landed are
        /// emitted empty rather than omitted.
        ///
        /// The agreed policy keeps them in every frame: an absent placement
        /// means "not visible this frame", not "unmounted", so the list has
        /// to be present even while the store is empty.
        ///
        /// Case: a terminal running before webviews, OSC 8, selection, or vi
        /// mode exist emits its first frame.
        #[test]
        fn a_snapshot_reserves_the_fields_their_features_have_not_reached() {
            let snap = snapshot(&device(), &PlacementStore::new());
            assert!(snap.placements.is_empty());
            assert!(snap.hyperlinks.is_empty());
            assert_eq!(snap.vi_cursor, None);
            assert_eq!(snap.selection, None);
        }

        /// Asserts that a blank screen's rows are one full-width run of
        /// default cells.
        ///
        /// Case: a terminal spawns and its very first frame paints an empty
        /// window.
        #[test]
        fn a_blank_screen_emits_one_default_run_per_row() {
            let snap = snapshot(&device(), &PlacementStore::new());
            for row in &snap.rows {
                assert_eq!(row.len(), 1);
                assert_eq!(row[0].text, "    ");
                assert_eq!(row[0].fg, Color::DefaultForeground);
                assert_eq!(row[0].bg, Color::DefaultBackground);
            }
        }
    }
}
