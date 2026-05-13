use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

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

/// In-memory domain model. Owns the multiplexer's three stores (sessions,
/// windows, pane_owner_window index). Pure data — no PTY, no extension
/// registry, no layout broadcast. Side-effecting orchestration lives in the
/// http_server `AppState`.
#[derive(Clone, Default)]
pub struct MultiplexerService {
    pub sessions: Arc<Mutex<SessionState>>,
    pub windows: Arc<DashMap<WindowId, Arc<Mutex<Window>>>>,
    pub pane_owner_window: Arc<DashMap<PaneId, WindowId>>,
}

impl MultiplexerService {
    /// Clone the `Arc<Mutex<Window>>` out of the DashMap, then drop the
    /// DashMap `Ref` before acquiring the tokio mutex. This preserves the
    /// DASHMAP-GUARD invariant: never hold a Ref across an `.await`.
    pub async fn with_window<R>(
        &self,
        wid: &WindowId,
        f: impl FnOnce(&mut Window) -> R,
    ) -> Option<R> {
        let arc = self.windows.get(wid).map(|e| e.clone())?;
        let mut win = arc.lock().await;
        Some(f(&mut win))
    }

    /// Same as `with_window` but propagates `WindowNotFound` for handler
    /// ergonomics.
    pub async fn with_window_or_404<R>(
        &self,
        wid: &WindowId,
        f: impl FnOnce(&mut Window) -> MultiplexerResult<R>,
    ) -> MultiplexerResult<R> {
        self.with_window(wid, f)
            .await
            .ok_or_else(|| MultiplexerError::WindowNotFound(wid.clone()))?
    }

    /// Create a Session and register it.
    pub async fn create_session(&self, name: Option<String>) -> SessionId {
        let mut sess = self.sessions.lock().await;
        let session_id = SessionId::new();
        let session_name = name.unwrap_or_else(|| format!("Session{}", sess.len() + 1));
        sess.register(session_id.clone(), Session::empty(session_name));
        session_id
    }

    /// Create a Window optionally attached to a Session.
    ///
    /// Lock order: sessions → windows\[wid\].
    pub async fn create_window(
        &self,
        session_id: Option<&SessionId>,
        name: Option<String>,
    ) -> MultiplexerResult<(WindowId, PaneId, ActivityId)> {
        let mut sess = self.sessions.lock().await;
        if let Some(sid) = session_id {
            sess.get(sid)?;
        }

        let window_id = WindowId::new();
        let pane_id = PaneId::new();
        let activity = Activity::terminal(ActivityId::new());
        let activity_id = activity.id.clone();
        let window_name = name.unwrap_or_else(|| format!("Window{}", self.windows.len() + 1));
        let window =
            Window::new_with_initial(window_id.clone(), window_name, pane_id.clone(), activity);

        self.windows
            .insert(window_id.clone(), Arc::new(Mutex::new(window)));
        self.pane_owner_window
            .insert(pane_id.clone(), window_id.clone());

        if let Some(sid) = session_id {
            let session = sess.get_mut(sid)?;
            session.attach_window(window_id.clone());
        }

        Ok((window_id, pane_id, activity_id))
    }

    /// Rename a Window in-place.
    pub async fn rename_window(&self, wid: &WindowId, name: String) -> MultiplexerResult<()> {
        self.with_window_or_404(wid, |w| {
            w.rename(name);
            Ok(())
        })
        .await
    }

    /// Rename a Session.
    pub async fn rename_session(&self, sid: &SessionId, name: String) -> MultiplexerResult<()> {
        let mut sess = self.sessions.lock().await;
        let session = sess.get_mut(sid)?;
        session.rename(name);
        Ok(())
    }

    /// Promote `wid` to the active window of whichever Session owns it.
    pub async fn select_active_window(&self, wid: &WindowId) -> MultiplexerResult<()> {
        if !self.windows.contains_key(wid) {
            return Err(MultiplexerError::WindowNotFound(wid.clone()));
        }
        let mut sess = self.sessions.lock().await;
        for (_, session) in sess.iter_mut() {
            if session.linked_windows.contains(wid) {
                session.active_window = Some(wid.clone());
                return Ok(());
            }
        }
        Err(MultiplexerError::WindowNotAttachedToSession(wid.clone()))
    }

    /// Resolve which Window currently owns `pid`. Returns `PaneNotFound`
    /// when the pane has no recorded owner.
    pub fn lookup_pane_window(&self, pid: &PaneId) -> MultiplexerResult<WindowId> {
        self.pane_owner_window
            .get(pid)
            .map(|e| e.clone())
            .ok_or_else(|| MultiplexerError::PaneNotFound(pid.clone()))
    }

    /// Look up an Activity's metadata regardless of which Window owns it. Walks
    /// every Window. Used by iframe-serve.
    pub async fn activity_metadata(&self, aid: &ActivityId) -> Option<Activity> {
        for entry in self.windows.iter() {
            let win_arc = entry.value().clone();
            drop(entry);
            let win = win_arc.lock().await;
            for (_, p) in win.panes.iter() {
                if let Some(a) = p.activity(aid) {
                    return Some(a.clone());
                }
            }
        }
        None
    }

    /// Remove a Window from the store, detach it from any Session that
    /// references it, and drop its pane_owner_window rows. Returns the
    /// activities and pane_ids the caller must clean up (PTY kill, extension
    /// registry forget, layout broadcast close).
    ///
    /// Lock order: sessions → windows\[wid\] → drop in reverse.
    pub async fn close_window_data(
        &self,
        wid: &WindowId,
    ) -> MultiplexerResult<(Vec<ActivityId>, Vec<PaneId>)> {
        let mut sess = self.sessions.lock().await;

        // Atomically remove the Window from the DashMap so no later caller
        // observes a half-torn-down Window. Holding `sess` here keeps any
        // concurrent `take_session_windows` blocked until cleanup finishes.
        let arc = self
            .windows
            .remove(wid)
            .map(|(_, v)| v)
            .ok_or_else(|| MultiplexerError::WindowNotFound(wid.clone()))?;
        let win = arc.lock().await;

        let activities = win.collect_activities_for_cleanup();
        let pane_ids: Vec<PaneId> = win.pane_ids().cloned().collect();

        for (_, session) in sess.iter_mut() {
            session.detach_window(wid);
        }
        drop(win);
        drop(arc);
        drop(sess);

        for pid in &pane_ids {
            self.pane_owner_window.remove(pid);
        }
        Ok((activities, pane_ids))
    }

    /// Remove a Session and return the WindowIds that were attached to it.
    /// The caller drives `close_window_data` for each returned wid.
    pub async fn take_session_windows(&self, sid: &SessionId) -> MultiplexerResult<Vec<WindowId>> {
        let mut sess = self.sessions.lock().await;
        let session = sess.remove(sid)?;
        Ok(session.linked_windows)
    }
}
