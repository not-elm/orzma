//! Frame assembly: turning device state into what one emit hands the
//! renderer.
//!
//! The pieces of a frame come from three places — the active screen's
//! rows and cursor, the placement store's projection, and the device's
//! palette — and they are read through one shared borrow so a frame
//! describes a single instant. Turning cells into runs is
//! [`Row::to_runs`](crate::screen::grid::row::Row); deciding whether a
//! frame is a snapshot belongs to the damage that produced it.

use crate::damage::{DamageRows, StagedDamage};
use crate::device::DeviceState;
use crate::placement::PlacementStore;
use crate::schema::{
    Cursor, GridSize, Hyperlink, Palette, ProjectedPlacement, Row, Run, SelectionRange, ViCursor,
    ViewportLine,
};
use crate::screen::viewport::DisplayOffset;

/// One emitted frame: a full repaint or a differential update.
///
/// Staged [`crate::damage::StagedDamage::Full`] emits a [`Frame::Snapshot`];
/// staged row damage emits a [`Frame::Delta`].
#[derive(Debug)]
pub enum Frame {
    /// A full repaint of the visible viewport.
    Snapshot(FrameSnapshot),
    /// A differential update relative to the prior frame.
    Delta(FrameDelta),
}

impl Frame {
    /// Builds the frame the staged damage calls for.
    pub(crate) fn emit(
        damage: StagedDamage,
        device: &DeviceState,
        placements: &PlacementStore,
    ) -> Self {
        match damage {
            StagedDamage::Full => Self::Snapshot(FrameSnapshot::new(device, placements)),
            StagedDamage::Delta(rows) => Self::Delta(FrameDelta::new(&rows, device, placements)),
        }
    }
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

impl FrameSnapshot {
    /// Builds a full repaint of the visible viewport.
    ///
    /// # Invariants
    ///
    /// The rows, the cursor, the offset, and the projection all come
    /// from one borrow of `device`, so a snapshot describes a single
    /// instant. Projecting placements outside and passing the list in
    /// would let a caller pair a stale offset with fresh rows.
    fn new(device: &DeviceState, placements: &PlacementStore) -> Self {
        let screen = device.active();
        let size = screen.grid_size();
        let display_offset = screen.display_offset();
        Self {
            size,
            rows: (0..size.rows)
                .map(|line| screen.viewport_row(ViewportLine(line)).to_runs())
                .collect(),
            cursor: screen.cursor(),
            display_offset,
            placements: placements.project(device.modes().active_screen, display_offset, size),
            vi_cursor: None,
            selection: None,
            hyperlinks: Vec::new(),
            palette: device.palette(),
        }
    }
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

impl FrameDelta {
    /// Builds a differential update for the damaged rows.
    ///
    /// # Invariants
    ///
    /// Every staged row is below the current `size.rows` and names a
    /// line in the active screen's viewport basis. Any intervening
    /// offset, size, or active-screen change merges
    /// [`StagedDamage::Full`](crate::damage::StagedDamage::Full), which replaces
    /// these rows outright, so a delta never outlives the viewport its
    /// rows were staged against.
    ///
    /// The rows, the cursor, the offset, and the projection all come
    /// from one borrow of `device`, so a delta describes a single
    /// instant.
    fn new(rows: &DamageRows, device: &DeviceState, placements: &PlacementStore) -> Self {
        let screen = device.active();
        let display_offset = screen.display_offset();
        Self {
            dirty_rows: rows
                .iter()
                .map(|&line| DirtyRow {
                    line,
                    contents: screen.viewport_row(line).to_runs(),
                })
                .collect(),
            cursor: screen.cursor(),
            display_offset,
            placements: placements.project(
                device.modes().active_screen,
                display_offset,
                screen.grid_size(),
            ),
            vi_cursor: None,
            selection: None,
            hyperlinks: Vec::new(),
        }
    }
}

/// One repainted viewport row inside a [`FrameDelta`].
#[derive(Debug, Clone, PartialEq)]
pub struct DirtyRow {
    /// The viewport row the contents repaint.
    pub line: ViewportLine,
    /// The row contents.
    pub contents: Row<Run>,
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

        /// Asserts that a snapshot carries every viewport row, each spanning
        /// the full width.
        ///
        /// Case: the renderer draws a terminal from scratch and has nothing
        /// but this frame to paint the whole window from.
        #[test]
        fn a_snapshot_covers_every_viewport_row_at_full_width() {
            let mut device = device();
            device.active_mut().print('a');
            let snap = FrameSnapshot::new(&device, &PlacementStore::new());
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
            let snap = FrameSnapshot::new(&device, &PlacementStore::new());
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
            let snap = FrameSnapshot::new(&device(), &PlacementStore::new());
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
            let snap = FrameSnapshot::new(&device(), &PlacementStore::new());
            for row in &snap.rows {
                assert_eq!(row.len(), 1);
                assert_eq!(row[0].text, "    ");
                assert_eq!(row[0].fg, Color::DefaultForeground);
                assert_eq!(row[0].bg, Color::DefaultBackground);
            }
        }
    }

    mod delta {
        use crate::damage::DamageRows;
        use crate::device::DeviceState;
        use crate::frame::FrameDelta;
        use crate::placement::PlacementStore;
        use crate::schema::{GridColumn, GridLine, GridSize, ViewportLine};

        fn device() -> DeviceState {
            DeviceState::new(GridSize { cols: 4, rows: 3 }, 10)
        }

        fn delta(device: &DeviceState, lines: &[u16]) -> FrameDelta {
            let rows: DamageRows = lines.iter().copied().map(ViewportLine).collect();
            FrameDelta::new(&rows, device, &PlacementStore::new())
        }

        /// Asserts that a delta repaints the damaged rows and no others.
        ///
        /// Case: a shell echoes a keystroke onto its prompt line, leaving
        /// every other line of the window untouched.
        #[test]
        fn a_delta_repaints_only_the_damaged_rows() {
            let mut device = device();
            device.active_mut().print('a');
            device.active_mut().cr();
            device.active_mut().lf();
            device.active_mut().print('b');
            let delta = delta(&device, &[1]);
            assert_eq!(delta.dirty_rows.len(), 1);
            assert_eq!(delta.dirty_rows[0].line, ViewportLine(1));
            assert_eq!(delta.dirty_rows[0].contents[0].text, "b   ");
        }

        /// Asserts that a damaged row is repainted across its full width.
        ///
        /// The agreed policy repaints whole rows rather than cell spans:
        /// the renderer replaces a row wholesale, so a partial row would
        /// leave the untouched columns showing the previous frame.
        ///
        /// Case: an erase-in-line clears the tail of a row the cursor
        /// sits in the middle of.
        #[test]
        fn a_dirty_row_spans_every_column() {
            let mut device = device();
            device.active_mut().print('a');
            let delta = delta(&device, &[0]);
            let contents = &delta.dirty_rows[0].contents;
            assert_eq!(
                contents.iter().map(|run| u32::from(run.cols)).sum::<u32>(),
                4
            );
        }

        /// Asserts that several damaged rows arrive ascending by line.
        ///
        /// Case: a program redraws a multi-line status area, damaging the
        /// bottom row before the top one.
        #[test]
        fn several_dirty_rows_arrive_ascending() {
            let delta = delta(&device(), &[2, 0, 1]);
            let lines: Vec<_> = delta.dirty_rows.iter().map(|row| row.line).collect();
            assert_eq!(lines, [ViewportLine(0), ViewportLine(1), ViewportLine(2)]);
        }

        /// Asserts that the bottom viewport row is a valid damage target.
        ///
        /// Case: a full-screen application repaints its last line, the
        /// highest line number the viewport addresses.
        #[test]
        fn the_bottom_viewport_row_is_repaintable() {
            let mut device = device();
            device.active_mut().lf();
            device.active_mut().lf();
            device.active_mut().print('z');
            let delta = delta(&device, &[2]);
            assert_eq!(delta.dirty_rows[0].line, ViewportLine(2));
            assert_eq!(delta.dirty_rows[0].contents[0].text, "z   ");
        }

        /// Asserts that a delta carries the write cursor and the offset.
        ///
        /// The agreed policy puts them on every delta, not just
        /// snapshots: cursor-only motion damages no cell content, so a
        /// delta that omitted them would strand the caret until the next
        /// full repaint.
        ///
        /// Case: the user presses an arrow key and the caret has to move
        /// without any row changing.
        #[test]
        fn a_delta_carries_the_cursor_and_the_offset() {
            let mut device = device();
            device.active_mut().print('a');
            let delta = delta(&device, &[0]);
            assert_eq!(delta.cursor.point.line, GridLine(0));
            assert_eq!(delta.cursor.point.column, GridColumn(1));
            assert!(delta.cursor.visible);
            assert_eq!(delta.display_offset, device.display_offset());
        }

        /// Asserts that a delta with no damaged rows still carries
        /// current metadata.
        ///
        /// The agreed policy keeps the empty delta rather than
        /// suppressing the frame: its cursor, offset, and placement list
        /// are what a metadata-only change has to deliver.
        ///
        /// Case: a webview placement moves while no cell content
        /// changes.
        #[test]
        fn an_empty_delta_still_carries_its_metadata() {
            let mut device = device();
            device.active_mut().print('a');
            let delta = delta(&device, &[]);
            assert!(delta.dirty_rows.is_empty());
            assert_eq!(delta.cursor.point.column, GridColumn(1));
            assert_eq!(delta.display_offset, device.display_offset());
        }

        /// Asserts that the fields whose features have not landed are
        /// emitted empty rather than omitted.
        ///
        /// Case: a terminal running before webviews, OSC 8, selection, or
        /// vi mode exist repaints a row.
        #[test]
        fn a_delta_reserves_the_fields_their_features_have_not_reached() {
            let delta = delta(&device(), &[0]);
            assert!(delta.placements.is_empty());
            assert!(delta.hyperlinks.is_empty());
            assert_eq!(delta.vi_cursor, None);
            assert_eq!(delta.selection, None);
        }
    }

    mod emit {
        use crate::damage::{DamageRows, StagedDamage};
        use crate::device::DeviceState;
        use crate::frame::Frame;
        use crate::placement::PlacementStore;
        use crate::schema::{GridSize, ViewportLine};

        fn emit(damage: StagedDamage) -> Frame {
            let device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
            Frame::emit(damage, &device, &PlacementStore::new())
        }

        /// Asserts that full damage becomes a snapshot.
        ///
        /// Case: the window is resized, and the renderer needs the new
        /// dimensions along with every row to redraw against them.
        #[test]
        fn full_damage_emits_a_snapshot() {
            assert!(matches!(emit(StagedDamage::Full), Frame::Snapshot(_)));
        }

        /// Asserts that row damage becomes a delta.
        ///
        /// Case: a shell echoes one character, and repainting the whole
        /// window for it would waste the frame.
        #[test]
        fn row_damage_emits_a_delta() {
            let rows: DamageRows = [ViewportLine(0)].into_iter().collect();
            assert!(matches!(emit(StagedDamage::Delta(rows)), Frame::Delta(_)));
        }
    }
}
