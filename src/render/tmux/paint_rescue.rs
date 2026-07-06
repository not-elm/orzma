//! Structural rescue for tmux panes whose grid was left unpainted after a
//! layout change: detects the unpainted state and asks `orzma_tmux` to
//! re-`capture-pane` until the grid paints (spec Component 2).
//!
//! It also recovers a pane whose grid went *blank* (structurally fine, so the
//! reseed path ignores it) while its live mirror still holds content, by
//! repainting from the mirror — automating the manual-scroll workaround for the
//! persistent-blank-pane bug.

use super::TmuxLayoutSet;
use crate::app_mode::TmuxActiveSet;
use crate::ui::vi_mode::ViModeState;
use bevy::prelude::*;
use orzma_tmux::{RequestPaneReseed, TmuxPane, TmuxProjectionSet};
use orzma_tty_engine::TerminalHandle;
use orzma_tty_renderer::schema::{Cell, TerminalGrid};

/// Frames the unpainted state must persist before the FIRST reseed request
/// (filters the ≤1-frame resize transient).
const RESEED_DEBOUNCE_FRAMES: u8 = 3;
/// Frames to wait for a reseed's capture to land before re-requesting. This is
/// the dedicated in-flight age (spec §3.2) so a lost reply does not wedge a pane.
const RESEED_INFLIGHT_TIMEOUT: u16 = 30;

/// Per-pane structural-reseed debounce: a streak before the first capture
/// request, then an in-flight age that re-requests on timeout until painted.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq)]
struct StructuralReseedState {
    /// Consecutive unpainted frames counted before the first reseed request.
    /// Once it reaches [`RESEED_DEBOUNCE_FRAMES`] the first `capture-pane`
    /// request fires and `inflight_age` takes over; this debounce filters the
    /// ≤1-frame resize transient so a momentary unpaint is not re-seeded.
    unpainted_streak: u8,
    /// Whether a reseed request is in flight. `None` while still debouncing (no
    /// request sent yet); `Some(age)` after a request, where `age` counts frames
    /// since the last request and re-requests every [`RESEED_INFLIGHT_TIMEOUT`]
    /// frames until the grid paints — so a lost capture reply cannot wedge a pane.
    inflight_age: Option<u16>,
}

/// Per-pane blank-recovery debounce: a blank-grid-vs-live-mirror episode keyed
/// on the grid seq, repainting from the mirror once the divergence persists.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq)]
struct BlankRecoveryState {
    /// Consecutive frames the grid has been blank while the live mirror still
    /// holds content, within the current `recovery_seq` episode. The repaint
    /// fires once it reaches [`RESEED_DEBOUNCE_FRAMES`], filtering the resize
    /// transient where the grid is briefly blank before the resize snapshot lands.
    streak: u8,
    /// The grid `last_seq` the current episode is evaluating. A different seq
    /// (the grid changed) reopens evaluation — resetting `streak` and `settled`;
    /// `None` forces a fresh evaluation. Keying on the seq is what lets a settled
    /// episode stop re-scanning until the grid actually changes again.
    recovery_seq: Option<u32>,
    /// Whether the current episode is resolved: the repaint fired, the mirror was
    /// also blank (genuinely empty pane, nothing to restore), or the grid is no
    /// longer blank. Suppresses the per-frame grid/mirror scan until
    /// `recovery_seq` changes.
    settled: bool,
}

/// Wires the structural paint-rescue system after the tmux projection chain.
pub(super) struct PaintRescuePlugin;

impl Plugin for PaintRescuePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(repaint_pane_from_mirror).add_systems(
            Update,
            (attach_rescue_state, rescue_unpainted_panes)
                .chain()
                .after(TmuxProjectionSet)
                .before(TmuxLayoutSet)
                .in_set(TmuxActiveSet),
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

/// Advances a pane's structural-reseed debounce one frame and returns whether to
/// emit a reseed request now. A painted grid (`!needs_seed`) resets the state.
/// Otherwise it debounces `RESEED_DEBOUNCE_FRAMES` consecutive unpainted frames
/// before the first request, then suppresses while a request is in flight,
/// re-requesting every `RESEED_INFLIGHT_TIMEOUT` frames until the grid paints.
fn reseed_decision(state: &mut StructuralReseedState, needs_seed: bool) -> bool {
    if !needs_seed {
        *state = StructuralReseedState::default();
        return false;
    }
    match &mut state.inflight_age {
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
            state.unpainted_streak = state.unpainted_streak.saturating_add(1);
            if state.unpainted_streak >= RESEED_DEBOUNCE_FRAMES {
                state.inflight_age = Some(0);
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
/// [`RESEED_DEBOUNCE_FRAMES`] consecutive frames. The episode is keyed on the
/// grid `last_seq`: a seq change reopens evaluation, and once an episode is
/// `settled` (repainted, mirror also blank, or grid painted) the per-frame
/// mirror scan is skipped until the grid changes again.
fn evaluate_blank_recovery(
    state: &mut BlankRecoveryState,
    grid: &TerminalGrid,
    handle: &TerminalHandle,
) -> bool {
    if state.recovery_seq != Some(grid.last_seq) {
        state.recovery_seq = Some(grid.last_seq);
        state.streak = 0;
        state.settled = false;
    }
    if state.settled {
        return false;
    }
    if !grid_visibly_blank(grid) {
        state.streak = 0;
        state.settled = true;
        return false;
    }
    if !handle.has_visible_content() {
        // NOTE: grid and mirror both blank — a genuinely empty pane with nothing
        // to restore. Settling here (not just returning) is load-bearing: it
        // stops the per-frame mirror scan until the grid's seq changes.
        state.settled = true;
        return false;
    }
    state.streak = state.streak.saturating_add(1);
    if state.streak >= RESEED_DEBOUNCE_FRAMES {
        state.settled = true;
        true
    } else {
        false
    }
}

/// Requests a tmux re-seed for each non-vi-mode pane whose grid is
/// structurally unpainted (see [`grid_needs_full_seed`]) once the state has
/// held for [`RESEED_DEBOUNCE_FRAMES`], then re-requests every
/// [`RESEED_INFLIGHT_TIMEOUT`] frames until the grid paints. Vi-mode panes
/// are skipped — the local vi applier (`crate::action::vi::applier`) is
/// already scrolling this same `TerminalHandle`/`TerminalGrid`, and a
/// structural reseed's `capture-pane` would recapture the live tail and
/// clobber that scrolled view.
///
/// Separately, recovers a grid that went *blank* (structurally fine) while its
/// live mirror still holds content: it triggers [`RepaintLiveMirror`], whose
/// observer repaints from the authoritative mirror. The gather query stays
/// read-only on the handle; the `&mut TerminalHandle` write lives in the observer.
fn rescue_unpainted_panes(
    mut commands: Commands,
    mut reseed: MessageWriter<RequestPaneReseed>,
    mut panes: Query<
        (
            Entity,
            &TmuxPane,
            &TerminalHandle,
            &TerminalGrid,
            &mut StructuralReseedState,
            &mut BlankRecoveryState,
        ),
        Without<ViModeState>,
    >,
) {
    for (entity, pane, handle, grid, mut reseed_state, mut blank_state) in panes.iter_mut() {
        let (h_cols, h_rows, _) = handle.read_geometry();
        let needs = grid_needs_full_seed(grid.cols, grid.rows, grid.cells.len(), h_cols, h_rows);
        // NOTE: run the deciders on a local copy and write back through the
        // `Mut` only on a real change. A bare `&mut`/`*state = ...` every frame
        // would mark these components `Changed` for every pane every frame
        // (steady state included), defeating any future `Changed`/`run_if`
        // consumer — the repo change-detection rule. `*state` reads via `Deref`
        // (no change tick); the guarded assignment is the only `DerefMut`.
        let mut next_reseed = *reseed_state;
        if reseed_decision(&mut next_reseed, needs) {
            reseed.write(RequestPaneReseed { pane: pane.id });
        }
        if *reseed_state != next_reseed {
            *reseed_state = next_reseed;
        }
        if needs {
            // Structural reseed owns this pane; reopen blank-recovery so it
            // re-evaluates once the grid is structurally repainted.
            if *blank_state != BlankRecoveryState::default() {
                *blank_state = BlankRecoveryState::default();
            }
            continue;
        }
        let mut next_blank = *blank_state;
        if evaluate_blank_recovery(&mut next_blank, grid, handle) {
            commands.trigger(RepaintLiveMirror { entity });
        }
        if *blank_state != next_blank {
            *blank_state = next_blank;
        }
    }
}

/// Attaches the per-pane rescue state components once per pane. `TmuxPane` is
/// defined in `orzma_tmux`, so the binary cannot `#[require]` these onto it; the
/// `Without<StructuralReseedState>` filter makes this run exactly once per pane
/// (both components are always inserted together).
fn attach_rescue_state(
    mut commands: Commands,
    panes: Query<Entity, (With<TmuxPane>, Without<StructuralReseedState>)>,
) {
    for entity in panes.iter() {
        commands.entity(entity).insert((
            StructuralReseedState::default(),
            BlankRecoveryState::default(),
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_tmux::PaneId;

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
        let mut t = StructuralReseedState::default();
        assert!(!reseed_decision(&mut t, true));
        assert!(!reseed_decision(&mut t, true));
        assert!(reseed_decision(&mut t, true));
    }

    #[test]
    fn reseed_suppresses_while_in_flight_then_retries_on_timeout() {
        let mut t = StructuralReseedState::default();
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
        let mut t = StructuralReseedState::default();
        for _ in 0..RESEED_DEBOUNCE_FRAMES {
            reseed_decision(&mut t, true);
        }
        assert!(!reseed_decision(&mut t, false));
        assert!(t.inflight_age.is_none());
        assert_eq!(t.unpainted_streak, 0);
    }

    #[test]
    fn reseed_ignores_one_frame_transient() {
        let mut t = StructuralReseedState::default();
        assert!(!reseed_decision(&mut t, true));
        assert!(!reseed_decision(&mut t, false));
        assert!(!reseed_decision(&mut t, true));
    }

    fn cell(text: &str) -> orzma_tty_renderer::schema::Cell {
        orzma_tty_renderer::schema::Cell {
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
        let mut t = BlankRecoveryState::default();
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
        let mut t = BlankRecoveryState::default();
        let grid = blank_grid(5);
        let blank = TerminalHandle::detached(4, 2);
        for _ in 0..(RESEED_DEBOUNCE_FRAMES + 2) {
            assert!(!evaluate_blank_recovery(&mut t, &grid, &blank));
        }
        // Settled on the first blank-mirror frame: no streak accumulates.
        assert!(t.settled);
        assert_eq!(t.streak, 0);
    }

    #[test]
    fn blank_recovery_resets_on_seq_change() {
        let mut t = BlankRecoveryState::default();
        let mut painted = TerminalHandle::detached(4, 2);
        painted.advance(b"hi");
        // Two blank frames at seq 5, then a new seq reopens the episode, so the
        // streak restarts and a single later frame does not fire.
        evaluate_blank_recovery(&mut t, &blank_grid(5), &painted);
        evaluate_blank_recovery(&mut t, &blank_grid(5), &painted);
        assert!(!evaluate_blank_recovery(&mut t, &blank_grid(6), &painted));
        assert_eq!(t.streak, 1);
    }

    #[test]
    fn blank_recovery_ignores_painted_grid() {
        let mut t = BlankRecoveryState::default();
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
        assert_eq!(t.streak, 0);
    }

    #[test]
    fn blank_grid_with_live_content_repaints_from_mirror() {
        use bevy::ecs::message::Messages;
        use orzma_tty_renderer::prelude::TerminalGridPlugin;
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Messages<RequestPaneReseed>>();
        app.add_observer(repaint_pane_from_mirror);
        app.add_systems(
            Update,
            (attach_rescue_state, rescue_unpainted_panes).chain(),
        );

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
    fn attach_inserts_both_state_components_once() {
        use tmux_control_parser::CellDims;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, attach_rescue_state);

        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 4,
                    height: 2,
                    xoff: 0,
                    yoff: 0,
                },
            })
            .id();

        app.update();

        assert!(
            app.world().get::<StructuralReseedState>(pane).is_some(),
            "attach inserts StructuralReseedState",
        );
        assert!(
            app.world().get::<BlankRecoveryState>(pane).is_some(),
            "attach inserts BlankRecoveryState",
        );

        // NOTE: prove idempotency — mutate the state, then a second pass must NOT
        // re-insert (which would reset it); the Without<StructuralReseedState> filter
        // must exclude the already-attached pane.
        app.world_mut()
            .get_mut::<StructuralReseedState>(pane)
            .unwrap()
            .unpainted_streak = 7;
        app.update();
        assert_eq!(
            app.world()
                .get::<StructuralReseedState>(pane)
                .unwrap()
                .unpainted_streak,
            7,
            "second pass must not re-insert (a re-insert would reset the streak to 0)",
        );
    }

    #[test]
    fn blank_grid_with_blank_mirror_is_not_repainted() {
        use bevy::ecs::message::Messages;
        use orzma_tty_renderer::prelude::TerminalGridPlugin;
        use orzma_tty_renderer::schema::FrameSnapshot;
        use tmux_control_parser::CellDims;

        #[derive(Resource, Default)]
        struct SnapHits(u32);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Messages<RequestPaneReseed>>();
        app.init_resource::<SnapHits>();
        app.add_observer(|_snap: On<FrameSnapshot>, mut hits: ResMut<SnapHits>| {
            hits.0 += 1;
        });
        app.add_observer(repaint_pane_from_mirror);
        app.add_systems(
            Update,
            (attach_rescue_state, rescue_unpainted_panes).chain(),
        );

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

    #[test]
    fn steady_pane_does_not_re_mark_state_changed_each_frame() {
        use bevy::ecs::message::Messages;
        use orzma_tty_renderer::prelude::TerminalGridPlugin;
        use tmux_control_parser::CellDims;

        #[derive(Resource, Default)]
        struct ChangedHits {
            reseed: u32,
            blank: u32,
        }

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Messages<RequestPaneReseed>>();
        app.init_resource::<ChangedHits>();
        app.add_observer(repaint_pane_from_mirror);
        app.add_systems(
            Update,
            (
                attach_rescue_state,
                rescue_unpainted_panes,
                |reseed: Query<(), Changed<StructuralReseedState>>,
                 blank: Query<(), Changed<BlankRecoveryState>>,
                 mut hits: ResMut<ChangedHits>| {
                    hits.reseed += reseed.iter().count() as u32;
                    hits.blank += blank.iter().count() as u32;
                },
            )
                .chain(),
        );

        // A painted, non-vi-mode pane in steady state: grid has a glyph and the
        // handle geometry matches the grid dims, so neither the structural reseed
        // nor the blank-recovery path mutates after it first settles.
        let mut handle = TerminalHandle::detached(4, 2);
        handle.advance(b"hi");
        app.world_mut().spawn((
            TmuxPane {
                id: PaneId(1),
                dims: CellDims {
                    width: 4,
                    height: 2,
                    xoff: 0,
                    yoff: 0,
                },
            },
            handle,
            TerminalGrid {
                cols: 4,
                rows: 2,
                cells: vec![vec![cell("h"), cell("i")], vec![cell(" ")]],
                ..Default::default()
            },
        ));

        // Let the attach "added" tick and the first settling write clear.
        for _ in 0..4 {
            app.update();
        }
        {
            let mut hits = app.world_mut().resource_mut::<ChangedHits>();
            hits.reseed = 0;
            hits.blank = 0;
        }
        // Now steady: further frames must not re-mark either component Changed.
        for _ in 0..3 {
            app.update();
        }
        let hits = app.world().resource::<ChangedHits>();
        assert_eq!(
            (hits.reseed, hits.blank),
            (0, 0),
            "a steady pane must not re-mark its rescue components Changed each frame \
             (conditional write-back), got reseed={} blank={}",
            hits.reseed,
            hits.blank,
        );
    }
}
