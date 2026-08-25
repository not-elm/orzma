//! Frame assembly: turning device state into what one emit hands the
//! renderer.
//!
//! A frame is one flat struct: a full repaint simply carries every
//! viewport row, and the changed-only sections (`placements`,
//! `palette`) are `Some` exactly when the emit-time diff against the
//! [`FrameTracker`]'s retained values found them genuinely different.
//! The pieces are read through one shared borrow of the device, so a
//! frame describes a single instant.

pub mod damage;

use self::damage::{Damage, DamageSpan};
use crate::device::color::Palette;
use crate::device::{ActiveScreen, DeviceState};
use crate::hyperlink::Hyperlink;
use crate::placement::{PlacementStore, ProjectedPlacement};
use crate::screen::cursor::Cursor;
use crate::screen::grid::GridSize;
use crate::screen::grid::row::Row;
use crate::screen::grid::run::Run;
use crate::screen::viewport::{DisplayOffset, ViewportLine};
use crate::selection::SelectionRange;
use crate::vi::ViCursor;

/// One emitted frame.
///
/// The `Option` fields split into two kinds: `placements` and
/// `palette` are changed-only (`None` = unchanged since the last
/// emitted frame, and for placements `Some(vec![])` = none visible —
/// a distinct state), while `vi_cursor` and `selection` are absent
/// state (`None` = not in vi mode / no selection, carried on every
/// frame). `rows` and `hyperlinks` need no `Option` — an absent row
/// is unchanged, so a plain `Vec` with empty as its zero is the
/// tighter type.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Grid dimensions; always carried, doubling as the resize signal.
    pub size: GridSize,
    /// The repainted rows, ascending by line. A full repaint carries
    /// every viewport row; a row absent from the list is unchanged.
    pub rows: Vec<DirtyRow>,
    /// Cursor state at emit time; always carried.
    pub cursor: Cursor,
    /// Lines scrolled back from the live tail; always carried.
    pub display_offset: DisplayOffset,
    /// Vi-mode cursor (active only in vi mode). Absent in normal mode.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range. Independent of vi cursor — survives motion.
    pub selection: Option<SelectionRange>,
    /// Viewport-projected webview placements: `None` when unchanged
    /// since the last emitted frame, otherwise the complete list —
    /// `Some(vec![])` means none are visible, which is not an unmount.
    pub placements: Option<Vec<ProjectedPlacement>>,
    /// The live palette symbolic colors resolve against: `None` when
    /// unchanged. A palette override owes a staged full repaint — the
    /// emit-time diff guarantees only that a frame is emitted, not
    /// that it carries rows — an obligation on the future
    /// OSC 4 / 10 / 11 / 12 handler.
    pub palette: Option<Palette>,
    /// Definitions for hyperlink ids referenced by `rows`, merged into
    /// the consumer's retained table. Reserved: empty until the
    /// hyperlink interner is ported, which is safe while
    /// [`crate::prelude::Run::hyperlink_id`] is always `None`.
    pub hyperlinks: Vec<Hyperlink>,
}

/// One repainted viewport row inside a [`Frame`].
#[derive(Debug, Clone, PartialEq)]
pub struct DirtyRow {
    /// The viewport row the contents repaint.
    pub line: ViewportLine,
    /// The row contents.
    pub contents: Row<Run>,
}

/// Tracks what the next frame owes and what the last frame carried.
pub(crate) struct FrameTracker {
    /// Damage staged for the next emit, from every source.
    damage: Damage,
    /// The cursor the last emitted frame carried, compared against to
    /// detect changes.
    cursor: Cursor,
    /// The display offset the last emitted frame carried.
    display_offset: DisplayOffset,
    /// The placement list the last emitted frame carried.
    placements: Vec<ProjectedPlacement>,
    /// The palette the last emitted frame carried.
    palette: Palette,
    /// Reusable projection buffer, so an unchanged emit attempt
    /// allocates nothing.
    scratch: Vec<ProjectedPlacement>,
}

impl FrameTracker {
    /// Builds a tracker whose damage is seeded with the bootstrap full
    /// repaint, so the first emitted frame carries every viewport row.
    ///
    /// The retained defaults are sound even though the default cursor
    /// differs from a fresh screen's visible cursor: the seeded full
    /// damage forces the first frame out regardless of any diff, and
    /// that emit settles the real cursor before the diffs are ever
    /// load-bearing.
    pub fn new() -> Self {
        Self {
            damage: Damage::new(),
            cursor: Cursor::default(),
            display_offset: DisplayOffset::default(),
            placements: Vec::new(),
            palette: Palette::default(),
            scratch: Vec::new(),
        }
    }

    /// Merges `span` into the staged damage.
    pub fn stage(&mut self, span: DamageSpan) {
        self.damage.stage(span);
    }

    /// Stages the reported damage, if any; returns whether there was
    /// any to stage.
    pub fn stage_if_changed(&mut self, span: Option<DamageSpan>) -> bool {
        match span {
            Some(span) => {
                self.stage(span);
                true
            }
            None => false,
        }
    }

    /// Builds the frame for the staged damage and section diffs; `None`
    /// when nothing observable changed.
    ///
    /// # Invariants
    ///
    /// - The rows, the cursor, the offset, and the projection all come
    ///   from one borrow of `device`, so a frame describes one instant.
    /// - The tracker never retains a value the consumer does not see:
    ///   the section diffs only compare, and the emitted frame is
    ///   settled after the gate, so an attempt that returns `None`
    ///   retains nothing.
    pub fn emit(&mut self, device: &DeviceState, placements: &PlacementStore) -> Option<Frame> {
        let screen = device.active();
        let cursor = screen.cursor();
        let display_offset = screen.display_offset();
        let placements = self.diff_placements(placements, device.active_screen());
        let palette = self.diff_palette(device.palette());
        if self.damage.is_clean()
            && placements.is_none()
            && palette.is_none()
            && !self.cursor_or_offset_changed(&cursor, display_offset)
        {
            return None;
        }
        let size = screen.grid_size();
        let dirty_row = |line: ViewportLine| DirtyRow {
            line,
            contents: screen.viewport_row(line).to_runs(),
        };
        let rows = self.damage.dirty_rows(size.rows).map(dirty_row).collect();
        self.damage.clear();
        let frame = Frame {
            size,
            rows,
            cursor,
            display_offset,
            vi_cursor: None,
            selection: None,
            placements,
            palette,
            hyperlinks: Vec::new(),
        };
        self.settle(
            frame.cursor,
            frame.display_offset,
            frame.placements.as_ref(),
            frame.palette.as_ref(),
        );
        Some(frame)
    }

    /// Returns whether the unconditionally-carried small fields differ
    /// from what the consumer last saw.
    fn cursor_or_offset_changed(&self, cursor: &Cursor, display_offset: DisplayOffset) -> bool {
        *cursor != self.cursor || display_offset != self.display_offset
    }

    /// Projects the placements and reports the complete new list when
    /// it differs from the last-emitted one; `None` when unchanged.
    fn diff_placements(
        &mut self,
        placements: &PlacementStore,
        active: ActiveScreen<'_>,
    ) -> Option<Vec<ProjectedPlacement>> {
        self.scratch.clear();
        placements.project_into(&mut self.scratch, active);
        if self.scratch == self.placements {
            return None;
        }
        Some(self.scratch.clone())
    }

    /// Reports the palette when it differs from the last-emitted one;
    /// `None` when unchanged.
    fn diff_palette(&self, palette: &Palette) -> Option<Palette> {
        (*palette != self.palette).then(|| palette.clone())
    }

    /// Records what an emitted frame carried, so later diffs compare
    /// against what the consumer last saw.
    ///
    /// # Invariants
    ///
    /// Every emitted frame settles here exactly once: a diff result
    /// that reaches a frame without being settled would report the
    /// same change again on the next attempt.
    fn settle(
        &mut self,
        cursor: Cursor,
        display_offset: DisplayOffset,
        placements: Option<&Vec<ProjectedPlacement>>,
        palette: Option<&Palette>,
    ) {
        self.cursor = cursor;
        self.display_offset = display_offset;
        if let Some(placements) = placements {
            self.placements.clone_from(placements);
        }
        if let Some(palette) = palette {
            self.palette.clone_from(palette);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceState;
    use crate::device::color::Color;
    use crate::screen::grid::GridSize;
    use crate::screen::grid::coords::{GridColumn, GridLine};

    struct Rig {
        tracker: FrameTracker,
        device: DeviceState,
        placements: PlacementStore,
    }

    /// Builds the emission rig with the seeded bootstrap repaint
    /// already drained, so each test stages exactly what it means to.
    fn drained_rig() -> Rig {
        let mut rig = Rig {
            tracker: FrameTracker::new(),
            device: DeviceState::new(GridSize { cols: 4, rows: 3 }, 10),
            placements: PlacementStore::new(),
        };
        emit(&mut rig).expect("the seeded Full drains as the first frame");
        rig
    }

    fn emit(rig: &mut Rig) -> Option<Frame> {
        rig.tracker.emit(&rig.device, &rig.placements)
    }

    /// Asserts that a new tracker's retained values match a fresh
    /// device for the diffed sections, so a fresh consumer and a fresh
    /// VT agree without a completeness flag.
    ///
    /// Case: a terminal spawns and its very first frame omits the
    /// palette and placement sections.
    #[test]
    fn a_new_tracker_matches_a_fresh_device() {
        let tracker = FrameTracker::new();
        let device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        assert_eq!(tracker.display_offset, device.display_offset());
        assert_eq!(&tracker.palette, device.palette());
        assert!(tracker.placements.is_empty());
    }

    /// Asserts that an unchanged projection diffs to `None`, a mutated
    /// store diffs to the complete new list, and settling that list
    /// makes the next diff report `None` again.
    ///
    /// Case: frames are emitted before and after a program mounts a
    /// webview at the cursor.
    #[test]
    fn diff_placements_reports_the_change_until_settled() {
        let mut tracker = FrameTracker::new();
        let device = DeviceState::new(GridSize { cols: 4, rows: 3 }, 10);
        let mut store = PlacementStore::new();
        assert_eq!(
            tracker.diff_placements(&store, device.active_screen()),
            None
        );
        store
            .mount(device.active_screen(), 2, 4, "v".to_string(), None)
            .expect("a mount under the cap is accepted");
        let listed = tracker
            .diff_placements(&store, device.active_screen())
            .expect("a mount changes the projection");
        assert_eq!(listed.len(), 1);
        let cursor = device.active().cursor();
        tracker.settle(cursor, device.display_offset(), Some(&listed), None);
        assert_eq!(
            tracker.diff_placements(&store, device.active_screen()),
            None
        );
    }

    /// Asserts that the palette diff reports the change until the
    /// emitted table is settled.
    ///
    /// Case: an OSC palette override arrives, one frame carries the new
    /// table, and the next frame omits it again.
    #[test]
    fn diff_palette_reports_the_change_until_settled() {
        let mut tracker = FrameTracker::new();
        let mut palette = Palette::default();
        assert_eq!(tracker.diff_palette(&palette), None);
        palette.foreground = palette.background;
        let changed = tracker
            .diff_palette(&palette)
            .expect("an override changes the table");
        tracker.settle(Cursor::default(), DisplayOffset(0), None, Some(&changed));
        assert_eq!(tracker.diff_palette(&palette), None);
    }

    /// Asserts that `stage_if_changed` reports whether it staged
    /// anything.
    ///
    /// Case: a scroll request is clamped to a no-op and its caller must
    /// learn the viewport did not move.
    #[test]
    fn stage_if_changed_reports_whether_anything_was_staged() {
        let mut rig = drained_rig();
        assert!(!rig.tracker.stage_if_changed(None));
        assert_eq!(emit(&mut rig), None);
        assert!(rig.tracker.stage_if_changed(Some(DamageSpan::Full)));
        assert!(emit(&mut rig).is_some());
    }

    /// Asserts that full damage emits every viewport row at full width,
    /// with the size and always-carried fields beside them.
    ///
    /// Case: the renderer draws a terminal from scratch and has nothing
    /// but this frame to paint the whole window from.
    #[test]
    fn full_damage_emits_every_viewport_row() {
        let mut rig = drained_rig();
        rig.device.active_mut().print('a');
        rig.tracker.stage(DamageSpan::Full);
        let frame = emit(&mut rig).expect("staged damage emits");
        assert_eq!(frame.size, GridSize { cols: 4, rows: 3 });
        assert_eq!(frame.rows.len(), 3);
        for row in &frame.rows {
            assert_eq!(
                row.contents
                    .iter()
                    .map(|run| u32::from(run.cols))
                    .sum::<u32>(),
                4
            );
        }
        assert_eq!(frame.rows[0].contents[0].text, "a   ");
        assert_eq!(frame.cursor.point.column, GridColumn(1));
        assert_eq!(frame.display_offset, rig.device.display_offset());
    }

    /// Asserts that row damage emits exactly the staged rows, ascending.
    ///
    /// Case: a program redraws a multi-line status area, damaging the
    /// bottom row before the top one.
    #[test]
    fn row_damage_emits_exactly_the_staged_rows() {
        let mut rig = drained_rig();
        rig.tracker
            .stage(DamageSpan::rows(ViewportLine(2), ViewportLine(2)));
        rig.tracker
            .stage(DamageSpan::rows(ViewportLine(0), ViewportLine(0)));
        let frame = emit(&mut rig).expect("staged damage emits");
        let lines: Vec<_> = frame.rows.iter().map(|row| row.line).collect();
        assert_eq!(lines, [ViewportLine(0), ViewportLine(2)]);
    }

    /// Asserts that the first frame follows the defaults convention:
    /// full row coverage with the changed-only sections omitted.
    ///
    /// Case: a terminal spawns and the renderer initializes from the
    /// very first frame.
    #[test]
    fn the_first_frame_covers_every_row_and_omits_default_sections() {
        let mut rig = Rig {
            tracker: FrameTracker::new(),
            device: DeviceState::new(GridSize { cols: 4, rows: 3 }, 10),
            placements: PlacementStore::new(),
        };
        let frame = emit(&mut rig).expect("the seeded Full emits");
        assert_eq!(frame.rows.len(), 3);
        assert_eq!(frame.placements, None);
        assert_eq!(frame.palette, None);
        assert_eq!(frame.vi_cursor, None);
        assert_eq!(frame.selection, None);
        assert!(frame.hyperlinks.is_empty());
    }

    /// Asserts that a cursor-only change emits a frame with no rows,
    /// and that the next attempt emits nothing.
    ///
    /// Case: the user presses an arrow key and the caret must move
    /// without any row changing.
    #[test]
    fn a_cursor_only_change_emits_an_empty_rows_frame_once() {
        let mut rig = drained_rig();
        rig.device.active_mut().move_cursor_to(Some(2), Some(3));
        let frame = emit(&mut rig).expect("a moved cursor emits");
        assert!(frame.rows.is_empty());
        assert_eq!(frame.cursor.point.line, GridLine(1));
        assert_eq!(frame.cursor.point.column, GridColumn(2));
        assert_eq!(emit(&mut rig), None);
    }

    /// Asserts that an emit attempt with nothing changed returns
    /// `None` — the lean contract.
    ///
    /// Case: a chunk changes only the pen, which no frame field
    /// carries.
    #[test]
    fn an_unchanged_attempt_emits_nothing() {
        let mut rig = drained_rig();
        rig.device.active_mut().pen_mut().fg = Color::Indexed(1);
        assert_eq!(emit(&mut rig), None);
    }

    /// Asserts that a display-offset change alone emits a frame
    /// carrying the new offset once — the mechanical backstop for the
    /// offset-stages-Full convention.
    ///
    /// Case: the viewport scrolls back while no cell content changes.
    #[test]
    fn an_offset_only_change_emits_once() {
        let mut rig = drained_rig();
        rig.device.active_mut().move_cursor_to(Some(3), None);
        rig.device.active_mut().line_feed();
        emit(&mut rig).expect("the cursor motion and scroll emit");
        rig.device.active_mut().set_display_offset(DisplayOffset(1));
        let frame = emit(&mut rig).expect("a moved viewport emits");
        assert!(frame.rows.is_empty());
        assert_eq!(frame.display_offset, DisplayOffset(1));
        assert_eq!(emit(&mut rig), None);
    }

    /// Asserts that a placement change alone emits a frame carrying
    /// the complete new list, and an unmount later carries the empty
    /// list rather than `None`.
    ///
    /// Case: a program mounts a webview, frames pass, and the program
    /// unmounts it while no cell content changes.
    #[test]
    fn a_placement_change_alone_emits_the_complete_list() {
        let mut rig = drained_rig();
        rig.placements
            .mount(rig.device.active_screen(), 2, 4, "v".to_string(), None)
            .expect("a mount under the cap is accepted");
        let mounted = emit(&mut rig).expect("a placement change emits");
        assert_eq!(mounted.placements.as_ref().map(Vec::len), Some(1));
        assert!(mounted.rows.is_empty());
        assert_eq!(emit(&mut rig), None);
        assert!(rig.placements.unmount(Some("v"), None));
        let unmounted = emit(&mut rig).expect("an unmount emits");
        assert_eq!(unmounted.placements, Some(Vec::new()));
    }
}
