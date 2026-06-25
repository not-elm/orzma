//! Structural rescue for tmux panes whose grid was left unpainted after a
//! layout change: detects the unpainted state and asks `ozmux_tmux` to
//! re-`capture-pane` until the grid paints (spec Component 2).

use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::schema::TerminalGrid;
use ozmux_tmux::{PaneId, RequestPaneReseed, TmuxPane, TmuxProjectionSet};
use std::collections::HashMap;

/// Frames the unpainted state must persist before a reseed is requested. Filters
/// the 1-frame resize transient (dims written before the deferred snapshot
/// flush) while still healing a genuinely lost seed quickly.
const RESEED_DEBOUNCE_FRAMES: u8 = 3;

/// Per-pane consecutive-frames-unpainted counters for the reseed debounce.
#[derive(Resource, Default)]
struct PaneSeedDebounce(HashMap<PaneId, u8>);

/// Wires the structural paint-rescue system after the tmux projection chain.
pub(crate) struct PaintRescuePlugin;

impl Plugin for PaintRescuePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PaneSeedDebounce>().add_systems(
            Update,
            rescue_unpainted_panes
                .after(TmuxProjectionSet)
                .in_set(super::TmuxActiveSet),
        );
    }
}

/// Returns whether a pane's grid is structurally unpainted and needs a full
/// re-seed. The dims-vs-handle clause catches the common `0×0` grid; the
/// `cells_len != rows` clause catches a grid whose dims were written but whose
/// rows were never repopulated (e.g. a lost resize snapshot). A genuinely blank
/// captured pane has `cells_len == rows`, so it does not fire.
fn grid_needs_full_seed(
    grid_cols: u16,
    grid_rows: u16,
    cells_len: usize,
    handle_cols: u16,
    handle_rows: u16,
) -> bool {
    (grid_cols, grid_rows) != (handle_cols, handle_rows) || cells_len != grid_rows as usize
}

/// Advances a per-pane debounce counter and returns whether to emit a reseed
/// request this frame. Emits exactly once when `needs_seed` has held for
/// `threshold` consecutive frames; a `false` resets the counter; once emitted it
/// will not re-emit until the counter resets (the saturated value stays above
/// `threshold`, and only the exact `== threshold` transition emits).
fn should_emit_reseed(counter: &mut u8, needs_seed: bool, threshold: u8) -> bool {
    if !needs_seed {
        *counter = 0;
        return false;
    }
    *counter = counter.saturating_add(1);
    *counter == threshold
}

/// Requests a tmux re-seed for each non-copy-mode pane whose grid is
/// structurally unpainted (see [`grid_needs_full_seed`]) once the state has held
/// for [`RESEED_DEBOUNCE_FRAMES`]. Copy-mode panes are skipped — they paint via
/// the separate `CopyRenderHandle` (Component 3).
fn rescue_unpainted_panes(
    mut debounce: ResMut<PaneSeedDebounce>,
    mut reseed: MessageWriter<RequestPaneReseed>,
    panes: Query<(&TmuxPane, &TerminalHandle, &TerminalGrid), Without<CopyModeState>>,
) {
    for (pane, handle, grid) in panes.iter() {
        let (h_cols, h_rows, _) = handle.read_geometry();
        let needs = grid_needs_full_seed(grid.cols, grid.rows, grid.cells.len(), h_cols, h_rows);
        let counter = debounce.0.entry(pane.id).or_default();
        if should_emit_reseed(counter, needs, RESEED_DEBOUNCE_FRAMES) {
            reseed.write(RequestPaneReseed { pane: pane.id });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_grid_against_sized_handle_needs_seed() {
        assert!(grid_needs_full_seed(0, 0, 0, 80, 24));
    }

    #[test]
    fn dims_written_but_cells_empty_needs_seed() {
        assert!(grid_needs_full_seed(80, 24, 0, 80, 24));
    }

    #[test]
    fn blank_captured_pane_does_not_need_seed() {
        assert!(!grid_needs_full_seed(80, 24, 24, 80, 24));
    }

    #[test]
    fn painted_matching_grid_does_not_need_seed() {
        assert!(!grid_needs_full_seed(80, 24, 24, 80, 24));
    }

    #[test]
    fn debounce_emits_only_after_threshold_consecutive_true() {
        let mut c = 0u8;
        assert!(!should_emit_reseed(&mut c, true, 3));
        assert!(!should_emit_reseed(&mut c, true, 3));
        assert!(should_emit_reseed(&mut c, true, 3));
    }

    #[test]
    fn debounce_resets_on_false() {
        let mut c = 0u8;
        should_emit_reseed(&mut c, true, 3);
        should_emit_reseed(&mut c, true, 3);
        assert!(!should_emit_reseed(&mut c, false, 3));
        assert_eq!(c, 0);
        assert!(!should_emit_reseed(&mut c, true, 3));
    }

    #[test]
    fn debounce_does_not_re_emit_every_frame_while_held() {
        let mut c = 0u8;
        for _ in 0..2 {
            should_emit_reseed(&mut c, true, 3);
        }
        assert!(should_emit_reseed(&mut c, true, 3));
        assert!(!should_emit_reseed(&mut c, true, 3));
    }
}
