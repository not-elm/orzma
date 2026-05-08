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
pub mod service;
pub mod window;
pub mod window_service;
pub mod session;

pub use error::{SessionError, SessionResult};
pub use window::{Window, WindowId, WindowStore};
pub use window_service::WindowService;

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

#[derive(Clone, Default)]
pub struct SessionState(Arc<Mutex<HashMap<SessionId, Session>>>);

impl SessionState {
    pub async fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, Session>> {
        self.0.lock().await
    }

    pub async fn session(&self, id: &SessionId) -> SessionResult<SessionRef<'_>> {
        let guard = self.0.lock().await;
        // tokio's MutexGuard::try_map requires the projection to return `&mut T`,
        // even when the caller will only use `&T`. The returned SessionRef wraps
        // the MappedMutexGuard and exposes only `Deref` (not `DerefMut`),
        // enforcing read-only access at the API surface.
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
