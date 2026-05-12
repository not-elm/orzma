pub mod error;
pub mod extractors;
pub mod handlers;
pub mod layout_broadcast;
pub mod layout_dto;

pub use error::{HttpError, HttpResult};

use axum::{
    Router,
    extract::FromRef,
    routing::{delete as method_delete, get, post},
};
use dashmap::DashMap;
use layout_broadcast::LayoutBroadcaster;
use ozmux_extension::ExtensionRegistry;
use ozmux_multiplexer::{
    Activity, ActivityId, MultiplexerError, MultiplexerResult, PaneId, Session, SessionId,
    SessionState, Window, WindowId,
};
use ozmux_terminal::TerminalService;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct AppState {
    pub sessions: Arc<Mutex<SessionState>>,
    pub windows: Arc<DashMap<WindowId, Arc<Mutex<Window>>>>,
    pub pane_owner_window: Arc<DashMap<PaneId, WindowId>>,
    pub limbo: LimboStore,
    pub terminal: TerminalService,
    pub extensions: ExtensionRegistry,
    pub layout_broadcast: LayoutBroadcaster,
}

/// Transitional limbo store for the pre-PR5 SDK flow
/// (createActivity → createPane → splitPane). Removed in PR7 when the
/// legacy split-with API disappears.
#[derive(Clone, Default)]
pub struct LimboStore {
    pub activities: Arc<DashMap<ActivityId, Activity>>,
    pub panes: Arc<DashMap<PaneId, ActivityId>>,
}

impl FromRef<AppState> for TerminalService {
    fn from_ref(input: &AppState) -> Self {
        input.terminal.clone()
    }
}

impl FromRef<AppState> for ExtensionRegistry {
    fn from_ref(input: &AppState) -> Self {
        input.extensions.clone()
    }
}

impl FromRef<AppState> for LayoutBroadcaster {
    fn from_ref(input: &AppState) -> Self {
        input.layout_broadcast.clone()
    }
}

impl AppState {
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

    /// Create a Window optionally attached to a Session. IDs are generated
    /// server-side here (PR5 makes them caller-supplied).
    ///
    /// Lock order: sessions → windows[wid].
    pub async fn create_window(
        &self,
        session_id: Option<&SessionId>,
        name: Option<String>,
    ) -> MultiplexerResult<(WindowId, PaneId, ActivityId)> {
        let mut sess = self.sessions.lock().await;
        if let Some(sid) = session_id
            && sess.get(sid).is_none()
        {
            return Err(MultiplexerError::SessionNotFound(sid.clone()));
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
            let session = sess.get_mut(sid).expect("validated existence above");
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
        let session = sess
            .get_mut(sid)
            .ok_or_else(|| MultiplexerError::SessionNotFound(sid.clone()))?;
        session.rename(name);
        Ok(())
    }

    /// Close a Window: tear down its Panes/Activities, detach from any
    /// owning Sessions, clean up runtime resources (PTYs, extension
    /// registry, layout broadcast).
    ///
    /// Lock order: sessions → windows[wid] → drop in reverse.
    pub async fn close_window(&self, wid: &WindowId) -> MultiplexerResult<Vec<ActivityId>> {
        let mut sess = self.sessions.lock().await;

        // Atomically remove the Window from the DashMap so no later caller
        // observes a half-torn-down Window. Holding `sess` here keeps any
        // concurrent `delete_session` blocked until cleanup finishes.
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

        for pid in pane_ids {
            self.pane_owner_window.remove(&pid);
            self.extensions.forget_pane(&pid);
        }
        for aid in &activities {
            let _ = self.terminal.kill(aid).await;
            self.extensions.forget_activity(aid);
        }
        self.layout_broadcast.close(wid);
        Ok(activities)
    }

    /// Delete a Session, cascading into every Window it owns.
    pub async fn delete_session(&self, sid: &SessionId) -> MultiplexerResult<Vec<ActivityId>> {
        let linked_windows = {
            let mut sess = self.sessions.lock().await;
            let session = sess.remove(sid)?;
            session.linked_windows
        };

        let mut activities = Vec::new();
        for wid in linked_windows {
            activities.extend(self.close_window(&wid).await?);
        }
        Ok(activities)
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

    /// Look up an Activity's metadata regardless of which Window owns it. Walks
    /// every Window then the limbo store. Used by `iframe_serve`.
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
        self.limbo.activities.get(aid).map(|e| e.clone())
    }
}

pub async fn serve(state: AppState) -> HttpResult {
    let sid = state.create_session(None).await;
    let (wid, pid, aid) = state
        .create_window(Some(&sid), None)
        .await
        .expect("bootstrap cannot fail on empty AppState");

    if let Err(e) = state
        .terminal
        .spawn(
            pid,
            aid,
            ozmux_terminal::SpawnOptions {
                cols: 80,
                rows: 24,
                shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                cwd: None,
                window_id: Some(wid),
                session_id: Some(sid),
            },
        )
        .await
    {
        panic!("bootstrap PTY spawn failed: {e}");
    }

    let listener = TcpListener::bind("127.0.0.1:3200")
        .await
        .map_err(|e| HttpError::FailedLaunch(e.to_string()))?;
    axum::serve(listener, daemon_router(state))
        .await
        .map_err(|e| HttpError::FailedLaunch(e.to_string()))?;
    Ok(())
}

pub fn daemon_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::index::handler))
        .route("/health", get(handlers::health::check))
        .route(
            "/sessions",
            get(handlers::sessions::list).post(handlers::sessions::create),
        )
        .route(
            "/sessions/{session_id}",
            get(handlers::sessions::get)
                .patch(handlers::sessions::rename)
                .delete(handlers::sessions::delete),
        )
        .route("/windows", post(handlers::windows::create))
        .route(
            "/windows/{window_id}",
            get(handlers::windows::get)
                .patch(handlers::windows::rename)
                .delete(handlers::windows::delete),
        )
        .route(
            "/windows/{window_id}/select",
            post(handlers::windows::select),
        )
        .route(
            "/windows/{window_id}/events",
            get(handlers::windows::events),
        )
        .route("/panes", post(handlers::panes::create))
        .route("/panes/{pane_id}/split", post(handlers::panes::split))
        .route("/panes/{src}/split-with", post(handlers::panes::split_with))
        .route("/panes/{pane_id}", method_delete(handlers::panes::close))
        .route(
            "/windows/{window_id}/panes/{pane_id}/activate",
            post(handlers::panes::activate),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/split",
            post(handlers::panes::split_v2),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}",
            method_delete(handlers::panes::close_v2),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities",
            post(handlers::activities::add_to_pane),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities/{activity_id}/activate",
            post(handlers::activities::activate_v2),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities/{activity_id}/terminal/ws",
            get(handlers::activities::terminal_ws_hierarchical),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities/{activity_id}/handlers/ws",
            get(handlers::activities::handlers_ws_hierarchical),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities/{activity_id}/iframe/{*path}",
            get(handlers::activities::iframe_serve_hierarchical),
        )
        .route("/activities", post(handlers::activities::create))
        .route(
            "/activities/{activity_id}/terminal/ws",
            get(handlers::activities::terminal_ws),
        )
        .route(
            "/activities/{activity_id}/handlers/ws",
            get(handlers::activities::handlers_ws),
        )
        .route(
            "/activities/{activity_id}/iframe/{*path}",
            get(handlers::activities::iframe_serve),
        )
        .with_state(state)
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::{ActivityId, AppState, PaneId, Session, SessionId, WindowId, daemon_router};
    use axum::Router;

    pub fn fresh_state() -> AppState {
        AppState::default()
    }

    pub fn daemon_router_for_test(state: AppState) -> Router {
        daemon_router(state)
    }

    pub fn router_with(state: AppState) -> (Router, AppState) {
        (daemon_router(state.clone()), state)
    }

    pub fn router_with_registry(
        state: AppState,
        registry: ozmux_extension::ExtensionRegistry,
    ) -> (Router, AppState) {
        let state = AppState {
            extensions: registry,
            ..state
        };
        (daemon_router(state.clone()), state)
    }

    /// Bootstrap test fixture: registers one Session with one Window
    /// (one Pane, one Activity). Returns the four ids.
    pub async fn bootstrap_default(state: &AppState) -> (SessionId, WindowId, PaneId, ActivityId) {
        let sid = {
            let mut sess = state.sessions.lock().await;
            let sid = SessionId::new();
            sess.register(sid.clone(), Session::empty("Session1"));
            sid
        };
        let (wid, pid, aid) = state.create_window(Some(&sid), None).await.unwrap();
        (sid, wid, pid, aid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn app_state_default_includes_layout_broadcaster() {
        let state = AppState::default();
        let _ = state.layout_broadcast.subscribe_or_create(&WindowId::new());
    }

    #[tokio::test]
    async fn delete_pane_without_extension_header_returns_404() {
        let (router, _) = test_helpers::router_with(test_helpers::fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/panes/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn concurrent_split_on_different_windows_does_not_serialize() {
        use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation};

        let state = test_helpers::fresh_state();
        let sid = state.create_session(Some("s".into())).await;
        let (wid_a, pid_a, _) = state.create_window(Some(&sid), None).await.unwrap();
        let (wid_b, pid_b, _) = state.create_window(Some(&sid), None).await.unwrap();

        let state_a = state.clone();
        let pid_a_c = pid_a.clone();
        let wid_a_c = wid_a.clone();
        let h_a = tokio::spawn(async move {
            state_a
                .with_window_or_404(&wid_a_c, |w| {
                    let np = PaneId::new();
                    let activity = Activity::terminal(ActivityId::new());
                    w.split_pane(
                        &pid_a_c,
                        np,
                        activity,
                        Side::After,
                        SplitOrientation::Horizontal,
                    )
                })
                .await
        });

        let state_b = state.clone();
        let pid_b_c = pid_b.clone();
        let wid_b_c = wid_b.clone();
        let h_b = tokio::spawn(async move {
            state_b
                .with_window_or_404(&wid_b_c, |w| {
                    let np = PaneId::new();
                    let activity = Activity::terminal(ActivityId::new());
                    w.split_pane(
                        &pid_b_c,
                        np,
                        activity,
                        Side::After,
                        SplitOrientation::Horizontal,
                    )
                })
                .await
        });

        let (a, b) = tokio::join!(h_a, h_b);
        a.unwrap().unwrap();
        b.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_close_window_and_delete_session_no_deadlock() {
        let state = test_helpers::fresh_state();
        let sid = state.create_session(Some("s".into())).await;
        let (wid_a, _, _) = state.create_window(Some(&sid), None).await.unwrap();
        let (_wid_b, _, _) = state.create_window(Some(&sid), None).await.unwrap();

        let state_a = state.clone();
        let h_close = tokio::spawn(async move { state_a.close_window(&wid_a).await });

        let state_s = state.clone();
        let sid_c = sid.clone();
        let h_delete = tokio::spawn(async move { state_s.delete_session(&sid_c).await });

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let _ = tokio::join!(h_close, h_delete);
        })
        .await;
        assert!(
            result.is_ok(),
            "deadlock: operations did not complete within 5s"
        );
    }
}
