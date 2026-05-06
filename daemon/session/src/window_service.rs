//! Domain service that orchestrates Session + Window mutations under the
//! canonical lock order: SessionState -> WindowStore.
//!
//! All multi-store mutations go through this service. Handlers must not lock
//! `SessionState` and `WindowStore` directly.

use crate::{
    SessionId, SessionState,
    activity::ActivityId,
    error::{SessionError, SessionResult},
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
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| SessionError::SessionNotFound(session_id.clone()))?;

        let window_id = WindowId::new();
        let assigned_name =
            name.unwrap_or_else(|| format!("window-{}", session.windows.len() + 1));
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
        let session = sessions
            .get(&session_id)
            .ok_or_else(|| SessionError::SessionNotFound(session_id.clone()))?;
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
        let session = sessions
            .get_mut(&session_id)
            .ok_or_else(|| SessionError::SessionNotFound(session_id.clone()))?;
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
        let activity_ids: Vec<ActivityId> = removed
            .panes()
            .iter()
            .flat_map(|(_, pane)| pane.activities().iter().map(|a| a.id().clone()))
            .collect();
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
}
