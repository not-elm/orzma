//! The retained last-emitted values `frame()` diffs against.
//!
//! [`EmitState`] is the truth layer of the spec's two-layer change
//! detection: chunk-local liveness decides when to attempt an emit,
//! and this diff decides what the frame carries — `Some` means
//! genuinely different from what the consumer last saw.
#![expect(
    dead_code,
    reason = "Frame::emit reaches this state when the flat frame lands"
)]

use crate::device::ActiveScreen;
use crate::placement::PlacementStore;
use crate::schema::{Cursor, DisplayOffset, Palette, ProjectedPlacement};
use std::mem;

/// The last-emitted cursor, offset, placements, and palette.
///
/// # Invariants
///
/// The retained values update only when a frame is actually emitted:
/// they must mirror what the consumer last saw, so an update on a
/// cancelled attempt would desync every later diff.
pub(crate) struct EmitState {
    cursor: Cursor,
    display_offset: DisplayOffset,
    placements: Vec<ProjectedPlacement>,
    palette: Palette,
    /// Reusable projection buffer, so an unchanged emit attempt
    /// allocates nothing.
    scratch: Vec<ProjectedPlacement>,
}

impl Default for EmitState {
    // NOTE: The cursor is the screen-initial cursor, not
    // `Cursor::default()` — the derived default's `visible` is `false`,
    // which would report a spurious cursor change on the first emit
    // and break the defaults convention the consumer mirrors.
    fn default() -> Self {
        Self {
            cursor: Cursor {
                visible: true,
                ..Cursor::default()
            },
            display_offset: DisplayOffset(0),
            placements: Vec::new(),
            palette: Palette::default(),
            scratch: Vec::new(),
        }
    }
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
        mem::swap(&mut self.placements, &mut self.scratch);
        Some(self.placements.clone())
    }

    /// Reports the palette when it differs from the last-emitted one;
    /// `None` when unchanged.
    pub fn diff_palette(&mut self, palette: &Palette) -> Option<Palette> {
        if *palette == self.palette {
            return None;
        }
        self.palette = palette.clone();
        Some(palette.clone())
    }

    /// Records the small fields an emitted frame just carried.
    pub fn settle_small_fields(&mut self, cursor: Cursor, display_offset: DisplayOffset) {
        self.cursor = cursor;
        self.display_offset = display_offset;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceState;
    use crate::placement::PlacementStore;
    use crate::schema::{CursorShape, GridSize};

    fn device() -> DeviceState {
        DeviceState::new(GridSize { cols: 4, rows: 3 }, 10)
    }

    /// Asserts that the retained defaults match a fresh device, so a
    /// fresh consumer and a fresh VT agree without a completeness flag.
    ///
    /// Case: a terminal spawns and its very first frame omits the
    /// palette and placement sections.
    #[test]
    fn the_default_emit_state_matches_a_fresh_device() {
        let state = EmitState::default();
        let device = device();
        assert_eq!(state.cursor, device.active().cursor());
        assert_eq!(state.display_offset, device.display_offset());
        assert_eq!(state.palette, device.palette());
        assert!(state.placements.is_empty());
        assert_eq!(state.cursor.shape, CursorShape::Block);
        assert!(state.cursor.visible);
    }

    /// Asserts that an unchanged projection diffs to `None` and a
    /// mutated store diffs to the complete new list.
    ///
    /// Case: frames are emitted before and after a program mounts a
    /// webview at the cursor.
    #[test]
    fn diff_placements_reports_only_a_real_change() {
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
        assert_eq!(state.diff_placements(&store, device.active_screen()), None);
    }

    /// Asserts that the palette diff reports a change exactly once.
    ///
    /// Case: an OSC palette override arrives, one frame carries the new
    /// table, and the next frame omits it again.
    #[test]
    fn diff_palette_reports_a_change_exactly_once() {
        let mut state = EmitState::default();
        let mut palette = Palette::default();
        assert_eq!(state.diff_palette(&palette), None);
        palette.foreground = palette.background;
        assert_eq!(state.diff_palette(&palette), Some(palette.clone()));
        assert_eq!(state.diff_palette(&palette), None);
    }
}
