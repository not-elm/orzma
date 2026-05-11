use std::collections::HashMap;

pub mod activity;
pub mod cells;
pub mod error;
pub mod pane;
pub mod session;
pub mod window;

pub use error::{SessionError, SessionResult};
pub use window::*;

use crate::{
    activity::{Activity, ActivityId, ActivityState},
    cells::{CellId, LayoutCellState, Side, SplitOrientation},
    pane::{Pane, PaneId, PaneState},
    session::{Session, SessionId, SessionState},
};

#[derive(Default)]
pub struct MultiplexerService {
    sessions: SessionState,
    windows: WindowState,
    panes: PaneState,
    cells: LayoutCellState,
    // どのセルが指定のセルを描画しているかを参照するためのマップ
    pane_to_cell: HashMap<PaneId, CellId>,
    activities: ActivityState,
}

impl MultiplexerService {
    // ── Narrow read-only accessors ────────────────────────────────────────────

    pub fn sessions(&self) -> &SessionState {
        &self.sessions
    }
    pub fn windows(&self) -> &WindowState {
        &self.windows
    }
    pub fn panes(&self) -> &PaneState {
        &self.panes
    }
    pub fn activities(&self) -> &ActivityState {
        &self.activities
    }

    /// Internal index exposed for assertions in tests and bookkeeping in callers.
    pub fn pane_to_cell_index(&self) -> &HashMap<PaneId, CellId> {
        &self.pane_to_cell
    }

    /// Read-only view of the cell tree, e.g. for serialization or test assertions.
    pub fn cells_ref(&self) -> &LayoutCellState {
        &self.cells
    }

    pub fn new_session(&mut self, name: Option<String>) -> SessionId {
        let session_id = SessionId::new();
        let session_name = name.unwrap_or_else(|| format!("Session{}", self.sessions.len() + 1));
        self.sessions
            .register(session_id.clone(), Session::empty(session_name));
        session_id
    }

    pub fn rename_session(&mut self, session_id: &SessionId, name: String) -> SessionResult {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionError::SessionNotFound(session_id.clone()))?;
        session.rename(name);
        Ok(())
    }

    /// Remove the session and cascade-close every window it owns.
    /// Returns every `ActivityId` that was destroyed so the caller can tear
    /// down the corresponding PTYs.
    pub fn delete_session(&mut self, session_id: &SessionId) -> SessionResult<Vec<ActivityId>> {
        let session = self.sessions.remove(session_id)?;
        let mut activities = Vec::new();
        for wid in session.windows {
            activities.extend(self.close_window(&wid)?);
        }
        Ok(activities)
    }

    fn new_window_internal(&mut self, name: Option<String>) -> WindowId {
        let id = WindowId::new();
        let activity_id = self.new_activity(Activity::default());
        let pane_id = PaneId::new();
        self.panes.insert(pane_id.clone(), Pane::new(activity_id));
        let (root_cell, pane_cell_id) = self.cells.new_window_layout(pane_id.clone());
        self.pane_to_cell.insert(pane_id.clone(), pane_cell_id);
        let window_name = name.unwrap_or_else(|| format!("Window{}", self.windows.len() + 1));
        self.windows
            .insert(id.clone(), Window::new(window_name, root_cell, pane_id));
        id
    }

    /// Create a new window with one initial pane + activity. If `session_id`
    /// is `Some`, the window is appended to that session and (if no other
    /// window was active) becomes the session's `active_window`. If `None`,
    /// the window is left unattached (orphan).
    pub fn new_window_in(
        &mut self,
        session_id: Option<&SessionId>,
        name: Option<String>,
    ) -> SessionResult<WindowId> {
        if let Some(sid) = session_id
            && self.sessions.get(sid).is_none()
        {
            return Err(SessionError::SessionNotFound(sid.clone()));
        }
        let window_id = self.new_window_internal(name);
        if let Some(sid) = session_id {
            let session = self
                .sessions
                .get_mut(sid)
                .expect("validated existence above");
            session.attach_window(window_id.clone());
        }
        Ok(window_id)
    }

    pub fn rename_window(&mut self, window_id: &WindowId, name: String) -> SessionResult {
        let window = self
            .windows
            .get_mut(window_id)
            .ok_or_else(|| SessionError::WindowNotFound(window_id.clone()))?;
        window.rename(name);
        Ok(())
    }

    /// Close a window: remove every pane it contains, drop the cell tree
    /// rooted at `Window.root_cell`, detach from any owning session, and
    /// return the `ActivityId`s destroyed so the caller can kill PTYs.
    pub fn close_window(&mut self, window_id: &WindowId) -> SessionResult<Vec<ActivityId>> {
        let root_cell = self
            .windows
            .get(window_id)
            .ok_or_else(|| SessionError::WindowNotFound(window_id.clone()))?
            .root_cell
            .clone();

        let pane_ids = self.cells.pane_ids_in_subtree(&root_cell)?;

        let mut activity_ids = Vec::new();
        for pid in &pane_ids {
            let pane = self
                .panes
                .remove(pid)
                .expect("pane referenced by window must exist");
            self.pane_to_cell.remove(pid);
            for aid in pane.activities {
                self.activities.remove(&aid);
                activity_ids.push(aid);
            }
        }

        self.cells.remove_subtree(&root_cell)?;
        self.windows.remove(window_id);

        for (_, session) in self.sessions.iter_mut() {
            session.detach_window(window_id);
        }

        Ok(activity_ids)
    }

    pub fn new_pane_with_activity(&mut self, activity_id: ActivityId) -> SessionResult<PaneId> {
        if !self.activities.contains(&activity_id) {
            return Err(SessionError::ActivityNotFound(activity_id));
        }
        let id = PaneId::new();
        self.panes.insert(id.clone(), Pane::new(activity_id));
        Ok(id)
    }

    pub fn new_pane(
        &mut self,
        activity_id: ActivityId,
        parent_cell: Option<CellId>,
    ) -> (PaneId, CellId) {
        let id = PaneId::new();
        self.panes.insert(id.clone(), Pane::new(activity_id));
        let cell_id = self.cells.new_pane(id.clone(), parent_cell);
        self.pane_to_cell.insert(id.clone(), cell_id.clone());
        (id, cell_id)
    }

    pub fn new_activity(&mut self, activity: Activity) -> ActivityId {
        let id = ActivityId::new();
        self.activities.insert(id.clone(), activity);
        id
    }

    pub fn split_pane(
        &mut self,
        target_pane_id: PaneId,
        side: Side,
        orientation: SplitOrientation,
    ) -> SessionResult<(PaneId, ActivityId)> {
        let target_cell_id = self.cell_id_for_pane(&target_pane_id)?.clone();
        let new_activity_id = self.new_activity(Activity::default());
        let (new_pane_id, new_cell_id) = self.new_pane(new_activity_id.clone(), None);
        self.cells
            .split_cell(target_cell_id, new_cell_id, side, orientation)?;
        self.windows
            .replace_active_pane(&target_pane_id, &new_pane_id);
        Ok((new_pane_id, new_activity_id))
    }

    pub fn split_with_pane(
        &mut self,
        src: PaneId,
        new_pane: PaneId,
        side: crate::cells::Side,
        orientation: crate::cells::SplitOrientation,
    ) -> SessionResult<()> {
        if !self.panes.contains_key(&new_pane) {
            return Err(SessionError::PaneNotFound(new_pane));
        }
        if self.pane_to_cell.contains_key(&new_pane) {
            return Err(SessionError::PaneAlreadyPlaced(new_pane));
        }
        let target_cell_id = self.cell_id_for_pane(&src)?.clone();
        let new_cell_id = self.cells.new_pane(new_pane.clone(), None);
        if let Err(e) =
            self.cells
                .split_cell(target_cell_id, new_cell_id.clone(), side, orientation)
        {
            // rollback: orphan の new_cell を消す
            let _ = self.cells.remove_subtree(&new_cell_id);
            return Err(e);
        }
        self.pane_to_cell.insert(new_pane.clone(), new_cell_id);
        self.windows.replace_active_pane(&src, &new_pane);
        Ok(())
    }

    pub fn close_pane(&mut self, pane_id: &PaneId) -> SessionResult {
        if !self.panes.contains_key(pane_id) {
            return Err(SessionError::PaneNotFound(pane_id.clone()));
        }
        if let Some(cell_id) = self.pane_to_cell.get(pane_id).cloned() {
            let outcome = self.cells.close_cell(&cell_id)?;
            let survivor_pane_id = self.cells.leftmost_pane(outcome.survivor())?.clone();
            self.windows.replace_active_pane(pane_id, &survivor_pane_id);
        }
        self.forget_pane(pane_id);
        Ok(())
    }

    /// Drop the pane's index entries and its owned activities. Caller is
    /// responsible for already having collapsed the cell tree and rerouted
    /// `active_pane`; this is the final commit step of `close_pane`.
    fn forget_pane(&mut self, pane_id: &PaneId) {
        let pane = self
            .panes
            .remove(pane_id)
            .expect("close_pane validated pane existed before forget_pane");
        self.pane_to_cell.remove(pane_id);
        for activity_id in pane.activities {
            self.activities.remove(&activity_id);
        }
    }

    pub fn cell_id_for_pane(&self, pane_id: &PaneId) -> SessionResult<&CellId> {
        self.pane_to_cell
            .get(pane_id)
            .ok_or_else(|| SessionError::CellForPaneNotFound(pane_id.clone()))
    }

    /// Return the `WindowId` whose layout subtree currently contains `pane_id`.
    /// Walks pane → cell → ... → root, then matches the root cell against `WindowState`.
    pub fn window_id_of_pane(&self, pane_id: &PaneId) -> SessionResult<WindowId> {
        let start_cell = self.cell_id_for_pane(pane_id)?.clone();
        let mut current = start_cell;
        loop {
            let cell = self.cells.cell(&current)?;
            match cell.parent() {
                Some(parent) => current = parent.clone(),
                None => break, // current is the Root
            }
        }
        for (wid, window) in self.windows.iter() {
            if window.root_cell == current {
                return Ok(wid.clone());
            }
        }
        Err(SessionError::WindowNotFoundForPane(pane_id.clone()))
    }

    /// Find the session that owns `window_id` and set its `active_window`.
    /// Returns `WindowNotFound` for a window that doesn't exist at all, and
    /// `WindowNotAttachedToSession` for orphan windows.
    pub fn select_active_window(&mut self, window_id: &WindowId) -> SessionResult {
        if !self.windows.contains_key(window_id) {
            return Err(SessionError::WindowNotFound(window_id.clone()));
        }
        for (_, session) in self.sessions.iter_mut() {
            if session.windows.contains(window_id) {
                session.active_window = Some(window_id.clone());
                return Ok(());
            }
        }
        Err(SessionError::WindowNotAttachedToSession(window_id.clone()))
    }

    /// Bootstrap the multiplexer with one session, one window inside it, and
    /// thus one initial pane + activity. Returns the four IDs.
    pub fn bootstrap_default(
        &mut self,
    ) -> SessionResult<(SessionId, WindowId, PaneId, ActivityId)> {
        let session_id = self.new_session(None);
        let window_id = self.new_window_in(Some(&session_id), None)?;
        let window = self.windows.get(&window_id).expect("just created");
        let pane_id = window.active_pane.clone();
        let activity_id = self
            .panes
            .get(&pane_id)
            .expect("just created pane has activities")
            .activities[0]
            .clone();
        Ok((session_id, window_id, pane_id, activity_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::Cell;

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
        let window = ms.windows().get(&window_id).expect("window exists").clone();
        let pane_id = window.active_pane.clone();
        let pane_cell = ms.cell_id_for_pane(&pane_id).unwrap().clone();
        WindowFixture {
            ms,
            window_id,
            pane_id,
            pane_cell,
            root_cell: window.root_cell,
        }
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
        let panes_before = ms.panes().len();

        let (new_pane, new_activity) = ms
            .split_pane(
                original_pane.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        assert_eq!(ms.windows().get(&window_id).unwrap().active_pane, new_pane);

        ms.close_pane(&new_pane).unwrap();

        // pane/index/activity for the closed pane are gone.
        assert_eq!(ms.panes().len(), panes_before);
        assert!(!ms.pane_to_cell_index().contains_key(&new_pane));
        assert!(!ms.activities().contains(&new_activity));

        // active_pane is rerouted back to the surviving original.
        assert_eq!(
            ms.windows().get(&window_id).unwrap().active_pane,
            original_pane
        );

        // The cell tree is collapsed: root.child points at the original pane cell.
        let Cell::Root(root) = ms.cells_ref().cell(&root_cell).unwrap() else {
            panic!("root cell missing");
        };
        assert_eq!(root.child, original_cell);
        let Cell::Pane(pane_cell) = ms.cells_ref().cell(&original_cell).unwrap() else {
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
        let panes_before = ms.panes().len();

        let result = ms.close_pane(&pane_id);

        assert!(matches!(result, Err(SessionError::CannotCloseLastPane(_))));
        // No store was mutated.
        assert_eq!(ms.panes().len(), panes_before);
        assert_eq!(ms.cell_id_for_pane(&pane_id).unwrap(), &pane_cell);
        assert!(ms.cells_ref().cell(&pane_cell).is_ok());
        assert!(ms.cells_ref().cell(&root_cell).is_ok());
        assert_eq!(ms.windows().get(&window_id).unwrap().active_pane, pane_id);
    }

    #[test]
    fn close_pane_unknown_id_returns_pane_not_found() {
        let mut ms = MultiplexerService::default();
        let unknown = PaneId::new();
        assert!(matches!(
            ms.close_pane(&unknown),
            Err(SessionError::PaneNotFound(_))
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
        // Make the original pane active again before closing the new one.
        ms.windows.replace_active_pane(&new_pane, &original_pane);

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
        assert!(session.windows.is_empty());
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
            Err(SessionError::SessionNotFound(_))
        ));
    }

    #[test]
    fn new_window_in_attaches_to_session_and_promotes_active() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let wid = ms.new_window_in(Some(&sid), None).unwrap();
        let session = ms.sessions().get(&sid).unwrap();
        assert_eq!(session.windows, vec![wid.clone()]);
        assert_eq!(session.active_window.as_ref(), Some(&wid));
    }

    #[test]
    fn new_window_in_with_no_session_creates_orphan() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, None).unwrap();
        assert!(ms.windows().get(&wid).is_some());
        // No session should reference it.
        for (_, s) in ms.sessions().iter() {
            assert!(!s.windows.contains(&wid));
        }
    }

    #[test]
    fn new_window_in_unknown_session_returns_session_not_found() {
        let mut ms = MultiplexerService::default();
        let bogus = SessionId::new();
        assert!(matches!(
            ms.new_window_in(Some(&bogus), None),
            Err(SessionError::SessionNotFound(_))
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
            Err(SessionError::WindowNotFound(_))
        ));
    }

    #[test]
    fn close_window_drops_panes_cells_activities_and_detaches_session() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let wid = ms.new_window_in(Some(&sid), None).unwrap();
        let root_cell = ms.windows().get(&wid).unwrap().root_cell.clone();
        let pane_id = ms.windows().get(&wid).unwrap().active_pane.clone();
        let pane_count_before = ms.panes().len();

        let activities = ms.close_window(&wid).unwrap();
        assert_eq!(activities.len(), 1);
        let activity_id = &activities[0];

        // Window gone from WindowState
        assert!(ms.windows().get(&wid).is_none());
        // Pane gone from PaneState
        assert_eq!(ms.panes().len(), pane_count_before - 1);
        // pane_to_cell index cleared for that pane
        assert!(!ms.pane_to_cell_index().contains_key(&pane_id));
        // Activity gone from ActivityState
        assert!(!ms.activities().contains(activity_id));
        // Cell tree (root + pane cell) is dropped
        assert!(ms.cells_ref().cell(&root_cell).is_err());
        // Session detached
        assert!(ms.sessions().get(&sid).unwrap().windows.is_empty());
        assert!(ms.sessions().get(&sid).unwrap().active_window.is_none());
    }

    #[test]
    fn close_window_unknown_returns_window_not_found() {
        let mut ms = MultiplexerService::default();
        let bogus = WindowId::new();
        assert!(matches!(
            ms.close_window(&bogus),
            Err(SessionError::WindowNotFound(_))
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
            Err(SessionError::WindowNotAttachedToSession(_))
        ));
    }

    #[test]
    fn select_active_window_for_unknown_id_returns_window_not_found() {
        let mut ms = MultiplexerService::default();
        let bogus = WindowId::new();
        assert!(matches!(
            ms.select_active_window(&bogus),
            Err(SessionError::WindowNotFound(_))
        ));
    }

    #[test]
    fn select_active_window_updates_session_active_window() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let wid_a = ms.new_window_in(Some(&sid), None).unwrap();
        let wid_b = ms.new_window_in(Some(&sid), None).unwrap();
        // active is whatever attached first (wid_a).
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
        let activity_id = ms.new_activity(Activity::default());
        let pane_id = ms.new_pane_with_activity(activity_id.clone()).unwrap();
        assert!(ms.panes().contains_key(&pane_id));
        assert!(ms.cell_id_for_pane(&pane_id).is_err());
    }

    #[test]
    fn new_pane_with_activity_rejects_unknown_activity() {
        let mut ms = MultiplexerService::default();
        let phantom = ActivityId::new();
        let err = ms.new_pane_with_activity(phantom.clone()).unwrap_err();
        assert!(matches!(err, SessionError::ActivityNotFound(id) if id == phantom));
    }

    #[test]
    fn bootstrap_default_yields_four_consistent_ids() {
        let mut ms = MultiplexerService::default();
        let (sid, wid, pid, aid) = ms.bootstrap_default().unwrap();
        let session = ms.sessions().get(&sid).unwrap();
        assert_eq!(session.windows, vec![wid.clone()]);
        assert_eq!(session.active_window.as_ref(), Some(&wid));
        let window = ms.windows().get(&wid).unwrap();
        assert_eq!(window.active_pane, pid);
        let pane_cell = ms.cell_id_for_pane(&pid).unwrap();
        let _ = pane_cell;
        // The activity must be in ActivityState (we exposed `contains` earlier).
        assert!(ms.activities().contains(&aid));
    }

    #[test]
    fn split_with_pane_places_limbo_pane_after_target() {
        use crate::cells::{Side, SplitOrientation};
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, target_pane, _aid) = ms.bootstrap_default().unwrap();
        let activity_id = ms.new_activity(Activity::default());
        let limbo = ms.new_pane_with_activity(activity_id).unwrap();
        ms.split_with_pane(
            target_pane.clone(),
            limbo.clone(),
            Side::After,
            SplitOrientation::Horizontal,
        )
        .unwrap();
        assert!(ms.cell_id_for_pane(&limbo).is_ok());
    }

    #[test]
    fn split_with_pane_rejects_already_placed_pane() {
        use crate::cells::{Side, SplitOrientation};
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
        assert!(matches!(err, SessionError::PaneAlreadyPlaced(id) if id == target_pane));
    }

    #[test]
    fn split_with_pane_rejects_unknown_new_pane() {
        use crate::cells::{Side, SplitOrientation};
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
        assert!(matches!(err, SessionError::PaneNotFound(id) if id == phantom));
    }

    #[test]
    fn window_id_of_pane_disambiguates_between_windows() {
        let mut ms = MultiplexerService::default();
        let (sid, wid_a, pane_a, _aid_a) = ms.bootstrap_default().unwrap();
        let (pane_a2, _) = ms
            .split_pane(
                pane_a.clone(),
                crate::cells::Side::After,
                crate::cells::SplitOrientation::Horizontal,
            )
            .unwrap();
        // Create a second window with its own pane in the same session.
        let wid_b = ms.new_window_in(Some(&sid), Some("second".into())).unwrap();
        let pane_b = {
            let win = ms.windows().get(&wid_b).unwrap();
            win.active_pane.clone()
        };

        assert_eq!(ms.window_id_of_pane(&pane_a).unwrap(), wid_a);
        assert_eq!(ms.window_id_of_pane(&pane_a2).unwrap(), wid_a);
        assert_eq!(ms.window_id_of_pane(&pane_b).unwrap(), wid_b);
    }

    #[test]
    fn window_id_of_pane_unknown_returns_cell_for_pane_not_found() {
        let ms = MultiplexerService::default();
        let pid = crate::pane::PaneId::new();
        assert!(matches!(
            ms.window_id_of_pane(&pid),
            Err(SessionError::CellForPaneNotFound(_))
        ));
    }

    #[test]
    fn window_id_of_pane_for_limbo_pane_returns_cell_for_pane_not_found() {
        let mut ms = MultiplexerService::default();
        let aid = ms.new_activity(crate::activity::Activity::default());
        let limbo_pane = ms.new_pane_with_activity(aid).unwrap();
        // limbo_pane is in `panes` but has no cell mapping yet.
        assert!(matches!(
            ms.window_id_of_pane(&limbo_pane),
            Err(SessionError::CellForPaneNotFound(_))
        ));
    }

    #[test]
    fn close_pane_handles_limbo_pane_without_touching_cell_tree() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, _pid, _aid) = ms.bootstrap_default().unwrap();
        let cells_before = ms.cells_ref().clone();
        let activity_id = ms.new_activity(Activity::default());
        let limbo = ms.new_pane_with_activity(activity_id).unwrap();
        ms.close_pane(&limbo).unwrap();
        assert!(!ms.panes().contains_key(&limbo));
        let serialized_before = serde_json::to_string(&cells_before).unwrap();
        let serialized_after = serde_json::to_string(ms.cells_ref()).unwrap();
        assert_eq!(serialized_before, serialized_after);
    }
}
