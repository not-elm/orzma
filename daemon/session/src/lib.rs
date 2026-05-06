use ozmux_macros::NewType;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard};

pub mod activity;
pub mod cell;
pub mod error;
pub mod pane;
pub mod window;
pub mod window_service;

pub use error::{SessionError, SessionResult};
pub use window::{Window, WindowId, WindowStore};
pub use window_service::WindowService;

#[derive(Clone, Default)]
pub struct SessionState(Arc<Mutex<HashMap<SessionId, Session>>>);

/// Read-only guard returned by [`SessionState::session`].
///
/// Holds the underlying mutex lock until dropped; only `Deref` is exposed
/// so callers cannot mutate the session through this type.
#[derive(Debug)]
pub struct SessionRef<'a>(MappedMutexGuard<'a, Session>);

impl Deref for SessionRef<'_> {
    type Target = Session;

    fn deref(&self) -> &Session {
        &self.0
    }
}

/// Exclusive guard returned by [`SessionState::session_mut`].
///
/// Holds the underlying mutex lock until dropped. Implements both `Deref`
/// and `DerefMut`, so callers can mutate the session in place and serialize
/// the result while still holding the lock.
#[derive(Debug)]
pub struct SessionGuard<'a>(MappedMutexGuard<'a, Session>);

impl Deref for SessionGuard<'_> {
    type Target = Session;

    fn deref(&self) -> &Session {
        &self.0
    }
}

impl DerefMut for SessionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.0
    }
}

impl SessionState {
    pub async fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, Session>> {
        self.0.lock().await
    }

    pub async fn session(&self, id: &SessionId) -> SessionResult<SessionRef<'_>> {
        let guard = self.0.lock().await;
        let session = MutexGuard::try_map(guard, |sessions| sessions.get_mut(id))
            .map_err(|_| SessionError::SessionNotFound(id.clone()))?;
        Ok(SessionRef(session))
    }

    pub async fn session_mut(&self, id: &SessionId) -> SessionResult<SessionGuard<'_>> {
        let guard = self.0.lock().await;
        let session = MutexGuard::try_map(guard, |sessions| sessions.get_mut(id))
            .map_err(|_| SessionError::SessionNotFound(id.clone()))?;
        Ok(SessionGuard(session))
    }

    pub async fn remove(&self, id: &SessionId) -> SessionResult<Session> {
        let mut guard = self.0.lock().await;
        guard
            .remove(id)
            .ok_or_else(|| SessionError::SessionNotFound(id.clone()))
    }

    /// Look up a session inside an already-locked guard.
    ///
    /// Use this when you need to hold the SessionState guard across additional
    /// operations (e.g., acquiring `WindowStore` under the canonical lock order).
    /// For one-shot reads or mutations that do not coordinate with another store,
    /// prefer [`Self::session`] / [`Self::session_mut`].
    pub(crate) fn require<'a>(
        guard: &'a HashMap<SessionId, Session>,
        id: &SessionId,
    ) -> SessionResult<&'a Session> {
        guard
            .get(id)
            .ok_or_else(|| SessionError::SessionNotFound(id.clone()))
    }

    pub(crate) fn require_mut<'a>(
        guard: &'a mut HashMap<SessionId, Session>,
        id: &SessionId,
    ) -> SessionResult<&'a mut Session> {
        guard
            .get_mut(id)
            .ok_or_else(|| SessionError::SessionNotFound(id.clone()))
    }

    /// Insert a default Session, a default Window for that session, a default Pane,
    /// and a default Terminal Activity. Returns IDs needed for PTY spawn.
    pub async fn bootstrap_default(
        &self,
        windows: &crate::window::WindowStore,
    ) -> (
        SessionId,
        crate::window::WindowId,
        crate::pane::PaneId,
        crate::activity::ActivityId,
    ) {
        // Build the in-memory graph outside any lock.
        let session_id = SessionId::new();
        let window_id = crate::window::WindowId::new();
        let window =
            crate::window::Window::new(window_id.clone(), session_id.clone(), "main".into());
        let pane_id = window
            .first_pane()
            .expect("Window::new has 1 pane")
            .id()
            .clone();
        let activity_id = window
            .first_pane()
            .unwrap()
            .first_activity()
            .expect("Pane::default has 1 Activity")
            .id()
            .clone();
        let session = Session::empty(session_id.clone(), "default".into(), window_id.clone());

        // Publish in canonical lock order: SessionState -> WindowStore.
        let mut sessions = self.0.lock().await;
        let mut win_store = windows.lock().await;
        sessions.insert(session_id.clone(), session);
        win_store.insert(window_id.clone(), window);
        drop(win_store);
        drop(sessions);

        (session_id, window_id, pane_id, activity_id)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, NewType)]
#[newtype(as_ref(str), display, new(uuid_v4_string), default)]
pub struct SessionId(String);

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub(crate) id: SessionId,
    pub(crate) name: String,
    pub(crate) windows: Vec<crate::window::WindowId>,
    pub(crate) active_window: crate::window::WindowId,
}

impl Session {
    /// Construct a session with no windows. Use `WindowService::create` to add
    /// windows; an empty windows list violates an invariant — `bootstrap_default`
    /// must be paired with a window insert.
    pub fn empty(id: SessionId, name: String, default_window: crate::window::WindowId) -> Self {
        Self {
            id,
            name,
            windows: vec![default_window.clone()],
            active_window: default_window,
        }
    }

    pub const fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn rename(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    pub fn windows(&self) -> &[crate::window::WindowId] {
        &self.windows
    }

    pub fn active_window(&self) -> &crate::window::WindowId {
        &self.active_window
    }
}

impl Default for Session {
    fn default() -> Self {
        // Default impl is a test scaffold only. The fresh `WindowId` it allocates has
        // no backing `Window` in any `WindowStore`, so callers that read `active_window`
        // through a store will fail. Use `Session::empty(...)` and pair with a real
        // `Window` insert (as `bootstrap_default` does) for any code that touches the
        // store.
        let session_id = SessionId::new();
        let window_id = crate::window::WindowId::new();
        Self::empty(session_id, String::new(), window_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::SessionError;
    use crate::window::{WindowId, WindowStore};

    #[test]
    fn session_carries_its_id_and_name() {
        let s = Session::empty(SessionId::new(), "demo".into(), WindowId::new());
        assert!(!s.id().as_ref().is_empty());
        assert_eq!(s.name(), "demo");
    }

    #[test]
    fn two_new_sessions_get_distinct_ids() {
        let a = Session::empty(SessionId::new(), String::new(), WindowId::new());
        let b = Session::empty(SessionId::new(), String::new(), WindowId::new());
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn session_serializes_with_id_name_windows_active_window() {
        let s = Session::empty(SessionId::new(), "hello".into(), WindowId::new());
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["name"].as_str(), Some("hello"));
        assert!(v["id"].is_string());
        assert!(v["windows"].is_array());
        assert_eq!(v["windows"].as_array().unwrap().len(), 1);
        assert!(v["active_window"].is_string());
    }

    #[tokio::test]
    async fn session_state_lock_starts_empty() {
        let state = SessionState::default();
        assert!(state.lock().await.is_empty());
    }

    #[tokio::test]
    async fn session_returns_ref_for_existing_id() {
        let state = SessionState::default();
        let id = SessionId::new();
        state.lock().await.insert(
            id.clone(),
            Session::empty(id.clone(), "hello".into(), WindowId::new()),
        );
        let r = state.session(&id).await.expect("exists");
        assert_eq!(r.name(), "hello");
    }

    #[tokio::test]
    async fn session_returns_err_for_unknown_id() {
        let state = SessionState::default();
        let id = SessionId::new();
        let err = state.session(&id).await.unwrap_err();
        assert!(matches!(err, SessionError::SessionNotFound(ref got) if got == &id));
    }

    #[tokio::test]
    async fn session_mut_allows_in_place_mutation() {
        let state = SessionState::default();
        let id = SessionId::new();
        state.lock().await.insert(
            id.clone(),
            Session::empty(id.clone(), "old".into(), WindowId::new()),
        );

        {
            let mut g = state.session_mut(&id).await.unwrap();
            g.rename("new");
        }
        assert_eq!(state.session(&id).await.unwrap().name(), "new");
    }

    #[tokio::test]
    async fn remove_returns_session_and_removes_it() {
        let state = SessionState::default();
        let id = SessionId::new();
        state.lock().await.insert(
            id.clone(),
            Session::empty(id.clone(), String::new(), WindowId::new()),
        );
        let removed = state.remove(&id).await.unwrap();
        assert_eq!(removed.id(), &id);
        assert!(state.lock().await.get(&id).is_none());
    }

    #[tokio::test]
    async fn session_mut_returns_err_for_unknown_id() {
        let state = SessionState::default();
        let id = SessionId::new();
        let err = state.session_mut(&id).await.unwrap_err();
        assert!(matches!(err, SessionError::SessionNotFound(ref got) if got == &id));
    }

    #[tokio::test]
    async fn remove_returns_err_for_unknown_id() {
        let state = SessionState::default();
        let id = SessionId::new();
        let err = state.remove(&id).await.unwrap_err();
        assert!(matches!(err, SessionError::SessionNotFound(ref got) if got == &id));
    }

    #[tokio::test]
    async fn bootstrap_default_inserts_session_and_window_and_returns_4tuple() {
        use crate::activity::ActivityKind;
        let state = SessionState::default();
        let windows = WindowStore::default();

        let (sid, wid, pid, aid) = state.bootstrap_default(&windows).await;

        let sessions = state.lock().await;
        assert_eq!(sessions.len(), 1);
        let s = sessions.get(&sid).unwrap();
        assert_eq!(s.windows(), std::slice::from_ref(&wid));
        assert_eq!(s.active_window(), &wid);

        let store = windows.lock().await;
        let w = store.get(&wid).unwrap();
        let p = w.panes().get(&pid).expect("pane exists");
        assert_eq!(p.id(), &pid);
        let a = p.first_activity().unwrap();
        assert_eq!(a.id(), &aid);
        assert!(matches!(a.kind(), ActivityKind::Terminal));
    }
}
