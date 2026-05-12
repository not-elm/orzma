use std::collections::HashMap;

pub mod error;
pub mod session;
pub mod window;

pub use error::{MultiplexerError, MultiplexerResult};
pub use session::{Session, SessionId, SessionState};
pub use window::{
    Activity, ActivityId, ActivityKind, Cell, CellId, CloseOutcome, LayoutCellState, Pane,
    PaneCell, PaneId, PaneState, RootCell, SetActiveOutcome, Side, SplitCell, SplitOrientation,
    Window, WindowId, WindowState,
};

/// Backwards-compatible alias for the active-pane outcome. Use
/// `SetActiveOutcome` directly in new code.
pub type SetActivePaneOutcome = SetActiveOutcome;

#[derive(Default)]
pub struct MultiplexerService {
    sessions: SessionState,
    windows: WindowState,
    // Transitional: limbo activities / panes used by the pre-PR5 SDK flow
    // (createActivity → createPane → splitPane). Removed in PR7 when the
    // legacy split-with API disappears.
    limbo_activities: HashMap<ActivityId, Activity>,
    limbo_panes: HashMap<PaneId, ActivityId>,
}

impl MultiplexerService {
    pub fn sessions(&self) -> &SessionState {
        &self.sessions
    }

    pub fn windows(&self) -> &WindowState {
        &self.windows
    }

    pub fn new_session(&mut self, name: Option<String>) -> SessionId {
        let session_id = SessionId::new();
        let session_name = name.unwrap_or_else(|| format!("Session{}", self.sessions.len() + 1));
        self.sessions
            .register(session_id.clone(), Session::empty(session_name));
        session_id
    }

    pub fn rename_session(&mut self, session_id: &SessionId, name: String) -> MultiplexerResult {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| MultiplexerError::SessionNotFound(session_id.clone()))?;
        session.rename(name);
        Ok(())
    }

    /// Delete a Session. Cascades by closing every Window it owns.
    /// Returns every ActivityId destroyed so the caller can tear down PTYs.
    pub fn delete_session(&mut self, session_id: &SessionId) -> MultiplexerResult<Vec<ActivityId>> {
        let session = self.sessions.remove(session_id)?;
        let mut activities = Vec::new();
        for wid in session.linked_windows {
            activities.extend(self.close_window(&wid)?);
        }
        Ok(activities)
    }

    /// Create a new Window, optionally linked to a Session.
    /// The Window's initial pane and activity get server-generated ids
    /// (transitional — PR5 makes these caller-supplied).
    pub fn new_window_in(
        &mut self,
        session_id: Option<&SessionId>,
        name: Option<String>,
    ) -> MultiplexerResult<WindowId> {
        if let Some(sid) = session_id
            && self.sessions.get(sid).is_none()
        {
            return Err(MultiplexerError::SessionNotFound(sid.clone()));
        }
        let window_id = WindowId::new();
        let pane_id = PaneId::new();
        let activity = Activity::terminal(ActivityId::new());
        let window_name = name.unwrap_or_else(|| format!("Window{}", self.windows.len() + 1));
        let window = Window::new_with_initial(window_id.clone(), window_name, pane_id, activity);
        self.windows.insert(window);
        if let Some(sid) = session_id {
            let session = self
                .sessions
                .get_mut(sid)
                .expect("validated existence above");
            session.attach_window(window_id.clone());
        }
        Ok(window_id)
    }

    pub fn rename_window(&mut self, window_id: &WindowId, name: String) -> MultiplexerResult {
        let window = self
            .windows
            .get_mut(window_id)
            .ok_or_else(|| MultiplexerError::WindowNotFound(window_id.clone()))?;
        window.rename(name);
        Ok(())
    }

    /// Close a Window: remove its Pane / Activity tree and detach from any
    /// owning Session. Returns every ActivityId destroyed so the caller can
    /// kill PTYs.
    pub fn close_window(&mut self, window_id: &WindowId) -> MultiplexerResult<Vec<ActivityId>> {
        let activities = {
            let window = self
                .windows
                .get(window_id)
                .ok_or_else(|| MultiplexerError::WindowNotFound(window_id.clone()))?;
            window.collect_activities_for_cleanup()
        };
        self.windows.remove(window_id);
        for (_, session) in self.sessions.iter_mut() {
            session.detach_window(window_id);
        }
        Ok(activities)
    }

    /// Backwards-compatible: split a pane by PaneId only (no WindowId).
    /// Internally finds the owning window via a linear scan; PR3 makes this
    /// O(1) via the pane_owner_window index in AppState.
    pub fn split_pane(
        &mut self,
        target_pane_id: PaneId,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<(PaneId, ActivityId)> {
        let window = self
            .windows
            .find_window_with_pane_mut(&target_pane_id)
            .ok_or_else(|| MultiplexerError::PaneNotFound(target_pane_id.clone()))?;
        let new_pane_id = PaneId::new();
        let new_activity_id = ActivityId::new();
        let activity = Activity::terminal(new_activity_id.clone());
        window.split_pane(
            &target_pane_id,
            new_pane_id.clone(),
            activity,
            side,
            orientation,
        )?;
        Ok((new_pane_id, new_activity_id))
    }

    /// Backwards-compatible close_pane by PaneId only. Closing a limbo pane
    /// (created via `new_pane_with_activity` but not yet placed via
    /// `split_with_pane`) tears down the limbo entry without touching any
    /// cell tree.
    pub fn close_pane(&mut self, pane_id: &PaneId) -> MultiplexerResult<Vec<ActivityId>> {
        if let Some(activity_id) = self.limbo_panes.remove(pane_id) {
            self.limbo_activities.remove(&activity_id);
            return Ok(vec![activity_id]);
        }
        let window = self
            .windows
            .find_window_with_pane_mut(pane_id)
            .ok_or_else(|| MultiplexerError::PaneNotFound(pane_id.clone()))?;
        window.close_pane(pane_id)
    }

    /// Backwards-compatible: set active pane by (window_id, pane_id).
    pub fn set_active_pane(
        &mut self,
        window_id: &WindowId,
        pane_id: &PaneId,
    ) -> MultiplexerResult<SetActiveOutcome> {
        let window = self
            .windows
            .get_mut(window_id)
            .ok_or_else(|| MultiplexerError::WindowNotFound(window_id.clone()))?;
        if !window.panes.contains_key(pane_id) {
            // Distinguish "the pane exists in some other window" from "doesn't
            // exist anywhere at all" so callers see the right HTTP status.
            let pane_exists_elsewhere = self
                .windows
                .iter()
                .any(|(_, w)| w.panes.contains_key(pane_id));
            if pane_exists_elsewhere {
                return Err(MultiplexerError::PaneNotInWindow {
                    window: window_id.clone(),
                    pane: pane_id.clone(),
                });
            }
            return Err(MultiplexerError::PaneNotFound(pane_id.clone()));
        }
        window.set_active_pane(pane_id)
    }

    /// Backwards-compatible: select active window for the owning session.
    pub fn select_active_window(&mut self, window_id: &WindowId) -> MultiplexerResult {
        if !self.windows.contains_key(window_id) {
            return Err(MultiplexerError::WindowNotFound(window_id.clone()));
        }
        for (_, session) in self.sessions.iter_mut() {
            if session.linked_windows.contains(window_id) {
                session.active_window = Some(window_id.clone());
                return Ok(());
            }
        }
        Err(MultiplexerError::WindowNotAttachedToSession(
            window_id.clone(),
        ))
    }

    /// Find which Window a Pane lives in.
    pub fn window_id_of_pane(&self, pane_id: &PaneId) -> MultiplexerResult<WindowId> {
        for (wid, window) in self.windows.iter() {
            if window.panes.contains_key(pane_id) {
                return Ok(wid.clone());
            }
        }
        Err(MultiplexerError::WindowNotFoundForPane(pane_id.clone()))
    }

    /// Bootstrap: one session, one window, one pane, one activity.
    pub fn bootstrap_default(
        &mut self,
    ) -> MultiplexerResult<(SessionId, WindowId, PaneId, ActivityId)> {
        let session_id = self.new_session(None);
        let window_id = self.new_window_in(Some(&session_id), None)?;
        let window = self.windows.get(&window_id).expect("just created");
        let pane_id = window.active_pane.clone();
        let activity_id = window
            .panes
            .get(&pane_id)
            .expect("just created pane")
            .active_activity
            .clone();
        Ok((session_id, window_id, pane_id, activity_id))
    }

    // ── Transitional limbo API (PR2 - PR6) ────────────────────────────────
    // These keep the pre-PR5 SDK flow working: extensions still call
    // `createActivity → createPane → splitPane`. Removed in PR7.

    /// Transitional. Create an Activity that's not yet attached to any Pane.
    pub fn new_activity(&mut self, activity: Activity) -> ActivityId {
        let id = activity.id.clone();
        self.limbo_activities.insert(id.clone(), activity);
        id
    }

    /// Transitional. Create a Pane that's not yet placed in a layout. The
    /// Activity must have been created via `new_activity` first.
    pub fn new_pane_with_activity(&mut self, activity_id: ActivityId) -> MultiplexerResult<PaneId> {
        if !self.limbo_activities.contains_key(&activity_id) {
            return Err(MultiplexerError::ActivityNotFound(activity_id));
        }
        let pane_id = PaneId::new();
        self.limbo_panes.insert(pane_id.clone(), activity_id);
        Ok(pane_id)
    }

    /// Transitional. Place a limbo Pane next to a target via cell split.
    pub fn split_with_pane(
        &mut self,
        src: PaneId,
        new_pane: PaneId,
        side: Side,
        orientation: SplitOrientation,
    ) -> MultiplexerResult<()> {
        let already_placed = self
            .windows
            .iter()
            .any(|(_, w)| w.panes.contains_key(&new_pane));
        if already_placed {
            return Err(MultiplexerError::PaneAlreadyPlaced(new_pane));
        }
        let activity_id = self
            .limbo_panes
            .remove(&new_pane)
            .ok_or_else(|| MultiplexerError::PaneNotFound(new_pane.clone()))?;
        let activity = self
            .limbo_activities
            .remove(&activity_id)
            .ok_or(MultiplexerError::ActivityNotFound(activity_id))?;
        let window = self
            .windows
            .find_window_with_pane_mut(&src)
            .ok_or_else(|| MultiplexerError::PaneNotFound(src.clone()))?;
        window.split_pane(&src, new_pane, activity, side, orientation)
    }

    /// Transitional accessor used by `handlers/activities.rs::iframe_serve`.
    /// Walks all windows then limbo to find the Activity metadata.
    pub fn activity_metadata(&self, aid: &ActivityId) -> Option<&Activity> {
        for (_, w) in self.windows.iter() {
            for (_, p) in w.panes.iter() {
                if let Some(a) = p.activity(aid) {
                    return Some(a);
                }
            }
        }
        self.limbo_activities.get(aid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::cells::Cell;

    struct WindowFixture {
        ms: MultiplexerService,
        window_id: WindowId,
        pane_id: PaneId,
        pane_cell: CellId,
        root_cell: CellId,
    }

    fn fresh_window() -> WindowFixture {
        let mut ms = MultiplexerService::default();
        let window_id = ms.new_window_in(None, None).unwrap();
        let win = ms.windows().get(&window_id).unwrap();
        let pane_id = win.active_pane.clone();
        let pane_cell = win.pane_to_cell.get(&pane_id).unwrap().clone();
        let root_cell = win.root_cell.clone();
        WindowFixture {
            ms,
            window_id,
            pane_id,
            pane_cell,
            root_cell,
        }
    }

    fn pane_count(ms: &MultiplexerService) -> usize {
        ms.windows().iter().map(|(_, w)| w.panes.len()).sum()
    }

    fn activity_exists_in_window(ms: &MultiplexerService, aid: &ActivityId) -> bool {
        ms.windows()
            .iter()
            .any(|(_, w)| w.panes.iter().any(|(_, p)| p.has_activity(aid)))
    }

    #[test]
    fn close_pane_after_split_fully_reverts_state() {
        let WindowFixture {
            mut ms,
            window_id,
            pane_id: original_pane,
            pane_cell: original_cell,
            root_cell,
        } = fresh_window();
        let panes_before = pane_count(&ms);

        let (new_pane, new_activity) = ms
            .split_pane(
                original_pane.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        assert_eq!(ms.windows().get(&window_id).unwrap().active_pane, new_pane);

        ms.close_pane(&new_pane).unwrap();

        assert_eq!(pane_count(&ms), panes_before);
        let win = ms.windows().get(&window_id).unwrap();
        assert!(!win.pane_to_cell.contains_key(&new_pane));
        assert!(!activity_exists_in_window(&ms, &new_activity));

        assert_eq!(win.active_pane, original_pane);

        let Cell::Root(root) = win.cells.cell(&root_cell).unwrap() else {
            panic!("root cell missing");
        };
        assert_eq!(root.child, original_cell);
        let Cell::Pane(pane_cell) = win.cells.cell(&original_cell).unwrap() else {
            panic!("original pane cell missing");
        };
        assert_eq!(pane_cell.parent.as_ref(), Some(&root_cell));
        assert_eq!(pane_cell.pane, original_pane);
    }

    #[test]
    fn close_last_pane_returns_error_without_mutating_state() {
        let WindowFixture {
            mut ms,
            window_id,
            pane_id,
            pane_cell,
            root_cell,
        } = fresh_window();
        let panes_before = pane_count(&ms);

        let result = ms.close_pane(&pane_id);

        assert!(matches!(
            result,
            Err(MultiplexerError::CannotCloseLastPane(_))
        ));
        assert_eq!(pane_count(&ms), panes_before);
        let win = ms.windows().get(&window_id).unwrap();
        assert_eq!(win.pane_to_cell.get(&pane_id).unwrap(), &pane_cell);
        assert!(win.cells.cell(&pane_cell).is_ok());
        assert!(win.cells.cell(&root_cell).is_ok());
        assert_eq!(win.active_pane, pane_id);
    }

    #[test]
    fn close_pane_unknown_id_returns_pane_not_found() {
        let mut ms = MultiplexerService::default();
        let unknown = PaneId::new();
        assert!(matches!(
            ms.close_pane(&unknown),
            Err(MultiplexerError::PaneNotFound(_))
        ));
    }

    #[test]
    fn close_non_active_pane_leaves_active_pane_unchanged() {
        let WindowFixture {
            mut ms,
            window_id,
            pane_id: original_pane,
            ..
        } = fresh_window();

        let (new_pane, _) = ms
            .split_pane(
                original_pane.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let _ = ms.set_active_pane(&window_id, &original_pane).unwrap();

        ms.close_pane(&new_pane).unwrap();

        assert_eq!(
            ms.windows().get(&window_id).unwrap().active_pane,
            original_pane
        );
    }

    #[test]
    fn new_session_returns_id_without_window() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let session = ms.sessions().get(&sid).unwrap();
        assert!(session.linked_windows.is_empty());
        assert!(session.active_window.is_none());
    }

    #[test]
    fn rename_session_updates_name() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(Some("orig".into()));
        ms.rename_session(&sid, "renamed".into()).unwrap();
        assert_eq!(ms.sessions().get(&sid).unwrap().name, "renamed");
    }

    #[test]
    fn rename_session_unknown_returns_session_not_found() {
        let mut ms = MultiplexerService::default();
        let sid = SessionId::new();
        assert!(matches!(
            ms.rename_session(&sid, "x".into()),
            Err(MultiplexerError::SessionNotFound(_))
        ));
    }

    #[test]
    fn new_window_in_attaches_to_session_and_promotes_active() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let wid = ms.new_window_in(Some(&sid), None).unwrap();
        let session = ms.sessions().get(&sid).unwrap();
        assert_eq!(session.linked_windows, vec![wid.clone()]);
        assert_eq!(session.active_window.as_ref(), Some(&wid));
    }

    #[test]
    fn new_window_in_with_no_session_creates_orphan() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, None).unwrap();
        assert!(ms.windows().get(&wid).is_some());
        for (_, s) in ms.sessions().iter() {
            assert!(!s.linked_windows.contains(&wid));
        }
    }

    #[test]
    fn new_window_in_unknown_session_returns_session_not_found() {
        let mut ms = MultiplexerService::default();
        let bogus = SessionId::new();
        assert!(matches!(
            ms.new_window_in(Some(&bogus), None),
            Err(MultiplexerError::SessionNotFound(_))
        ));
    }

    #[test]
    fn rename_window_updates_name() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, Some("orig".into())).unwrap();
        ms.rename_window(&wid, "renamed".into()).unwrap();
        assert_eq!(ms.windows().get(&wid).unwrap().name, "renamed");
    }

    #[test]
    fn rename_window_unknown_returns_window_not_found() {
        let mut ms = MultiplexerService::default();
        let bogus = WindowId::new();
        assert!(matches!(
            ms.rename_window(&bogus, "x".into()),
            Err(MultiplexerError::WindowNotFound(_))
        ));
    }

    #[test]
    fn close_window_drops_panes_cells_activities_and_detaches_session() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let wid = ms.new_window_in(Some(&sid), None).unwrap();
        let pane_id = ms.windows().get(&wid).unwrap().active_pane.clone();
        let pane_count_before = pane_count(&ms);

        let activities = ms.close_window(&wid).unwrap();
        assert_eq!(activities.len(), 1);
        let activity_id = &activities[0];

        assert!(ms.windows().get(&wid).is_none());
        assert_eq!(pane_count(&ms), pane_count_before - 1);
        assert!(!activity_exists_in_window(&ms, activity_id));
        // Session is detached.
        assert!(ms.sessions().get(&sid).unwrap().linked_windows.is_empty());
        assert!(ms.sessions().get(&sid).unwrap().active_window.is_none());
        let _ = pane_id;
    }

    #[test]
    fn close_window_unknown_returns_window_not_found() {
        let mut ms = MultiplexerService::default();
        let bogus = WindowId::new();
        assert!(matches!(
            ms.close_window(&bogus),
            Err(MultiplexerError::WindowNotFound(_))
        ));
    }

    #[test]
    fn delete_session_cascades_window_close_and_returns_activities() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let wid_a = ms.new_window_in(Some(&sid), None).unwrap();
        let wid_b = ms.new_window_in(Some(&sid), None).unwrap();

        let activities = ms.delete_session(&sid).unwrap();
        assert_eq!(activities.len(), 2);
        assert!(ms.sessions().get(&sid).is_none());
        assert!(ms.windows().get(&wid_a).is_none());
        assert!(ms.windows().get(&wid_b).is_none());
    }

    #[test]
    fn select_active_window_for_orphan_returns_not_attached() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, None).unwrap();
        assert!(matches!(
            ms.select_active_window(&wid),
            Err(MultiplexerError::WindowNotAttachedToSession(_))
        ));
    }

    #[test]
    fn select_active_window_for_unknown_id_returns_window_not_found() {
        let mut ms = MultiplexerService::default();
        let bogus = WindowId::new();
        assert!(matches!(
            ms.select_active_window(&bogus),
            Err(MultiplexerError::WindowNotFound(_))
        ));
    }

    #[test]
    fn select_active_window_updates_session_active_window() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let wid_a = ms.new_window_in(Some(&sid), None).unwrap();
        let wid_b = ms.new_window_in(Some(&sid), None).unwrap();
        assert_eq!(
            ms.sessions().get(&sid).unwrap().active_window.as_ref(),
            Some(&wid_a)
        );
        ms.select_active_window(&wid_b).unwrap();
        assert_eq!(
            ms.sessions().get(&sid).unwrap().active_window.as_ref(),
            Some(&wid_b)
        );
    }

    #[test]
    fn new_pane_with_activity_creates_limbo_pane() {
        let mut ms = MultiplexerService::default();
        let aid = ActivityId::new();
        let activity_id = ms.new_activity(Activity::terminal(aid.clone()));
        let pane_id = ms.new_pane_with_activity(activity_id.clone()).unwrap();
        // limbo pane is NOT in any window's panes yet.
        assert!(!activity_exists_in_window(&ms, &aid));
        // But the limbo store knows about it.
        assert!(ms.activity_metadata(&aid).is_some());
        let _ = pane_id;
    }

    #[test]
    fn new_pane_with_activity_rejects_unknown_activity() {
        let mut ms = MultiplexerService::default();
        let phantom = ActivityId::new();
        let err = ms.new_pane_with_activity(phantom.clone()).unwrap_err();
        assert!(matches!(err, MultiplexerError::ActivityNotFound(id) if id == phantom));
    }

    #[test]
    fn bootstrap_default_yields_four_consistent_ids() {
        let mut ms = MultiplexerService::default();
        let (sid, wid, pid, aid) = ms.bootstrap_default().unwrap();
        let session = ms.sessions().get(&sid).unwrap();
        assert_eq!(session.linked_windows, vec![wid.clone()]);
        assert_eq!(session.active_window.as_ref(), Some(&wid));
        let window = ms.windows().get(&wid).unwrap();
        assert_eq!(window.active_pane, pid);
        assert!(activity_exists_in_window(&ms, &aid));
    }

    #[test]
    fn split_with_pane_places_limbo_pane_after_target() {
        let mut ms = MultiplexerService::default();
        let (_sid, wid, target_pane, _aid) = ms.bootstrap_default().unwrap();
        let activity_id = ms.new_activity(Activity::terminal(ActivityId::new()));
        let limbo = ms.new_pane_with_activity(activity_id).unwrap();
        ms.split_with_pane(
            target_pane.clone(),
            limbo.clone(),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        assert!(
            ms.windows()
                .get(&wid)
                .unwrap()
                .pane_to_cell
                .contains_key(&limbo)
        );
    }

    #[test]
    fn split_with_pane_rejects_already_placed_pane() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, target_pane, _aid) = ms.bootstrap_default().unwrap();
        let err = ms
            .split_with_pane(
                target_pane.clone(),
                target_pane.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::PaneAlreadyPlaced(id) if id == target_pane));
    }

    #[test]
    fn split_with_pane_rejects_unknown_new_pane() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, target_pane, _aid) = ms.bootstrap_default().unwrap();
        let phantom = PaneId::new();
        let err = ms
            .split_with_pane(
                target_pane,
                phantom.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap_err();
        assert!(matches!(err, MultiplexerError::PaneNotFound(id) if id == phantom));
    }

    #[test]
    fn window_id_of_pane_disambiguates_between_windows() {
        let mut ms = MultiplexerService::default();
        let (sid, wid_a, pane_a, _aid_a) = ms.bootstrap_default().unwrap();
        let (pane_a2, _) = ms
            .split_pane(pane_a.clone(), Side::After, SplitOrientation::Horizontal)
            .unwrap();
        let wid_b = ms.new_window_in(Some(&sid), Some("second".into())).unwrap();
        let pane_b = ms.windows().get(&wid_b).unwrap().active_pane.clone();

        assert_eq!(ms.window_id_of_pane(&pane_a).unwrap(), wid_a);
        assert_eq!(ms.window_id_of_pane(&pane_a2).unwrap(), wid_a);
        assert_eq!(ms.window_id_of_pane(&pane_b).unwrap(), wid_b);
    }

    #[test]
    fn window_id_of_pane_unknown_returns_window_not_found_for_pane() {
        let ms = MultiplexerService::default();
        let pid = PaneId::new();
        assert!(matches!(
            ms.window_id_of_pane(&pid),
            Err(MultiplexerError::WindowNotFoundForPane(_))
        ));
    }

    #[test]
    fn close_pane_handles_limbo_pane_without_touching_cell_tree() {
        let mut ms = MultiplexerService::default();
        let (_sid, wid, _pid, _aid) = ms.bootstrap_default().unwrap();
        let cells_before = ms.windows().get(&wid).unwrap().cells.clone();
        let activity_id = ms.new_activity(Activity::terminal(ActivityId::new()));
        let limbo = ms.new_pane_with_activity(activity_id).unwrap();
        ms.close_pane(&limbo).unwrap();
        let cells_after = ms.windows().get(&wid).unwrap().cells.clone();
        let serialized_before = serde_json::to_string(&cells_before).unwrap();
        let serialized_after = serde_json::to_string(&cells_after).unwrap();
        assert_eq!(serialized_before, serialized_after);
    }

    #[test]
    fn set_active_pane_changes_active_for_non_active_pane() {
        let WindowFixture {
            mut ms,
            window_id,
            pane_id: original_pane,
            ..
        } = fresh_window();
        let (new_pane, _) = ms
            .split_pane(
                original_pane.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let outcome = ms.set_active_pane(&window_id, &original_pane).unwrap();

        assert!(matches!(outcome, SetActiveOutcome::Changed));
        assert_eq!(
            ms.windows().get(&window_id).unwrap().active_pane,
            original_pane
        );
        let _ = new_pane;
    }

    #[test]
    fn set_active_pane_returns_unchanged_when_already_active() {
        let WindowFixture {
            mut ms,
            window_id,
            pane_id,
            ..
        } = fresh_window();
        let outcome = ms.set_active_pane(&window_id, &pane_id).unwrap();
        assert!(matches!(outcome, SetActiveOutcome::Unchanged));
        assert_eq!(ms.windows().get(&window_id).unwrap().active_pane, pane_id);
    }

    #[test]
    fn set_active_pane_unknown_window_returns_window_not_found() {
        let WindowFixture {
            mut ms, pane_id, ..
        } = fresh_window();
        let bogus = WindowId::new();
        let err = ms.set_active_pane(&bogus, &pane_id).unwrap_err();
        assert!(matches!(err, MultiplexerError::WindowNotFound(_)));
    }

    #[test]
    fn set_active_pane_unknown_pane_returns_pane_not_found() {
        let WindowFixture {
            mut ms, window_id, ..
        } = fresh_window();
        let bogus = PaneId::new();
        let err = ms.set_active_pane(&window_id, &bogus).unwrap_err();
        assert!(matches!(err, MultiplexerError::PaneNotFound(_)));
    }

    #[test]
    fn set_active_pane_pane_in_other_window_returns_pane_not_in_window() {
        let WindowFixture {
            mut ms,
            window_id: w_a,
            pane_id: pane_a,
            ..
        } = fresh_window();
        let w_b = ms.new_window_in(None, None).unwrap();
        let err = ms.set_active_pane(&w_b, &pane_a).unwrap_err();
        assert!(matches!(
            err,
            MultiplexerError::PaneNotInWindow { ref window, ref pane }
                if window == &w_b && pane == &pane_a
        ));
        assert_eq!(ms.windows().get(&w_a).unwrap().active_pane, pane_a);
    }
}
