//! HTTP/WebSocket server: axum router, REST handlers, and PTY WS bridging.

pub mod error;
pub mod extractors;
pub mod handlers;
pub mod layout_broadcast;
pub mod layout_dto;
pub mod state;
pub mod window_view;
mod title_republish;

pub use error::{HttpError, HttpResult};
pub use state::AppState;

use axum::{
    Router,
    routing::{get, post},
};
use tokio::net::TcpListener;

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
    tokio::spawn(title_republish::run(state.clone()));
    axum::serve(listener, daemon_router(state))
        .await
        .map_err(|e| HttpError::FailedLaunch(e.to_string()))?;
    Ok(())
}

pub fn daemon_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::index::handler))
        .route("/health", get(handlers::health::check))
        .nest("/sessions", sessions_router())
        .nest("/windows", windows_router())
        .nest("/configs", configs_router())
        .with_state(state)
}

/// Router for read-only config endpoints under `/configs`.
pub fn configs_router() -> Router<AppState> {
    Router::new()
        .route("/shortcuts", get(handlers::configs::shortcuts::get))
        .route("/font", get(handlers::configs::font::get))
}

pub fn sessions_router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(handlers::sessions::list::list).post(handlers::sessions::create::create),
        )
        .route(
            "/{session_id}",
            get(handlers::sessions::get::get)
                .patch(handlers::sessions::rename::rename)
                .delete(handlers::sessions::delete::delete),
        )
}

pub fn windows_router() -> Router<AppState> {
    Router::new()
        .route("/", post(handlers::windows::create::create))
        .route(
            "/{window_id}",
            get(handlers::windows::get)
                .patch(handlers::windows::rename::rename)
                .delete(handlers::windows::delete::delete),
        )
        .route(
            "/{window_id}/select",
            post(handlers::windows::select::select),
        )
        .route(
            "/{window_id}/events",
            get(handlers::windows::events::events),
        )
        .nest("/{window_id}/panes", handlers::windows::panes::router())
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::{AppState, daemon_router};
    use axum::Router;
    use ozmux_multiplexer::{ActivityId, PaneId, SessionId, WindowId};

    pub(crate) fn fresh_state() -> AppState {
        AppState::new(
            ozmux_terminal::TerminalService::default(),
            ozmux_extension::ExtensionRegistry::default(),
            crate::layout_broadcast::LayoutBroadcaster::default(),
            std::sync::Arc::new(ozmux_configs::OzmuxConfigs::default()),
        )
    }

    pub(crate) fn daemon_router_for_test(state: AppState) -> Router {
        daemon_router(state)
    }

    pub(crate) fn router_with(state: AppState) -> (Router, AppState) {
        (daemon_router(state.clone()), state)
    }

    pub(crate) fn router_with_registry(
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
    pub(crate) async fn bootstrap_default(
        state: &AppState,
    ) -> (SessionId, WindowId, PaneId, ActivityId) {
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
    use ozmux_multiplexer::WindowId;

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
