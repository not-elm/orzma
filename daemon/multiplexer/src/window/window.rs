use crate::error::{MultiplexerError, MultiplexerResult};
use crate::window::cells::{CellId, LayoutCellState, Side, SplitOrientation};
use crate::window::pane::activity::{Activity, ActivityId};
use crate::window::pane::{Pane, PaneId, PaneState, SetActiveOutcome};
use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct WindowId(String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub id: WindowId,
    pub name: String,
    pub cells: LayoutCellState,
    pub panes: PaneState,
    pub pane_to_cell: HashMap<PaneId, CellId>,
    pub root_cell: CellId,
    pub active_pane: PaneId,
    pub(crate) pane_active_points: HashMap<PaneId, u64>,
    pub(crate) next_active_point: u64,
}

impl Window {
    /// Construct a Window containing one initial Pane with one initial Activity.
    /// The caller supplies the ids (typically generated client-side as UUIDv4).
    pub fn new_with_initial(
        id: WindowId,
        name: String,
        initial_pane_id: PaneId,
        initial_activity: Activity,
    ) -> Self {
        let mut cells = LayoutCellState::default();
        let (root_cell, pane_cell_id) = cells.new_window_layout(initial_pane_id.clone());
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
        }
    }

    /// Bump the per-window counter and record it as the new active pane's
    /// activation point. The tiebreak in `Window::pane_in_direction` reads
    /// from `pane_active_points`; a missing entry is treated as `0`, so we
    /// only need to insert when a pane actually becomes active.
    fn record_active_point(&mut self, pane_id: &PaneId) {
        self.next_active_point += 1;
        self.pane_active_points
            .insert(pane_id.clone(), self.next_active_point);
    }

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
    /// because the whole method runs under the per-Window lock.
    ///
    /// # Errors
    ///
    /// - `PaneIdConflict` — `new_pane_id` is already in use.
    /// - `PaneNotFound` — `target_pane_id` is not in this Window.
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

    /// Set the active pane in this Window. Returns `Unchanged` so the caller
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

    /// Collect every ActivityId across every Pane. Used at window-close time
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

    pub fn pane_ids(&self) -> impl Iterator<Item = &PaneId> {
        self.panes.ids()
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WindowState(HashMap<WindowId, Window>);

impl WindowState {
    #[inline]
    pub fn insert(&mut self, window: Window) {
        self.0.insert(window.id.clone(), window);
    }

    #[inline]
    pub fn get(&self, id: &WindowId) -> Option<&Window> {
        self.0.get(id)
    }

    #[inline]
    pub fn get_mut(&mut self, id: &WindowId) -> Option<&mut Window> {
        self.0.get_mut(id)
    }

    #[inline]
    pub fn remove(&mut self, id: &WindowId) -> Option<Window> {
        self.0.remove(id)
    }

    #[inline]
    pub fn contains_key(&self, id: &WindowId) -> bool {
        self.0.contains_key(id)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&WindowId, &Window)> {
        self.0.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&WindowId, &mut Window)> {
        self.0.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::cells::{Side, SplitOrientation};
    use crate::window::pane::activity::{Activity, ActivityId};

    fn window_with_two_activities() -> (Window, PaneId, ActivityId, ActivityId) {
        let pid = PaneId::new();
        let a0 = Activity::terminal(ActivityId::new());
        let a1 = Activity::terminal(ActivityId::new());
        let a0_id = a0.id.clone();
        let a1_id = a1.id.clone();
        let mut win = Window::new_with_initial(WindowId::new(), "w".into(), pid.clone(), a0);
        win.pane_mut(&pid).unwrap().add_activity(a1).unwrap();
        (win, pid, a0_id, a1_id)
    }

    #[test]
    fn break_activity_moves_active_activity_into_new_pane() {
        let (mut win, pid, a0_id, a1_id) = window_with_two_activities();
        let _ = win
            .pane_mut(&pid)
            .unwrap()
            .set_active_activity(&a1_id)
            .unwrap();
        let new_pid = PaneId::new();

        win.break_activity_to_pane(
            &pid,
            &a1_id,
            new_pid.clone(),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();

        let src = win.pane(&pid).unwrap();
        assert!(
            !src.has_activity(&a1_id),
            "moved activity left the source pane"
        );
        assert_eq!(
            src.active_activity, a0_id,
            "source active falls back to first remaining"
        );

        let new_pane = win.pane(&new_pid).unwrap();
        assert_eq!(new_pane.activities.len(), 1);
        assert!(new_pane.has_activity(&a1_id));
        assert_eq!(new_pane.active_activity, a1_id);

        assert_eq!(win.active_pane, new_pid, "new pane becomes active");
    }

    #[test]
    fn break_activity_preserves_remaining_order_when_moving_non_active() {
        let (mut win, pid, a0_id, a1_id) = window_with_two_activities();
        win.break_activity_to_pane(
            &pid,
            &a1_id,
            PaneId::new(),
            Side::After,
            SplitOrientation::Vertical,
        )
        .unwrap();
        let src = win.pane(&pid).unwrap();
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
        let mut win = Window::new_with_initial(WindowId::new(), "w".into(), pid.clone(), only);
        let err = win
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
        let (mut win, pid, _a0, _a1) = window_with_two_activities();
        let err = win
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
        let (mut win, pid, _a0, a1_id) = window_with_two_activities();
        let err = win
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

    fn fresh_window() -> (Window, PaneId, ActivityId) {
        let wid = WindowId::new();
        let pid = PaneId::new();
        let aid = ActivityId::new();
        let activity = Activity::terminal(aid.clone());
        let win = Window::new_with_initial(wid, "w".into(), pid.clone(), activity);
        (win, pid, aid)
    }

    #[test]
    fn split_pane_inserts_new_pane_and_promotes_active() {
        let (mut win, original_pid, _) = fresh_window();
        let new_pid = PaneId::new();
        let new_aid = ActivityId::new();
        let activity = Activity::terminal(new_aid.clone());
        win.split_pane(
            &original_pid,
            new_pid.clone(),
            activity,
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        assert_eq!(win.panes.len(), 2);
        assert_eq!(win.active_pane, new_pid);
        let new_pane = win.panes.get(&new_pid).unwrap();
        assert_eq!(new_pane.activities.len(), 1);
        assert_eq!(new_pane.active_activity, new_aid);
    }

    #[test]
    fn split_pane_with_duplicate_id_returns_pane_id_conflict() {
        let (mut win, original_pid, _) = fresh_window();
        let activity = Activity::terminal(ActivityId::new());
        let err = win
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
        let (mut win, original_pid, _) = fresh_window();
        let new_pid = PaneId::new();
        let new_aid = ActivityId::new();
        win.split_pane(
            &original_pid,
            new_pid.clone(),
            Activity::terminal(new_aid.clone()),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        let destroyed = win.close_pane(&new_pid).unwrap();
        assert_eq!(destroyed, vec![new_aid]);
        assert_eq!(win.panes.len(), 1);
        assert_eq!(win.active_pane, original_pid);
    }

    #[test]
    fn close_last_pane_returns_cannot_close_last_pane() {
        let (mut win, pid, _) = fresh_window();
        let err = win.close_pane(&pid).unwrap_err();
        assert!(matches!(err, MultiplexerError::CannotCloseLastPane(_)));
    }

    fn sample_window() -> Window {
        Window::new_with_initial(
            WindowId::new(),
            "test".into(),
            PaneId::new(),
            Activity::terminal(ActivityId::new()),
        )
    }

    #[test]
    fn new_window_starts_with_empty_active_point_table() {
        let win = sample_window();
        assert_eq!(win.next_active_point, 0);
        assert!(win.pane_active_points.is_empty());
    }

    #[test]
    fn set_active_pane_records_active_point() {
        let mut win = sample_window();
        let original = win.active_pane.clone();
        let other = PaneId::new();
        win.split_pane(
            &original,
            other.clone(),
            Activity::terminal(ActivityId::new()),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        let first_point = *win.pane_active_points.get(&other).unwrap();
        assert!(first_point >= 1);

        win.set_active_pane(&original).unwrap();
        let original_point = *win.pane_active_points.get(&original).unwrap();
        assert!(
            original_point > first_point,
            "switching back must increment past the previous max",
        );
    }

    #[test]
    fn set_active_pane_unchanged_does_not_bump_counter() {
        let mut win = sample_window();
        let active = win.active_pane.clone();
        let before = win.next_active_point;
        win.set_active_pane(&active).unwrap();
        assert_eq!(win.next_active_point, before);
    }

    #[test]
    fn close_pane_removes_active_point_entry() {
        let mut win = sample_window();
        let original = win.active_pane.clone();
        let new_pane = PaneId::new();
        win.split_pane(
            &original,
            new_pane.clone(),
            Activity::terminal(ActivityId::new()),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        assert!(win.pane_active_points.contains_key(&new_pane));
        win.close_pane(&new_pane).unwrap();
        assert!(!win.pane_active_points.contains_key(&new_pane));
    }
}
