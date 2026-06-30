//! Structural rescue for tmux panes whose grid was left unpainted after a
//! layout change: detects the unpainted state and asks `ozmux_tmux` to
//! re-`capture-pane` until the grid paints (spec Component 2).
//!
//! It also recovers a pane whose grid went *blank* (structurally fine, so the
//! reseed path ignores it) while its live mirror still holds content, by
//! repainting from the mirror — automating the manual-scroll workaround for the
//! persistent-blank-pane bug.

use super::render::TmuxLayoutSet;
use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::schema::{Cell, TerminalGrid};
use ozmux_tmux::{PaneId, RequestPaneReseed, TmuxPane, TmuxProjectionSet};
use std::collections::HashMap;

/// Frames the unpainted state must persist before the FIRST reseed request
/// (filters the ≤1-frame resize transient).
const RESEED_DEBOUNCE_FRAMES: u8 = 3;
/// Frames to wait for a reseed's capture to land before re-requesting. This is
/// the dedicated in-flight age (spec §3.2) so a lost reply does not wedge a pane.
const RESEED_INFLIGHT_TIMEOUT: u16 = 30;

/// Per-pane reseed state: a debounce streak before the first request, then an
/// in-flight age that re-requests on timeout until the grid paints. The
/// `blank_*` fields drive the independent blank-grid-vs-live-mirror recovery
/// (see [`evaluate_blank_recovery`]).
#[derive(Default)]
struct ReseedTracker {
    unpainted_streak: u8,
    inflight_age: Option<u16>,
    /// Consecutive frames the grid has been blank while the mirror holds
    /// content, within the current `blank_recovery_seq` episode.
    blank_streak: u8,
    /// The grid `last_seq` the current blank-recovery episode is evaluating. A
    /// new seq reopens evaluation; `None` forces a fresh evaluation.
    blank_recovery_seq: Option<u32>,
    /// Set once a blank episode is resolved (repainted, or the mirror is also
    /// blank). Stops the per-frame mirror scan until the grid's seq changes.
    blank_recovery_settled: bool,
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
            .add_observer(repaint_pane_from_mirror)
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
        // NOTE: reset only the structural-reseed fields, NOT the whole tracker —
        // the `blank_*` fields belong to the independent blank-recovery state
        // machine (`evaluate_blank_recovery`), which runs on the common `!needs`
        // path; clobbering them here would wipe its debounce every frame.
        tracker.unpainted_streak = 0;
        tracker.inflight_age = None;
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

/// Whether the rendered grid paints no glyph in any cell (via [`Cell::is_blank`],
/// the same predicate the renderer's glyph resolution uses). A pane that lost its
/// painted content to a transient blank frame reads as blank here while its live
/// mirror still reports `has_visible_content`. An empty `cells` vec also reads as
/// blank, but that is the structural case [`grid_needs_full_seed`] already owns
/// (guarded by `!needs` at the call site).
///
/// NOTE: glyph-only — a cell visible solely through a non-default background or
/// reverse video (a colored status bar with no text) reads as blank. This pairs
/// with the equally glyph-only `TerminalHandle::has_visible_content`, so the two
/// agree and the recovery never loops; the cost is that a purely color-block
/// pane is not auto-recovered (it still has the manual-scroll fallback).
fn grid_visibly_blank(grid: &TerminalGrid) -> bool {
    grid.cells.iter().flatten().all(Cell::is_blank)
}

/// Advances a pane's blank-recovery state machine one frame and returns whether
/// to repaint it from the live mirror now.
///
/// Fires once the grid has been blank while the mirror still holds content for
/// [`RESEED_DEBOUNCE_FRAMES`] consecutive frames (filtering the resize transient
/// where the grid is briefly blank before the resize snapshot lands). The
/// episode is keyed on the grid `last_seq`: a seq change reopens evaluation, and
/// once an episode is `settled` (repainted, mirror also blank, or grid painted)
/// the per-frame mirror scan is skipped until the grid changes again — a
/// content-gaining mirror always bumps the seq, so this cannot wedge.
fn evaluate_blank_recovery(
    tracker: &mut ReseedTracker,
    grid: &TerminalGrid,
    handle: &TerminalHandle,
) -> bool {
    if tracker.blank_recovery_seq != Some(grid.last_seq) {
        tracker.blank_recovery_seq = Some(grid.last_seq);
        tracker.blank_streak = 0;
        tracker.blank_recovery_settled = false;
    }
    if tracker.blank_recovery_settled {
        return false;
    }
    if !grid_visibly_blank(grid) {
        tracker.blank_streak = 0;
        tracker.blank_recovery_settled = true;
        return false;
    }
    if !handle.has_visible_content() {
        // NOTE: grid and mirror both blank — a genuinely empty pane with nothing
        // to restore. Settling here (not just returning) is load-bearing: it
        // stops the per-frame mirror scan until the grid's seq changes.
        tracker.blank_recovery_settled = true;
        return false;
    }
    tracker.blank_streak = tracker.blank_streak.saturating_add(1);
    if tracker.blank_streak >= RESEED_DEBOUNCE_FRAMES {
        tracker.blank_recovery_settled = true;
        true
    } else {
        false
    }
}

/// Requests a tmux re-seed for each non-copy-mode pane whose grid is
/// structurally unpainted (see [`grid_needs_full_seed`]) once the state has
/// held for [`RESEED_DEBOUNCE_FRAMES`], then re-requests every
/// [`RESEED_INFLIGHT_TIMEOUT`] frames until the grid paints. Copy-mode panes
/// are skipped — they paint via the separate `CopyRenderHandle` (Component 3).
///
/// Separately, recovers a grid that went *blank* (structurally fine, so the
/// reseed path above ignores it) while its live mirror still holds content: it
/// triggers [`RepaintLiveMirror`], whose observer repaints from the
/// authoritative mirror. This automates the manual-scroll workaround for the
/// persistent-blank-pane bug — the only path that previously re-synced an idle
/// pane out of a blank-but-sized grid. The gather query stays read-only; the
/// `&mut TerminalHandle` write lives in the observer (apply-via-observer idiom).
fn rescue_unpainted_panes(
    mut commands: Commands,
    mut trackers: ResMut<PaneSeedTrackers>,
    mut reseed: MessageWriter<RequestPaneReseed>,
    panes: Query<(Entity, &TmuxPane, &TerminalHandle, &TerminalGrid), Without<CopyModeState>>,
) {
    for (entity, pane, handle, grid) in panes.iter() {
        let (h_cols, h_rows, _) = handle.read_geometry();
        let needs = grid_needs_full_seed(grid.cols, grid.rows, grid.cells.len(), h_cols, h_rows);
        let tracker = trackers.0.entry(pane.id).or_default();
        if reseed_decision(tracker, needs) {
            reseed.write(RequestPaneReseed { pane: pane.id });
        }
        if needs {
            // NOTE: structural reseed owns this pane; reopening blank-recovery
            // (clearing `blank_recovery_seq`) is required so it re-evaluates once
            // the grid is structurally repainted — otherwise a settled episode
            // would suppress recovery after the reseed lands.
            tracker.blank_streak = 0;
            tracker.blank_recovery_seq = None;
            tracker.blank_recovery_settled = false;
            continue;
        }
        if evaluate_blank_recovery(tracker, grid, handle) {
            commands.trigger(RepaintLiveMirror { entity });
        }
    }
}

/// Asks the blank-recovery observer to repaint a pane's grid from its live
/// `TerminalHandle` mirror. Triggered by [`rescue_unpainted_panes`].
#[derive(EntityEvent)]
struct RepaintLiveMirror {
    #[event_target]
    entity: Entity,
}

/// Repaints the target pane's grid from its live mirror. Holds the `&mut
/// TerminalHandle` here so [`rescue_unpainted_panes`] can keep a read-only,
/// parallelizable gather query.
fn repaint_pane_from_mirror(
    repaint: On<RepaintLiveMirror>,
    mut commands: Commands,
    mut handles: Query<&mut TerminalHandle>,
) {
    if let Ok(mut handle) = handles.get_mut(repaint.entity) {
        handle.repaint_full(&mut commands, repaint.entity);
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

    fn cell(text: &str) -> ozma_tty_renderer::schema::Cell {
        ozma_tty_renderer::schema::Cell {
            text: text.to_string(),
            width: 1,
            fg: Default::default(),
            bg: Default::default(),
            style: 0,
            hyperlink_id: None,
        }
    }

    #[test]
    fn grid_with_only_whitespace_is_blank() {
        let grid = TerminalGrid {
            cols: 3,
            rows: 2,
            cells: vec![vec![cell(" "), cell(" ")], vec![cell(""), cell(" ")]],
            ..Default::default()
        };
        assert!(grid_visibly_blank(&grid));
    }

    #[test]
    fn grid_with_any_glyph_is_not_blank() {
        let grid = TerminalGrid {
            cols: 3,
            rows: 2,
            cells: vec![vec![cell(" "), cell("x")], vec![cell(" "), cell(" ")]],
            ..Default::default()
        };
        assert!(!grid_visibly_blank(&grid));
    }

    #[test]
    fn empty_cells_reads_as_blank() {
        let grid = TerminalGrid {
            cols: 0,
            rows: 0,
            cells: vec![],
            ..Default::default()
        };
        assert!(grid_visibly_blank(&grid));
    }

    #[test]
    fn width_zero_cells_read_as_blank() {
        // A width-0 cell (combining mark / wide-char spacer) paints no glyph, so
        // it must read blank here too — matching the renderer's `Cell::is_blank`.
        let zero_width = Cell {
            text: "x".to_string(),
            width: 0,
            ..cell("x")
        };
        let grid = TerminalGrid {
            cols: 1,
            rows: 1,
            cells: vec![vec![zero_width]],
            ..Default::default()
        };
        assert!(grid_visibly_blank(&grid));
    }

    fn blank_grid(seq: u32) -> TerminalGrid {
        TerminalGrid {
            cols: 4,
            rows: 2,
            cells: vec![vec![cell(" ")], vec![cell(" ")]],
            last_seq: seq,
            ..Default::default()
        }
    }

    #[test]
    fn blank_recovery_fires_after_debounce_then_settles() {
        let mut t = ReseedTracker::default();
        let grid = blank_grid(5);
        let mut painted = TerminalHandle::detached(4, 2);
        painted.advance(b"hi");
        // Same seq across frames: the streak accumulates to the debounce.
        assert!(!evaluate_blank_recovery(&mut t, &grid, &painted));
        assert!(!evaluate_blank_recovery(&mut t, &grid, &painted));
        assert!(evaluate_blank_recovery(&mut t, &grid, &painted));
        // Settled: the same seq does not re-fire (the repaint bumps the seq).
        assert!(!evaluate_blank_recovery(&mut t, &grid, &painted));
    }

    #[test]
    fn blank_recovery_skips_when_mirror_is_also_blank() {
        let mut t = ReseedTracker::default();
        let grid = blank_grid(5);
        let blank = TerminalHandle::detached(4, 2);
        for _ in 0..(RESEED_DEBOUNCE_FRAMES + 2) {
            assert!(!evaluate_blank_recovery(&mut t, &grid, &blank));
        }
        // Settled on the first blank-mirror frame: no streak accumulates.
        assert!(t.blank_recovery_settled);
        assert_eq!(t.blank_streak, 0);
    }

    #[test]
    fn blank_recovery_resets_on_seq_change() {
        let mut t = ReseedTracker::default();
        let mut painted = TerminalHandle::detached(4, 2);
        painted.advance(b"hi");
        // Two blank frames at seq 5, then a new seq reopens the episode, so the
        // streak restarts and a single later frame does not fire.
        evaluate_blank_recovery(&mut t, &blank_grid(5), &painted);
        evaluate_blank_recovery(&mut t, &blank_grid(5), &painted);
        assert!(!evaluate_blank_recovery(&mut t, &blank_grid(6), &painted));
        assert_eq!(t.blank_streak, 1);
    }

    #[test]
    fn blank_recovery_ignores_painted_grid() {
        let mut t = ReseedTracker::default();
        let grid = TerminalGrid {
            cols: 4,
            rows: 2,
            cells: vec![vec![cell("x")], vec![cell(" ")]],
            last_seq: 5,
            ..Default::default()
        };
        let mut painted = TerminalHandle::detached(4, 2);
        painted.advance(b"hi");
        assert!(!evaluate_blank_recovery(&mut t, &grid, &painted));
        assert_eq!(t.blank_streak, 0);
    }

    #[test]
    fn blank_grid_with_live_content_repaints_from_mirror() {
        use bevy::ecs::message::Messages;
        use ozma_tty_renderer::prelude::TerminalGridPlugin;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<PaneSeedTrackers>();
        app.init_resource::<Messages<RequestPaneReseed>>();
        app.add_observer(repaint_pane_from_mirror);
        app.add_systems(Update, rescue_unpainted_panes);

        let dims = CellDims {
            width: 4,
            height: 2,
            xoff: 0,
            yoff: 0,
        };
        let mut handle = TerminalHandle::detached(4, 2);
        handle.advance(b"hi");
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims,
                },
                handle,
                TerminalGrid {
                    cols: 4,
                    rows: 2,
                    cells: vec![vec![cell(" ")], vec![cell(" ")]],
                    ..Default::default()
                },
            ))
            .id();

        // The mirror holds "hi" but the rendered grid is blank: after the
        // debounce the rescue must repaint the grid from the live mirror.
        for _ in 0..(RESEED_DEBOUNCE_FRAMES as usize + 1) {
            app.update();
        }

        let grid = app.world().get::<TerminalGrid>(pane).unwrap();
        let row0: String = grid.cells[0].iter().map(|c| c.text.as_str()).collect();
        assert!(
            row0.starts_with("hi"),
            "blank grid with a content-bearing mirror repaints to live content, got {row0:?}",
        );
    }

    #[test]
    fn blank_grid_with_blank_mirror_is_not_repainted() {
        use bevy::ecs::message::Messages;
        use ozma_tty_renderer::prelude::TerminalGridPlugin;
        use ozma_tty_renderer::schema::FrameSnapshot;
        use tmux_control_parser::CellDims;

        #[derive(Resource, Default)]
        struct SnapHits(u32);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<PaneSeedTrackers>();
        app.init_resource::<Messages<RequestPaneReseed>>();
        app.init_resource::<SnapHits>();
        app.add_observer(|_snap: On<FrameSnapshot>, mut hits: ResMut<SnapHits>| {
            hits.0 += 1;
        });
        app.add_observer(repaint_pane_from_mirror);
        app.add_systems(Update, rescue_unpainted_panes);

        let dims = CellDims {
            width: 4,
            height: 2,
            xoff: 0,
            yoff: 0,
        };
        app.world_mut().spawn((
            TmuxPane {
                id: PaneId(1),
                dims,
            },
            TerminalHandle::detached(4, 2),
            TerminalGrid {
                cols: 4,
                rows: 2,
                cells: vec![vec![cell(" ")], vec![cell(" ")]],
                ..Default::default()
            },
        ));

        for _ in 0..(RESEED_DEBOUNCE_FRAMES as usize + 2) {
            app.update();
        }

        assert_eq!(
            app.world().resource::<SnapHits>().0,
            0,
            "a genuinely blank pane (blank mirror) must not be repainted — no repaint loop",
        );
    }
}
