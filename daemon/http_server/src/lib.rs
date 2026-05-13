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
use layout_broadcast::LayoutBroadcaster;
use ozmux_extension::ExtensionRegistry;
use ozmux_multiplexer::{ActivityId, MultiplexerResult, MultiplexerService, SessionId, WindowId};
use ozmux_terminal::TerminalService;
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct AppState {
    pub multiplexer: MultiplexerService,
    pub terminal: TerminalService,
    pub extensions: ExtensionRegistry,
    pub layout_broadcast: LayoutBroadcaster,
}

impl AppState {
    /// Build an `AppState` wired to the supplied runtime services. This is the
    /// only sanctioned construction path outside tests — `Default` is
    /// intentionally not derived so callers cannot silently produce a state
    /// whose `TerminalService`, `ExtensionRegistry`, or `LayoutBroadcaster`
    /// are detached from the daemon's runtime root.
    pub fn new(
        terminal: TerminalService,
        extensions: ExtensionRegistry,
        layout_broadcast: LayoutBroadcaster,
    ) -> Self {
        Self {
            multiplexer: MultiplexerService::default(),
            terminal,
            extensions,
            layout_broadcast,
        }
    }
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

impl FromRef<AppState> for MultiplexerService {
    fn from_ref(input: &AppState) -> Self {
        input.multiplexer.clone()
    }
}

impl AppState {
    /// Close a Window: tear down its Panes/Activities and run runtime
    /// cleanup. Delegates the data half to `multiplexer.close_window_data`
    /// and then issues PTY kills, extension registry forgets, and a layout
    /// broadcast close.
    pub async fn close_window(&self, wid: &WindowId) -> MultiplexerResult<Vec<ActivityId>> {
        let (activities, pane_ids) = self.multiplexer.close_window_data(wid).await?;
        for pid in pane_ids {
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
        let linked = self.multiplexer.take_session_windows(sid).await?;
        let mut activities = Vec::new();
        for wid in linked {
            activities.extend(self.close_window(&wid).await?);
        }
        Ok(activities)
    }
}

pub async fn serve(state: AppState) -> HttpResult {
    let sid = state.multiplexer.create_session(None).await;
    let (wid, pid, aid) = state
        .multiplexer
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
            get(handlers::sessions::list::list).post(handlers::sessions::create::create),
        )
        .route(
            "/sessions/{session_id}",
            get(handlers::sessions::get::get)
                .patch(handlers::sessions::rename::rename)
                .delete(handlers::sessions::delete::delete),
        )
        .route("/windows", post(handlers::windows::create::create))
        .route(
            "/windows/{window_id}",
            get(handlers::windows::get)
                .patch(handlers::windows::rename::rename)
                .delete(handlers::windows::delete::delete),
        )
        .route(
            "/windows/{window_id}/select",
            post(handlers::windows::select::select),
        )
        .route(
            "/windows/{window_id}/events",
            get(handlers::windows::events::events),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activate",
            post(handlers::windows::panes::activate::activate),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/split",
            post(handlers::windows::panes::split::split),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}",
            method_delete(handlers::windows::panes::close::close),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities",
            post(handlers::windows::panes::activities::add_to_pane::add_to_pane),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities/{activity_id}/activate",
            post(handlers::windows::panes::activities::activate::activate),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities/{activity_id}/terminal/ws",
            get(handlers::windows::panes::activities::terminal_ws::terminal_ws),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities/{activity_id}/handlers/ws",
            get(handlers::windows::panes::activities::handlers_ws::handlers_ws),
        )
        .route(
            "/windows/{window_id}/panes/{pane_id}/activities/{activity_id}/iframe/{*path}",
            get(handlers::windows::panes::activities::iframe_serve::iframe_serve),
        )
        .with_state(state)
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::{ActivityId, AppState, SessionId, WindowId, daemon_router};
    use axum::Router;
    use ozmux_multiplexer::PaneId;

    pub fn fresh_state() -> AppState {
        AppState::new(
            ozmux_terminal::TerminalService::default(),
            ozmux_extension::ExtensionRegistry::default(),
            crate::layout_broadcast::LayoutBroadcaster::default(),
        )
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
        let sid = state
            .multiplexer
            .create_session(Some("Session1".into()))
            .await;
        let (wid, pid, aid) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        (sid, wid, pid, aid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_state_default_includes_layout_broadcaster() {
        let state = test_helpers::fresh_state();
        let _ = state.layout_broadcast.subscribe_or_create(&WindowId::new());
    }

    #[tokio::test]
    async fn concurrent_split_on_different_windows_does_not_serialize() {
        use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation};

        let state = test_helpers::fresh_state();
        let sid = state.multiplexer.create_session(Some("s".into())).await;
        let (wid_a, pid_a, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let (wid_b, pid_b, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();

        let state_a = state.clone();
        let pid_a_c = pid_a.clone();
        let wid_a_c = wid_a.clone();
        let h_a = tokio::spawn(async move {
            state_a
                .multiplexer
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
                .multiplexer
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
        let sid = state.multiplexer.create_session(Some("s".into())).await;
        let (wid_a, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let (_wid_b, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();

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
