//! Structural rescue for tmux panes whose grid was left unpainted after a
//! layout change: detects the unpainted state and asks `ozmux_tmux` to
//! re-`capture-pane` until the grid paints (spec Component 2).

use super::render::TmuxLayoutSet;
use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::schema::TerminalGrid;
use ozmux_tmux::{PaneId, RequestPaneReseed, TmuxPane, TmuxProjectionSet};
use std::collections::HashMap;

/// Frames the unpainted state must persist before the FIRST reseed request
/// (filters the ≤1-frame resize transient).
const RESEED_DEBOUNCE_FRAMES: u8 = 3;
/// Frames to wait for a reseed's capture to land before re-requesting. This is
/// the dedicated in-flight age (spec §3.2) so a lost reply does not wedge a pane.
const RESEED_INFLIGHT_TIMEOUT: u16 = 30;

/// Per-pane reseed state: a debounce streak before the first request, then an
/// in-flight age that re-requests on timeout until the grid paints.
#[derive(Default)]
struct ReseedTracker {
    unpainted_streak: u8,
    inflight_age: Option<u16>,
}

/// Per-pane reseed trackers, keyed by `PaneId`.
#[derive(Resource, Default)]
struct PaneSeedTrackers(HashMap<PaneId, ReseedTracker>);

/// Wires the structural paint-rescue system after the tmux projection chain.
pub(crate) struct PaintRescuePlugin;

impl Plugin for PaintRescuePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PaneSeedTrackers>()
            .add_observer(prune_tracker_on_pane_removed)
            .add_systems(
                Update,
                rescue_unpainted_panes
                    .after(TmuxProjectionSet)
                    .before(TmuxLayoutSet)
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

/// Advances a pane's reseed tracker one frame and returns whether to emit a
/// reseed request now. A painted grid (`!needs_seed`) resets the tracker.
/// Otherwise it debounces `RESEED_DEBOUNCE_FRAMES` consecutive unpainted frames
/// before the first request, then suppresses while a request is in flight,
/// re-requesting every `RESEED_INFLIGHT_TIMEOUT` frames until the grid paints.
fn reseed_decision(tracker: &mut ReseedTracker, needs_seed: bool) -> bool {
    if !needs_seed {
        *tracker = ReseedTracker::default();
        return false;
    }
    match &mut tracker.inflight_age {
        Some(age) => {
            *age = age.saturating_add(1);
            if *age >= RESEED_INFLIGHT_TIMEOUT {
                *age = 0;
                true
            } else {
                false
            }
        }
        None => {
            tracker.unpainted_streak = tracker.unpainted_streak.saturating_add(1);
            if tracker.unpainted_streak >= RESEED_DEBOUNCE_FRAMES {
                tracker.inflight_age = Some(0);
                true
            } else {
                false
            }
        }
    }
}

/// Requests a tmux re-seed for each non-copy-mode pane whose grid is
/// structurally unpainted (see [`grid_needs_full_seed`]) once the state has
/// held for [`RESEED_DEBOUNCE_FRAMES`], then re-requests every
/// [`RESEED_INFLIGHT_TIMEOUT`] frames until the grid paints. Copy-mode panes
/// are skipped — they paint via the separate `CopyRenderHandle` (Component 3).
fn rescue_unpainted_panes(
    mut trackers: ResMut<PaneSeedTrackers>,
    mut reseed: MessageWriter<RequestPaneReseed>,
    panes: Query<(&TmuxPane, &TerminalHandle, &TerminalGrid), Without<CopyModeState>>,
) {
    for (pane, handle, grid) in panes.iter() {
        let (h_cols, h_rows, _) = handle.read_geometry();
        let needs = grid_needs_full_seed(grid.cols, grid.rows, grid.cells.len(), h_cols, h_rows);
        let tracker = trackers.0.entry(pane.id).or_default();
        if reseed_decision(tracker, needs) {
            reseed.write(RequestPaneReseed { pane: pane.id });
        }
    }
}

fn prune_tracker_on_pane_removed(
    ev: On<Remove, TmuxPane>,
    mut trackers: ResMut<PaneSeedTrackers>,
    panes: Query<&TmuxPane>,
) {
    if let Ok(pane) = panes.get(ev.entity) {
        trackers.0.remove(&pane.id);
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
    fn reseed_emits_after_debounce_frames() {
        let mut t = ReseedTracker::default();
        assert!(!reseed_decision(&mut t, true));
        assert!(!reseed_decision(&mut t, true));
        assert!(reseed_decision(&mut t, true));
    }

    #[test]
    fn reseed_suppresses_while_in_flight_then_retries_on_timeout() {
        let mut t = ReseedTracker::default();
        for _ in 0..RESEED_DEBOUNCE_FRAMES {
            reseed_decision(&mut t, true);
        }
        for _ in 0..(RESEED_INFLIGHT_TIMEOUT - 1) {
            assert!(!reseed_decision(&mut t, true));
        }
        assert!(reseed_decision(&mut t, true));
    }

    #[test]
    fn reseed_resets_when_painted() {
        let mut t = ReseedTracker::default();
        for _ in 0..RESEED_DEBOUNCE_FRAMES {
            reseed_decision(&mut t, true);
        }
        assert!(!reseed_decision(&mut t, false));
        assert!(t.inflight_age.is_none());
        assert_eq!(t.unpainted_streak, 0);
    }

    #[test]
    fn reseed_ignores_one_frame_transient() {
        let mut t = ReseedTracker::default();
        assert!(!reseed_decision(&mut t, true));
        assert!(!reseed_decision(&mut t, false));
        assert!(!reseed_decision(&mut t, true));
    }
}
