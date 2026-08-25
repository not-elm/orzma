//! The retained last-emitted values `frame()` diffs against.
//!
//! [`EmitState`] is the truth layer of the spec's two-layer change
//! detection: chunk-local liveness decides when to attempt an emit,
//! and this diff decides what the frame carries — `Some` means
//! genuinely different from what the consumer last saw.

use crate::device::ActiveScreen;
use crate::placement::PlacementStore;
use crate::schema::{Cursor, DisplayOffset, Palette, ProjectedPlacement};

/// The last-emitted cursor, offset, placements, and palette.
///
/// # Invariants
///
/// A retained value must mirror what the consumer last saw. The diff
/// methods only compare; [`Self::settle`] records what an emitted
/// frame carried, so retention cannot outrun emission.
///
/// The derived default is sound even though its cursor differs from a
/// fresh screen's visible cursor: the ledger's seeded full damage
/// forces the first frame out regardless of any diff, and that emit
/// settles the real cursor before the diffs are ever load-bearing.
#[derive(Default)]
pub(crate) struct EmitState {
    cursor: Cursor,
    display_offset: DisplayOffset,
    placements: Vec<ProjectedPlacement>,
    palette: Palette,
    /// Reusable projection buffer, so an unchanged emit attempt
    /// allocates nothing.
    scratch: Vec<ProjectedPlacement>,
}

impl EmitState {
    /// Returns whether the unconditionally-carried small fields differ
    /// from what the consumer last saw.
    pub fn cursor_or_offset_changed(&self, cursor: &Cursor, display_offset: DisplayOffset) -> bool {
        *cursor != self.cursor || display_offset != self.display_offset
    }

    /// Projects the placements and reports the complete new list when
    /// it differs from the last-emitted one; `None` when unchanged.
    pub fn diff_placements(
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
    pub fn diff_palette(&self, palette: &Palette) -> Option<Palette> {
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
    pub fn settle(
        &mut self,
        cursor: &Cursor,
        display_offset: DisplayOffset,
        placements: Option<&Vec<ProjectedPlacement>>,
        palette: Option<&Palette>,
    ) {
        self.cursor.clone_from(cursor);
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
    use crate::placement::PlacementStore;
    use crate::schema::GridSize;

    fn device() -> DeviceState {
        DeviceState::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    /// Asserts that the retained defaults match a fresh device for the
    /// diffed sections, so a fresh consumer and a fresh VT agree
    /// without a completeness flag.
    ///
    /// Case: a terminal spawns and its very first frame omits the
    /// palette and placement sections.
    #[test]
    fn the_default_emit_state_matches_a_fresh_device() {
        let state = EmitState::default();
        let device = device();
        assert_eq!(state.display_offset, device.display_offset());
        assert_eq!(&state.palette, device.palette());
        assert!(state.placements.is_empty());
    }

    /// Asserts that an unchanged projection diffs to `None`, a mutated
    /// store diffs to the complete new list, and settling that list
    /// makes the next diff report `None` again.
    ///
    /// Case: frames are emitted before and after a program mounts a
    /// webview at the cursor.
    #[test]
    fn diff_placements_reports_the_change_until_settled() {
        let mut state = EmitState::default();
        let device = device();
        let mut store = PlacementStore::new();
        assert_eq!(state.diff_placements(&store, device.active_screen()), None);
        store
            .mount(device.active_screen(), 2, 4, "v".to_string(), None)
            .expect("a mount under the cap is accepted");
        let listed = state
            .diff_placements(&store, device.active_screen())
            .expect("a mount changes the projection");
        assert_eq!(listed.len(), 1);
        let cursor = device.active().cursor();
        state.settle(&cursor, device.display_offset(), Some(&listed), None);
        assert_eq!(state.diff_placements(&store, device.active_screen()), None);
    }

    /// Asserts that the palette diff reports the change until the
    /// emitted table is settled.
    ///
    /// Case: an OSC palette override arrives, one frame carries the new
    /// table, and the next frame omits it again.
    #[test]
    fn diff_palette_reports_the_change_until_settled() {
        let mut state = EmitState::default();
        let mut palette = Palette::default();
        assert_eq!(state.diff_palette(&palette), None);
        palette.foreground = palette.background;
        let changed = state
            .diff_palette(&palette)
            .expect("an override changes the table");
        state.settle(&Cursor::default(), DisplayOffset(0), None, Some(&changed));
        assert_eq!(state.diff_palette(&palette), None);
    }
}
