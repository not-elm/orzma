//! Frame assembly: turning device state into what one emit hands the
//! renderer.
//!
//! A frame is one flat struct: a full repaint simply carries every
//! viewport row, and the changed-only sections (`placements`,
//! `palette`) are `Some` exactly when the emit-time diff against
//! [`crate::emit::EmitState`] found them genuinely different. The
//! pieces are read through one shared borrow of the device, so a frame
//! describes a single instant.

use crate::damage::{DamageLedger, StagedDamage};
use crate::device::DeviceState;
use crate::emit::EmitState;
use crate::placement::PlacementStore;
use crate::schema::{
    Cursor, GridSize, Hyperlink, Palette, ProjectedPlacement, Row, Run, SelectionRange, ViCursor,
    ViewportLine,
};
use crate::screen::viewport::DisplayOffset;

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
    /// unchanged. A palette change stages a full repaint, so `Some`
    /// always accompanies full row coverage.
    pub palette: Option<Palette>,
    /// Definitions for hyperlink ids referenced by `rows`, merged into
    /// the consumer's retained table. Reserved: empty until the
    /// hyperlink interner is ported, which is safe while
    /// [`crate::schema::Run::hyperlink_id`] is always `None`.
    pub hyperlinks: Vec<Hyperlink>,
}

impl Frame {
    /// Builds the frame for the staged damage and section diffs; `None`
    /// when nothing observable changed.
    ///
    /// # Invariants
    ///
    /// - The rows, the cursor, the offset, and the projection all come
    ///   from one borrow of `device`, so a frame describes one instant.
    /// - `state` is updated only when a frame is returned, so the
    ///   retained values always mirror what the consumer last saw.
    pub(crate) fn emit(
        state: &mut EmitState,
        damage: &mut DamageLedger,
        device: &DeviceState,
        placements: &PlacementStore,
    ) -> Option<Self> {
        let screen = device.active();
        let cursor = screen.cursor();
        let display_offset = screen.display_offset();
        let staged = damage.take();
        let placements = state.diff_placements(placements, device.active_screen());
        let palette = state.diff_palette(&device.palette());
        if staged.is_none()
            && placements.is_none()
            && palette.is_none()
            && !state.cursor_or_offset_changed(&cursor, display_offset)
        {
            return None;
        }
        state.settle_small_fields(cursor.clone(), display_offset);
        let size = screen.grid_size();
        let rows = match staged {
            Some(StagedDamage::Full) => (0..size.rows)
                .map(|line| DirtyRow {
                    line: ViewportLine(line),
                    contents: screen.viewport_row(ViewportLine(line)).to_runs(),
                })
                .collect(),
            Some(StagedDamage::Delta(dirty)) => dirty
                .iter()
                .map(|&line| DirtyRow {
                    line,
                    contents: screen.viewport_row(line).to_runs(),
                })
                .collect(),
            None => Vec::new(),
        };
        Some(Self {
            size,
            rows,
            cursor,
            display_offset,
            vi_cursor: None,
            selection: None,
            placements,
            palette,
            hyperlinks: Vec::new(),
        })
    }
}

/// One repainted viewport row inside a [`Frame`].
#[derive(Debug, Clone, PartialEq)]
pub struct DirtyRow {
    /// The viewport row the contents repaint.
    pub line: ViewportLine,
    /// The row contents.
    pub contents: Row<Run>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::Damage;
    use crate::schema::{Color, GridColumn, GridLine, GridSize};

    struct Rig {
        state: EmitState,
        damage: DamageLedger,
        device: DeviceState,
        placements: PlacementStore,
    }

    /// Builds the emission rig with the seeded bootstrap repaint
    /// already drained, so each test stages exactly what it means to.
    fn drained_rig() -> Rig {
        let mut rig = Rig {
            state: EmitState::default(),
            damage: DamageLedger::new(),
            device: DeviceState::new(GridSize { cols: 4, rows: 3 }, 10),
            placements: PlacementStore::new(),
        };
        emit(&mut rig).expect("the seeded Full drains as the first frame");
        rig
    }

    fn emit(rig: &mut Rig) -> Option<Frame> {
        Frame::emit(
            &mut rig.state,
            &mut rig.damage,
            &rig.device,
            &rig.placements,
        )
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
        rig.damage.stage(Damage::Full);
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
        rig.damage
            .stage(Damage::rows(ViewportLine(2), ViewportLine(2)));
        rig.damage
            .stage(Damage::rows(ViewportLine(0), ViewportLine(0)));
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
            state: EmitState::default(),
            damage: DamageLedger::new(),
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
