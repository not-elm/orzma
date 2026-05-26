use crate::error::{MultiplexerError, MultiplexerResult};
use crate::session::cells::{CellId, LayoutCellState, Side, SplitOrientation};
use crate::session::pane::activity::{Activity, ActivityId};
use crate::session::pane::{Pane, PaneId, PaneState, SetActiveOutcome};
use crate::session::swap::{SwapOffset, SwapOutcome};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Stable identifier for a Session. Minted by `MultiplexerService::create_session`
/// as a monotonically increasing counter. Bevy-free — the GUI side wraps this
/// in `SessionEntityId` for use as a Component.
#[derive(
    Debug,
    Clone,
    Copy,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
    derive_more::Display,
)]
pub struct SessionId(pub u32);

/// Cell-grid dimensions of a Session's outer container, as reported by
/// the renderer. Used as the root `P` for the resize-pane algorithm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionDimensions {
    pub cols: u16,
    pub rows: u16,
}

/// Owns the cell tree, panes, and active-pane pointer for one Session.
///
/// Carries the per-Session active-point counter used by `pane_in_direction`
/// to tiebreak adjacent panes by most-recent activation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub cells: LayoutCellState,
    pub panes: PaneState,
    pub pane_to_cell: HashMap<PaneId, CellId>,
    pub root_cell: CellId,
    pub active_pane: PaneId,
    pub(crate) pane_active_points: HashMap<PaneId, u64>,
    pub(crate) next_active_point: u64,
    /// Cell-grid dimensions reported by the renderer. `None` until the
    /// first measurement.
    #[serde(default)]
    pub dimensions: Option<SessionDimensions>,
}

impl Session {
    /// Construct a Session containing one initial Pane with one initial Activity.
    /// The caller supplies the ids (typically generated upstream as UUIDv4).
    pub fn new_with_initial(
        id: SessionId,
        name: String,
        initial_pane_id: PaneId,
        initial_activity: Activity,
    ) -> Self {
        let mut cells = LayoutCellState::default();
        let (root_cell, pane_cell_id) = cells.new_session_layout(initial_pane_id.clone());
        let mut panes = PaneState::default();
        panes.insert(Pane::new(initial_pane_id.clone(), initial_activity));
        let mut pane_to_cell = HashMap::new();
        pane_to_cell.insert(initial_pane_id.clone(), pane_cell_id);

        Self {
            id,
            name,
            cells,
            panes,
            pane_to_cell,
            root_cell,
            active_pane: initial_pane_id,
            pane_active_points: HashMap::new(),
            next_active_point: 0,
            dimensions: None,
        }
    }

    /// Replace the cached dimensions. Set on first measurement and
    /// updated on subsequent container resizes.
    pub fn set_dimensions(&mut self, cols: u16, rows: u16) {
        self.dimensions = Some(SessionDimensions { cols, rows });
    }

    /// Bump the per-session counter and record it as the new active pane's
    /// activation point. The tiebreak in `Session::pane_in_direction` reads
    /// from `pane_active_points`; a missing entry is treated as `0`, so we
    /// only need to insert when a pane actually becomes active.
    fn record_active_point(&mut self, pane_id: &PaneId) {
        self.next_active_point += 1;
        self.pane_active_points
            .insert(pane_id.clone(), self.next_active_point);
    }

    /// Replace the Session's display name in-place.
    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Split a target Pane, placing a new Pane next to it. The new Pane gets
    /// exactly one Activity. Both the new pane id and the new activity id are
    /// supplied by the caller (UUID-validated upstream).
    pub fn split_pane(
        &mut self,
        target_pane_id: &PaneId,
        new_pane_id: PaneId,
        new_activity: Activity,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<()> {
        if !self.panes.contains_key(target_pane_id) {
            return Err(MultiplexerError::PaneNotFound(target_pane_id.clone()));
        }
        if self.panes.contains_key(&new_pane_id) {
            return Err(MultiplexerError::PaneIdConflict(new_pane_id));
        }
        let target_cell_id = self
            .pane_to_cell
            .get(target_pane_id)
            .ok_or_else(|| MultiplexerError::CellForPaneNotFound(target_pane_id.clone()))?
            .clone();
        let new_cell_id = self.cells.new_pane(new_pane_id.clone(), None);
        if let Err(e) =
            self.cells
                .split_cell(target_cell_id, new_cell_id.clone(), side, orientation)
        {
            let _ = self.cells.remove_subtree(&new_cell_id);
            return Err(e);
        }
        self.pane_to_cell.insert(new_pane_id.clone(), new_cell_id);
        let pane = Pane::new(new_pane_id.clone(), new_activity);
        self.panes.insert(pane);
        self.active_pane = new_pane_id.clone();
        self.record_active_point(&new_pane_id);
        Ok(())
    }

    /// Split `target_pane_id` and move the Activity `aid` out of it into the
    /// freshly-created Pane.
    ///
    /// The moved Activity becomes the sole Activity of the new Pane, and the
    /// new Pane becomes `active_pane`. The Activity is *cloned* into the new
    /// Pane first and removed from the source Pane second, so the only
    /// fallible mutation (`split_pane`) runs before the irreversible one and
    /// no rollback path is needed. Between those two steps the same
    /// `ActivityId` is briefly present in both Panes; this is safe only
    /// because the whole method runs under the per-Session lock.
    ///
    /// # Errors
    ///
    /// - `PaneIdConflict` — `new_pane_id` is already in use.
    /// - `PaneNotFound` — `target_pane_id` is not in this Session.
    /// - `ActivityNotInPane` — `aid` is not an Activity of the target Pane.
    /// - `CannotRemoveLastActivity` — the target Pane holds only `aid`.
    pub fn break_activity_to_pane(
        &mut self,
        target_pane_id: &PaneId,
        aid: &ActivityId,
        new_pane_id: PaneId,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<()> {
        // NOTE: `new_pane_id` is not pre-checked here — `split_pane` rejects
        // a collision before any mutation, which preserves the rollback-free
        // ordering and avoids a duplicate `HashMap::contains_key` lookup.
        let target = self.pane(target_pane_id)?;
        let moved = match target.activity(aid) {
            Some(activity) => activity.clone(),
            None => {
                return Err(MultiplexerError::ActivityNotInPane {
                    pane: target_pane_id.clone(),
                    activity: aid.clone(),
                });
            }
        };
        if target.activities.len() == 1 {
            return Err(MultiplexerError::CannotRemoveLastActivity(
                target_pane_id.clone(),
            ));
        }
        self.split_pane(target_pane_id, new_pane_id, moved, side, orientation)?;
        self.pane_mut(target_pane_id)?.remove_activity(aid)?;
        Ok(())
    }

    /// Close a Pane. Returns the ids of the activities that were destroyed,
    /// so the caller can tear down PTYs and extension registry entries.
    pub fn close_pane(&mut self, pane_id: &PaneId) -> MultiplexerResult<Vec<ActivityId>> {
        if !self.panes.contains_key(pane_id) {
            return Err(MultiplexerError::PaneNotFound(pane_id.clone()));
        }
        let cell_id = self
            .pane_to_cell
            .get(pane_id)
            .ok_or_else(|| MultiplexerError::CellForPaneNotFound(pane_id.clone()))?
            .clone();
        let outcome = self.cells.close_cell(&cell_id)?;
        let survivor_pane_id = self.cells.leftmost_pane(outcome.survivor())?.clone();
        if &self.active_pane == pane_id {
            self.active_pane = survivor_pane_id.clone();
            self.record_active_point(&survivor_pane_id);
        }
        let pane = self.panes.remove(pane_id)?;
        self.pane_to_cell.remove(pane_id);
        self.pane_active_points.remove(pane_id);
        Ok(pane.activities.into_iter().map(|a| a.id).collect())
    }

    /// Swap the named pane's contents with its previous or next neighbor in
    /// the depth-first leaf traversal of the cell tree. Returns
    /// `SwapOutcome::NoOp` for a single-pane session. The active pane id is
    /// not mutated — the same `PaneId` is now hosted by a different cell,
    /// so focus visually follows the swap.
    pub fn swap_pane(
        &mut self,
        pane: &PaneId,
        offset: SwapOffset,
    ) -> MultiplexerResult<SwapOutcome> {
        if self.panes.len() < 2 {
            return Ok(SwapOutcome::NoOp);
        }
        let ordered = self.cells.ordered_pane_cells(&self.root_cell)?;
        let i = ordered
            .iter()
            .position(|(_, p)| p == pane)
            .ok_or_else(|| MultiplexerError::PaneNotFound(pane.clone()))?;
        let len = ordered.len() as isize;
        let delta: isize = match offset {
            SwapOffset::Prev => -1,
            SwapOffset::Next => 1,
        };
        let j = ((i as isize + delta).rem_euclid(len)) as usize;

        let cell_i = ordered[i].0.clone();
        let cell_j = ordered[j].0.clone();
        let other_pane = ordered[j].1.clone();

        self.cells.swap_panes(&cell_i, &cell_j)?;
        self.pane_to_cell.insert(pane.clone(), cell_j);
        self.pane_to_cell.insert(other_pane.clone(), cell_i);

        Ok(SwapOutcome::Swapped { other_pane })
    }

    /// Set the active pane in this Session. Returns `Unchanged` so the caller
    /// can skip a redundant broadcast.
    pub fn set_active_pane(&mut self, pane_id: &PaneId) -> MultiplexerResult<SetActiveOutcome> {
        if !self.panes.contains_key(pane_id) {
            return Err(MultiplexerError::PaneNotFound(pane_id.clone()));
        }
        if &self.active_pane == pane_id {
            return Ok(SetActiveOutcome::Unchanged);
        }
        self.active_pane = pane_id.clone();
        self.record_active_point(pane_id);
        Ok(SetActiveOutcome::Changed)
    }

    /// Collect every ActivityId across every Pane. Used at session-close time
    /// to enumerate runtime resources that need cleanup.
    pub fn collect_activities_for_cleanup(&self) -> Vec<ActivityId> {
        self.panes
            .iter()
            .flat_map(|(_, p)| p.activity_ids().cloned())
            .collect()
    }

    /// Read-only Pane accessor.
    pub fn pane(&self, pid: &PaneId) -> MultiplexerResult<&Pane> {
        self.panes
            .get(pid)
            .ok_or_else(|| MultiplexerError::PaneNotFound(pid.clone()))
    }

    /// Mutable Pane accessor used to chain into Pane methods (add_activity,
    /// set_active_activity).
    pub fn pane_mut(&mut self, pid: &PaneId) -> MultiplexerResult<&mut Pane> {
        self.panes
            .get_mut(pid)
            .ok_or_else(|| MultiplexerError::PaneNotFound(pid.clone()))
    }

    /// Iterate over all PaneIds in this Session in insertion order.
    pub fn pane_ids(&self) -> impl Iterator<Item = &PaneId> {
        self.panes.ids()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::cells::{Side, SplitOrientation};
    use crate::session::pane::activity::{Activity, ActivityId};

    fn session_with_two_activities() -> (Session, PaneId, ActivityId, ActivityId) {
        let pid = PaneId::new();
        let a0 = Activity::terminal(ActivityId::new());
        let a1 = Activity::terminal(ActivityId::new());
        let a0_id = a0.id.clone();
        let a1_id = a1.id.clone();
        let mut s = Session::new_with_initial(SessionId(0), "s".into(), pid.clone(), a0);
        s.pane_mut(&pid).unwrap().add_activity(a1).unwrap();
        (s, pid, a0_id, a1_id)
    }

    #[test]
    fn break_activity_moves_active_activity_into_new_pane() {
        let (mut s, pid, a0_id, a1_id) = session_with_two_activities();
        let _ = s
            .pane_mut(&pid)
            .unwrap()
            .set_active_activity(&a1_id)
            .unwrap();
        let new_pid = PaneId::new();

        s.break_activity_to_pane(
            &pid,
            &a1_id,
            new_pid.clone(),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();

        let src = s.pane(&pid).unwrap();
        assert!(
            !src.has_activity(&a1_id),
            "moved activity left the source pane"
        );
        assert_eq!(
            src.active_activity, a0_id,
            "source active falls back to first remaining"
        );

        let new_pane = s.pane(&new_pid).unwrap();
        assert_eq!(new_pane.activities.len(), 1);
        assert!(new_pane.has_activity(&a1_id));
        assert_eq!(new_pane.active_activity, a1_id);

        assert_eq!(s.active_pane, new_pid, "new pane becomes active");
    }

    #[test]
    fn break_activity_preserves_remaining_order_when_moving_non_active() {
        let (mut s, pid, a0_id, a1_id) = session_with_two_activities();
        s.break_activity_to_pane(
            &pid,
            &a1_id,
            PaneId::new(),
            Side::After,
            SplitOrientation::Vertical,
        )
        .unwrap();
        let src = s.pane(&pid).unwrap();
        assert_eq!(src.activities.len(), 1);
        assert_eq!(src.activities[0].id, a0_id);
        assert_eq!(
            src.active_activity, a0_id,
            "non-active move leaves source active unchanged"
        );
    }

    #[test]
    fn break_activity_rejects_single_activity_pane() {
        let pid = PaneId::new();
        let only = Activity::terminal(ActivityId::new());
        let only_id = only.id.clone();
        let mut s = Session::new_with_initial(SessionId(0), "s".into(), pid.clone(), only);
        let err = s
            .break_activity_to_pane(
                &pid,
                &only_id,
                PaneId::new(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::CannotRemoveLastActivity(_)));
    }

    #[test]
    fn break_activity_rejects_unknown_activity() {
        let (mut s, pid, _a0, _a1) = session_with_two_activities();
        let err = s
            .break_activity_to_pane(
                &pid,
                &ActivityId::new(),
                PaneId::new(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::ActivityNotInPane { .. }));
    }

    #[test]
    fn break_activity_rejects_duplicate_new_pane_id() {
        let (mut s, pid, _a0, a1_id) = session_with_two_activities();
        let err = s
            .break_activity_to_pane(
                &pid,
                &a1_id,
                pid.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::PaneIdConflict(_)));
    }

    fn fresh_session() -> (Session, PaneId, ActivityId) {
        let sid = SessionId(0);
        let pid = PaneId::new();
        let aid = ActivityId::new();
        let activity = Activity::terminal(aid.clone());
        let s = Session::new_with_initial(sid, "s".into(), pid.clone(), activity);
        (s, pid, aid)
    }

    #[test]
    fn split_pane_inserts_new_pane_and_promotes_active() {
        let (mut s, original_pid, _) = fresh_session();
        let new_pid = PaneId::new();
        let new_aid = ActivityId::new();
        let activity = Activity::terminal(new_aid.clone());
        s.split_pane(
            &original_pid,
            new_pid.clone(),
            activity,
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        assert_eq!(s.panes.len(), 2);
        assert_eq!(s.active_pane, new_pid);
        let new_pane = s.panes.get(&new_pid).unwrap();
        assert_eq!(new_pane.activities.len(), 1);
        assert_eq!(new_pane.active_activity, new_aid);
    }

    #[test]
    fn split_pane_with_duplicate_id_returns_pane_id_conflict() {
        let (mut s, original_pid, _) = fresh_session();
        let activity = Activity::terminal(ActivityId::new());
        let err = s
            .split_pane(
                &original_pid,
                original_pid.clone(),
                activity,
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::PaneIdConflict(_)));
    }

    #[test]
    fn close_pane_returns_destroyed_activity_ids() {
        let (mut s, original_pid, _) = fresh_session();
        let new_pid = PaneId::new();
        let new_aid = ActivityId::new();
        s.split_pane(
            &original_pid,
            new_pid.clone(),
            Activity::terminal(new_aid.clone()),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        let destroyed = s.close_pane(&new_pid).unwrap();
        assert_eq!(destroyed, vec![new_aid]);
        assert_eq!(s.panes.len(), 1);
        assert_eq!(s.active_pane, original_pid);
    }

    #[test]
    fn close_last_pane_returns_cannot_close_last_pane() {
        let (mut s, pid, _) = fresh_session();
        let err = s.close_pane(&pid).unwrap_err();
        assert!(matches!(err, MultiplexerError::CannotCloseLastPane(_)));
    }

    #[test]
    fn new_session_starts_with_empty_active_point_table() {
        let (s, _, _) = fresh_session();
        assert_eq!(s.next_active_point, 0);
        assert!(s.pane_active_points.is_empty());
    }

    #[test]
    fn set_active_pane_records_active_point() {
        let (mut s, original, _) = fresh_session();
        let other = PaneId::new();
        s.split_pane(
            &original,
            other.clone(),
            Activity::terminal(ActivityId::new()),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        let first_point = *s.pane_active_points.get(&other).unwrap();
        assert!(first_point >= 1);

        assert!(matches!(
            s.set_active_pane(&original).unwrap(),
            SetActiveOutcome::Changed,
        ));
        let original_point = *s.pane_active_points.get(&original).unwrap();
        assert!(
            original_point > first_point,
            "switching back must increment past the previous max",
        );
    }

    #[test]
    fn set_active_pane_unchanged_does_not_bump_counter() {
        let (mut s, active, _) = fresh_session();
        let before = s.next_active_point;
        assert!(matches!(
            s.set_active_pane(&active).unwrap(),
            SetActiveOutcome::Unchanged,
        ));
        assert_eq!(s.next_active_point, before);
    }

    #[test]
    fn new_session_starts_with_unset_dimensions() {
        let (s, _, _) = fresh_session();
        assert!(s.dimensions.is_none());
    }

    #[test]
    fn session_set_dimensions_stores_cols_and_rows() {
        let (mut s, _, _) = fresh_session();
        s.set_dimensions(120, 40);
        assert_eq!(
            s.dimensions,
            Some(SessionDimensions {
                cols: 120,
                rows: 40,
            })
        );
    }

    #[test]
    fn close_pane_removes_active_point_entry() {
        let (mut s, original, _) = fresh_session();
        let new_pane = PaneId::new();
        s.split_pane(
            &original,
            new_pane.clone(),
            Activity::terminal(ActivityId::new()),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        assert!(s.pane_active_points.contains_key(&new_pane));
        s.close_pane(&new_pane).unwrap();
        assert!(!s.pane_active_points.contains_key(&new_pane));
    }
}

#[cfg(test)]
mod swap_tests {
    use super::*;
    use crate::session::cells::{Side, SplitOrientation};
    use crate::session::pane::activity::{Activity, ActivityId};
    use crate::session::swap::{SwapOffset, SwapOutcome};

    fn three_pane_session() -> (Session, PaneId, PaneId, PaneId) {
        let sid = SessionId(0);
        let pa = PaneId::new();
        let aa = Activity::terminal(ActivityId::new());
        let mut s = Session::new_with_initial(sid, "s".into(), pa.clone(), aa);

        let pb = PaneId::new();
        let ab = Activity::terminal(ActivityId::new());
        s.split_pane(
            &pa,
            pb.clone(),
            ab,
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();

        let pc = PaneId::new();
        let ac = Activity::terminal(ActivityId::new());
        s.split_pane(&pb, pc.clone(), ac, Side::After, SplitOrientation::Vertical)
            .unwrap();

        (s, pa, pb, pc)
    }

    fn pane_order(s: &Session) -> Vec<PaneId> {
        s.cells
            .ordered_pane_cells(&s.root_cell)
            .unwrap()
            .into_iter()
            .map(|(_, p)| p)
            .collect()
    }

    #[test]
    fn swap_pane_in_single_pane_session_returns_noop() {
        let sid = SessionId(0);
        let pa = PaneId::new();
        let aa = Activity::terminal(ActivityId::new());
        let mut s = Session::new_with_initial(sid, "s".into(), pa.clone(), aa);

        let out = s.swap_pane(&pa, SwapOffset::Next).unwrap();
        assert_eq!(out, SwapOutcome::NoOp);
        let out = s.swap_pane(&pa, SwapOffset::Prev).unwrap();
        assert_eq!(out, SwapOutcome::NoOp);
    }

    #[test]
    fn swap_pane_next_moves_active_pane_one_slot_forward() {
        let (mut s, pa, pb, pc) = three_pane_session();
        assert_eq!(pane_order(&s), vec![pa.clone(), pb.clone(), pc.clone()]);

        let out = s.swap_pane(&pa, SwapOffset::Next).unwrap();
        assert_eq!(
            out,
            SwapOutcome::Swapped {
                other_pane: pb.clone()
            }
        );
        assert_eq!(pane_order(&s), vec![pb.clone(), pa.clone(), pc]);
    }

    #[test]
    fn swap_pane_prev_wraps_around_from_first() {
        let (mut s, pa, pb, pc) = three_pane_session();
        let out = s.swap_pane(&pa, SwapOffset::Prev).unwrap();
        assert_eq!(
            out,
            SwapOutcome::Swapped {
                other_pane: pc.clone()
            }
        );
        assert_eq!(pane_order(&s), vec![pc, pb, pa]);
    }

    #[test]
    fn swap_pane_next_wraps_around_from_last() {
        let (mut s, pa, pb, pc) = three_pane_session();
        let out = s.swap_pane(&pc, SwapOffset::Next).unwrap();
        assert_eq!(
            out,
            SwapOutcome::Swapped {
                other_pane: pa.clone()
            }
        );
        assert_eq!(pane_order(&s), vec![pc, pb, pa]);
    }

    #[test]
    fn swap_pane_prev_is_inverse_of_next() {
        let (mut s, _pa, pb, _pc) = three_pane_session();
        let before = pane_order(&s);
        s.swap_pane(&pb, SwapOffset::Next).unwrap();
        s.swap_pane(&pb, SwapOffset::Prev).unwrap();
        assert_eq!(pane_order(&s), before);
    }

    #[test]
    fn swap_pane_preserves_active_pane_id_and_pane_to_cell_bijection() {
        let (mut s, pa, pb, _pc) = three_pane_session();
        let active_before = s.active_pane.clone();
        s.swap_pane(&pa, SwapOffset::Next).unwrap();
        assert_eq!(s.active_pane, active_before);

        for (cell_id, pane_id) in s.cells.ordered_pane_cells(&s.root_cell).unwrap() {
            assert_eq!(s.pane_to_cell.get(&pane_id), Some(&cell_id));
        }
        let _ = pb;
    }

    #[test]
    fn swap_pane_unknown_pane_returns_pane_not_found() {
        let (mut s, _pa, _pb, _pc) = three_pane_session();
        let stranger = PaneId::new();
        let err = s.swap_pane(&stranger, SwapOffset::Next).unwrap_err();
        assert!(matches!(err, MultiplexerError::PaneNotFound(_)));
    }

    #[test]
    fn swap_pane_with_two_panes_prev_and_next_produce_same_state() {
        let sid = SessionId(0);
        let pa = PaneId::new();
        let aa = Activity::terminal(ActivityId::new());
        let mut sn = Session::new_with_initial(sid, "s".into(), pa.clone(), aa);
        let pb = PaneId::new();
        let ab = Activity::terminal(ActivityId::new());
        sn.split_pane(
            &pa,
            pb.clone(),
            ab,
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();

        let aa2 = Activity::terminal(ActivityId::new());
        let mut sp = Session::new_with_initial(sid, "s".into(), pa.clone(), aa2);
        let ab2 = Activity::terminal(ActivityId::new());
        sp.split_pane(
            &pa,
            pb.clone(),
            ab2,
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();

        sn.swap_pane(&pa, SwapOffset::Next).unwrap();
        sp.swap_pane(&pa, SwapOffset::Prev).unwrap();
        assert_eq!(pane_order(&sn), pane_order(&sp));
    }
}
