//! Domain service that orchestrates Session + Window mutations under the
//! canonical lock order: SessionState -> WindowStore.
//!
//! All multi-store mutations go through this service. Handlers must not lock
//! `SessionState` and `WindowStore` directly.

use crate::{
    SessionId, SessionState,
    activity::ActivityId,
    cell::{Side, SplitOrientation},
    error::{SessionError, SessionResult},
    pane::PaneId,
    window::{Window, WindowId, WindowStore},
};

#[derive(Clone)]
pub struct WindowService {
    sessions: SessionState,
    windows: WindowStore,
}

impl WindowService {
    pub fn new(sessions: SessionState, windows: WindowStore) -> Self {
        Self { sessions, windows }
    }
}

impl WindowService {
    /// Create a new window in the given session.
    /// Lock order: SessionState -> WindowStore.
    pub async fn create(
        &self,
        session_id: SessionId,
        name: Option<String>,
    ) -> SessionResult<Window> {
        let mut sessions = self.sessions.lock().await;
        let session = SessionState::require_mut(&mut sessions, &session_id)?;

        let window_id = WindowId::new();
        let assigned_name = name.unwrap_or_else(|| format!("window-{}", session.windows.len() + 1));
        let window = Window::new(window_id.clone(), session_id.clone(), assigned_name);

        session.windows.push(window_id.clone());

        let mut win_store = self.windows.lock().await;
        win_store.insert(window_id.clone(), window);
        let cloned = win_store
            .get(&window_id)
            .expect("just inserted")
            .clone_for_response();
        Ok(cloned)
    }

    pub async fn rename(
        &self,
        session_id: SessionId,
        window_id: WindowId,
        name: String,
    ) -> SessionResult<Window> {
        // Validate session ownership without keeping the SessionState lock.
        let sessions = self.sessions.lock().await;
        let session = SessionState::require(&sessions, &session_id)?;
        if !session.windows.contains(&window_id) {
            return Err(SessionError::WindowNotFound(window_id));
        }
        drop(sessions);

        let mut win_store = self.windows.lock().await;
        let window = win_store
            .get_mut(&window_id)
            .ok_or_else(|| SessionError::WindowNotFound(window_id.clone()))?;
        if window.session_id() != &session_id {
            return Err(SessionError::WindowDoesNotBelongToSession {
                session_id,
                window_id,
            });
        }
        window.rename(name);
        Ok(window.clone_for_response())
    }

    /// Close a window. Returns the activity_ids whose PTYs the caller must kill.
    /// Errors with CannotCloseLastWindow if this is the session's only window.
    pub async fn close(
        &self,
        session_id: SessionId,
        window_id: WindowId,
    ) -> SessionResult<Vec<ActivityId>> {
        let mut sessions = self.sessions.lock().await;
        let session = SessionState::require_mut(&mut sessions, &session_id)?;
        if !session.windows.contains(&window_id) {
            return Err(SessionError::WindowNotFound(window_id));
        }
        if session.windows.len() == 1 {
            return Err(SessionError::CannotCloseLastWindow(session_id.clone()));
        }
        // Determine new active_window if we're removing the active one.
        let position = session
            .windows
            .iter()
            .position(|w| w == &window_id)
            .expect("contains() returned true");
        let was_active = session.active_window == window_id;
        session.windows.retain(|w| w != &window_id);
        if was_active {
            // Choose the prior index (or first if we removed the head).
            let new_active_idx = if position == 0 { 0 } else { position - 1 };
            session.active_window = session.windows[new_active_idx].clone();
        }
        drop(sessions);

        let mut win_store = self.windows.lock().await;
        let removed = win_store
            .remove(&window_id)
            .ok_or_else(|| SessionError::WindowNotFound(window_id.clone()))?;
        if removed.session_id() != &session_id {
            // Restore the window to the store and signal the integrity violation.
            // This branch indicates a bug — the Session.windows list pointed at a
            // window that doesn't actually belong to that session.
            let inconsistent_id = removed.id().clone();
            win_store.insert(window_id.clone(), removed);
            return Err(SessionError::WindowDoesNotBelongToSession {
                session_id,
                window_id: inconsistent_id,
            });
        }
        let activity_ids: Vec<ActivityId> = removed
            .panes()
            .iter()
            .flat_map(|(_, pane)| pane.activities().iter().map(|a| a.id().clone()))
            .collect();
        Ok(activity_ids)
    }

    pub async fn select_active(
        &self,
        session_id: SessionId,
        window_id: WindowId,
    ) -> SessionResult<()> {
        let mut sessions = self.sessions.lock().await;
        let session = SessionState::require_mut(&mut sessions, &session_id)?;
        if !session.windows.contains(&window_id) {
            return Err(SessionError::WindowNotFound(window_id));
        }
        session.active_window = window_id;
        Ok(())
    }
}

impl WindowService {
    /// Split a pane within a window. Caller pre-generates `new_pane_id` so the
    /// PTY spawn can use it before the state mutation; if spawn fails, the
    /// caller invokes [`Self::close_pane`] to roll back.
    pub async fn split_pane(
        &self,
        session_id: SessionId,
        window_id: WindowId,
        target_pane_id: PaneId,
        new_pane_id: PaneId,
        orientation: SplitOrientation,
        side: Side,
    ) -> SessionResult<()> {
        let sessions = self.sessions.lock().await;
        let session = SessionState::require(&sessions, &session_id)?;
        if !session.windows.contains(&window_id) {
            return Err(SessionError::WindowNotFound(window_id));
        }
        drop(sessions);

        let mut win_store = self.windows.lock().await;
        let window = win_store
            .get_mut(&window_id)
            .ok_or_else(|| SessionError::WindowNotFound(window_id.clone()))?;
        if window.session_id() != &session_id {
            return Err(SessionError::WindowDoesNotBelongToSession {
                session_id,
                window_id,
            });
        }
        window.split_pane(&target_pane_id, new_pane_id, orientation, side)?;
        Ok(())
    }

    /// Close a pane within a window. Returns the activity_id whose PTY the
    /// caller must kill (None if the pane has no terminal activity).
    pub async fn close_pane(
        &self,
        session_id: SessionId,
        window_id: WindowId,
        pane_id: PaneId,
    ) -> SessionResult<Option<ActivityId>> {
        let sessions = self.sessions.lock().await;
        let session = SessionState::require(&sessions, &session_id)?;
        if !session.windows.contains(&window_id) {
            return Err(SessionError::WindowNotFound(window_id));
        }
        drop(sessions);

        let mut win_store = self.windows.lock().await;
        let window = win_store
            .get_mut(&window_id)
            .ok_or_else(|| SessionError::WindowNotFound(window_id.clone()))?;
        if window.session_id() != &session_id {
            return Err(SessionError::WindowDoesNotBelongToSession {
                session_id,
                window_id,
            });
        }
        // Capture the activity_id before close mutates the pane.
        let activity_id = window
            .panes()
            .get(&pane_id)
            .ok()
            .and_then(|p| p.first_activity().map(|a| a.id().clone()));
        window.close_pane(&pane_id)?;
        Ok(activity_id)
    }

    /// Remove a session and all of its windows. Returns the activity_ids whose
    /// PTYs the caller must kill.
    pub async fn cascade_delete_session(
        &self,
        session_id: SessionId,
    ) -> SessionResult<Vec<ActivityId>> {
        // Step 1: take the session out of SessionState (closing the entry point).
        let mut sessions = self.sessions.lock().await;
        let removed = sessions
            .remove(&session_id)
            .ok_or_else(|| SessionError::SessionNotFound(session_id.clone()))?;
        drop(sessions);

        // Step 2: remove all windows from WindowStore and collect activity_ids.
        let mut win_store = self.windows.lock().await;
        let mut activity_ids = Vec::new();
        for wid in &removed.windows {
            if let Some(w) = win_store.remove(wid) {
                for (_, pane) in w.panes().iter() {
                    for activity in pane.activities() {
                        activity_ids.push(activity.id().clone());
                    }
                }
            }
        }
        Ok(activity_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn window_service_can_be_constructed() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let _svc = WindowService::new(sessions, windows);
    }

    #[tokio::test]
    async fn create_adds_window_to_session_and_store() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        let (session_id, _wid, _pid, _aid) = sessions.bootstrap_default(&windows).await;

        // Session starts with 1 window (from bootstrap).
        let window = svc
            .create(session_id.clone(), Some("second".into()))
            .await
            .expect("create should succeed");

        assert_eq!(window.name(), "second");

        // Session now has 2 windows.
        let guard = sessions.lock().await;
        let s = guard.get(&session_id).unwrap();
        assert_eq!(s.windows().len(), 2);

        // WindowStore has the new window.
        let win_guard = windows.lock().await;
        assert!(win_guard.get(window.id()).is_some());
    }

    #[tokio::test]
    async fn create_unknown_session_returns_session_not_found() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions, windows);

        let bad_id = SessionId::new();
        let err = svc.create(bad_id.clone(), None).await.unwrap_err();
        assert!(matches!(err, SessionError::SessionNotFound(ref id) if id == &bad_id));
    }

    #[tokio::test]
    async fn rename_changes_window_name() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        let (session_id, window_id, _pid, _aid) = sessions.bootstrap_default(&windows).await;

        let renamed = svc
            .rename(session_id, window_id.clone(), "renamed".into())
            .await
            .expect("rename should succeed");

        assert_eq!(renamed.name(), "renamed");
    }

    #[tokio::test]
    async fn close_removes_window_and_returns_activity_ids() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        let (session_id, first_wid, _pid, _aid) = sessions.bootstrap_default(&windows).await;

        // Create a second window so we can close the first.
        svc.create(session_id.clone(), Some("second".into()))
            .await
            .expect("create second window");

        let activity_ids = svc
            .close(session_id.clone(), first_wid.clone())
            .await
            .expect("close should succeed");

        // First window had 1 pane with 1 activity.
        assert_eq!(activity_ids.len(), 1);

        // Session still has 1 window.
        let guard = sessions.lock().await;
        let s = guard.get(&session_id).unwrap();
        assert_eq!(s.windows().len(), 1);
        drop(guard);

        // First window is gone from the store.
        assert!(windows.lock().await.get(&first_wid).is_none());
    }

    #[tokio::test]
    async fn close_last_window_returns_cannot_close_last_window() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        let (session_id, window_id, _pid, _aid) = sessions.bootstrap_default(&windows).await;

        let err = svc.close(session_id.clone(), window_id).await.unwrap_err();
        assert!(matches!(
            err,
            SessionError::CannotCloseLastWindow(ref sid) if sid == &session_id
        ));
    }

    #[tokio::test]
    async fn select_active_updates_session_active_window() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        let (session_id, first_wid, _pid, _aid) = sessions.bootstrap_default(&windows).await;

        let second = svc
            .create(session_id.clone(), None)
            .await
            .expect("create second");

        svc.select_active(session_id.clone(), second.id().clone())
            .await
            .expect("select_active");

        let guard = sessions.lock().await;
        let s = guard.get(&session_id).unwrap();
        assert_eq!(s.active_window(), second.id());
        // Sanity: first window still exists but is no longer active.
        assert!(s.windows().contains(&first_wid));
    }

    #[tokio::test]
    async fn split_pane_adds_pane_to_window() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        let (session_id, window_id, pane_id, _aid) = sessions.bootstrap_default(&windows).await;
        let new_pane_id = PaneId::new();

        svc.split_pane(
            session_id,
            window_id.clone(),
            pane_id,
            new_pane_id.clone(),
            SplitOrientation::Horizontal,
            Side::After,
        )
        .await
        .expect("split_pane should succeed");

        let win_guard = windows.lock().await;
        let w = win_guard.get(&window_id).unwrap();
        assert_eq!(w.panes().len(), 2);
        assert!(w.panes().get(&new_pane_id).is_ok());
    }

    #[tokio::test]
    async fn close_pane_removes_pane_and_returns_activity_id() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        let (session_id, window_id, pane_id, _aid) = sessions.bootstrap_default(&windows).await;
        let new_pane_id = PaneId::new();

        // First split so we have 2 panes.
        svc.split_pane(
            session_id.clone(),
            window_id.clone(),
            pane_id.clone(),
            new_pane_id.clone(),
            SplitOrientation::Horizontal,
            Side::After,
        )
        .await
        .expect("split");

        // Close the original pane.
        let activity_id = svc
            .close_pane(session_id, window_id.clone(), pane_id.clone())
            .await
            .expect("close_pane");

        assert!(activity_id.is_some());

        let win_guard = windows.lock().await;
        let w = win_guard.get(&window_id).unwrap();
        assert_eq!(w.panes().len(), 1);
        assert!(w.panes().get(&pane_id).is_err());
    }

    #[tokio::test]
    async fn cascade_delete_session_removes_session_and_all_windows() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        let (session_id, window_id, _pid, _aid) = sessions.bootstrap_default(&windows).await;

        // Add a second window.
        svc.create(session_id.clone(), None)
            .await
            .expect("create second window");

        let activity_ids = svc
            .cascade_delete_session(session_id.clone())
            .await
            .expect("cascade delete");

        // 2 windows, each with 1 pane with 1 activity = 2 activity ids.
        assert_eq!(activity_ids.len(), 2);

        // Session is gone.
        assert!(sessions.lock().await.get(&session_id).is_none());

        // First window is gone.
        assert!(windows.lock().await.get(&window_id).is_none());
    }

    #[tokio::test]
    async fn close_returns_back_ref_error_when_window_session_id_mismatch() {
        use crate::Session;

        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let svc = WindowService::new(sessions.clone(), windows.clone());

        // Build session A with a window that claims to belong to session B.
        let sid_a = SessionId::new();
        let sid_b = SessionId::new();
        let wid_real = WindowId::new();
        let wid_extra = WindowId::new();

        // Session A lists wid_real (legitimate, session_id=A) and wid_extra (corrupt, session_id=B).
        let session_a = Session::empty(sid_a.clone(), "a".into(), wid_real.clone());
        let real_window = Window::new(wid_real.clone(), sid_a.clone(), "real".into());
        let mismatched_window = Window::new(wid_extra.clone(), sid_b, "mismatched".into());

        sessions.lock().await.insert(sid_a.clone(), session_a);
        // Manually inject a 2nd window into Session A's list to bypass create's enforcement.
        {
            let mut s = sessions.lock().await;
            let entry = s.get_mut(&sid_a).unwrap();
            entry.windows.push(wid_extra.clone());
        }
        windows.lock().await.insert(wid_real.clone(), real_window);
        windows
            .lock()
            .await
            .insert(wid_extra.clone(), mismatched_window);

        // Closing wid_extra should detect the back-ref violation and refuse.
        let err = svc
            .close(sid_a.clone(), wid_extra.clone())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            SessionError::WindowDoesNotBelongToSession { .. }
        ));

        // The window should still be in WindowStore (rolled back).
        assert!(windows.lock().await.get(&wid_extra).is_some());
    }
}
