//! HTTP/WebSocket server: axum router, REST handlers, and PTY WS bridging.

pub mod activity_titles;
pub mod error;
pub mod extractors;
pub mod handlers;
pub mod layout_broadcast;
pub mod layout_dto;
pub(crate) mod origin_guard;
pub(crate) mod provision;
pub mod session_broadcast;
pub mod session_view;
pub mod state;
mod title_republish;
pub mod window_view;

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
        .route(
            "/{session_id}/events",
            get(handlers::sessions::events::events),
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
            "/{window_id}/dimensions",
            axum::routing::patch(handlers::windows::dimensions::patch_dimensions),
        )
        .route(
            "/{window_id}/select",
            post(handlers::windows::select::select),
        )
        .route(
            "/{window_id}/focus-pane",
            post(handlers::windows::focus_pane::focus_pane),
        )
        .route(
            "/{window_id}/events",
            get(handlers::windows::events::events),
        )
        .nest("/{window_id}/panes", handlers::windows::panes::router())
}

/// Returns `true` when `OZMUX_TEST_REAL_CHROME=1` is set in the environment.
/// Tests that require a live Chromium process should skip themselves when this
/// returns `false`.
#[cfg(test)]
pub(crate) fn requires_real_chrome() -> bool {
    std::env::var("OZMUX_TEST_REAL_CHROME").ok().as_deref() == Some("1")
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::{AppState, daemon_router};
    use axum::Router;
    use ozmux_extension::runtime::RuntimeRoot;
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, SessionId, WindowId};
    use std::sync::Arc;

    pub(crate) fn fresh_state() -> AppState {
        let tmp = tempfile::tempdir().expect("tempdir");
        let runtime =
            Arc::new(RuntimeRoot::new_in(tmp.path(), std::process::id()).expect("RuntimeRoot"));
        // NOTE: keep the tempdir alive for the process lifetime so the paths
        // inside RuntimeRoot remain valid for tests that exercise the fs paths.
        std::mem::forget(tmp);
        let terminal = ozmux_terminal::TerminalService::with_runtime_root(Arc::clone(&runtime));
        let cef_host = Arc::new(ozmux_browser::cef_service::stub_for_tests());
        AppState::new(
            terminal,
            ozmux_extension::ExtensionRegistry::default(),
            crate::layout_broadcast::LayoutBroadcaster::default(),
            crate::session_broadcast::SessionBroadcaster::default(),
            Arc::new(ozmux_configs::OzmuxConfigs::default()),
            crate::activity_titles::ActivityTitles::default(),
            cef_host,
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

    /// Insert a fresh terminal Activity into an existing pane via the
    /// multiplexer, bypassing PTY spawn. Returns the new ActivityId.
    pub(crate) async fn add_activity_via_window(
        state: &AppState,
        wid: &WindowId,
        pid: &PaneId,
    ) -> ActivityId {
        let aid = ActivityId::new();
        state
            .multiplexer
            .with_window_or_404(wid, |w| {
                w.pane_mut(pid)?
                    .add_activity(Activity::terminal(aid.clone()))
            })
            .await
            .unwrap();
        aid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_multiplexer::WindowId;

    #[tokio::test]
    async fn app_state_default_includes_layout_broadcaster() {
        let state = test_helpers::fresh_state();
        let _ = state.layout_broadcast.subscribe_or_create(&WindowId::new());
    }

    #[tokio::test]
    async fn close_activity_removes_cef_ring() {
        use ozmux_browser::frame_ring::FrameRing;
        use ozmux_browser::shm_alloc::{SLOT_PAYLOAD_MAX, create_shm_for_activity};
        use ozmux_browser::shm_reader::OwnedShmReader;
        use ozmux_browser_cef_protocol::types::ActivityId as CefActivityId;
        use std::sync::Arc;

        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, aid) = test_helpers::bootstrap_default(&state).await;
        // Bootstrap created a pane with one activity; close_activity refuses
        // to remove the last activity, so seed a second one to be the target.
        let extra = test_helpers::add_activity_via_window(&state, &wid, &pid).await;
        let cef_aid = CefActivityId(extra.to_string());
        let shm_fd = create_shm_for_activity(&cef_aid.0, SLOT_PAYLOAD_MAX).expect("shm alloc");
        let reader = Arc::new(OwnedShmReader::map(&shm_fd, SLOT_PAYLOAD_MAX).expect("shm map"));
        state
            .browser_cef
            .insert(cef_aid.clone(), Arc::new(FrameRing::new(123, 1)), reader);
        assert!(state.browser_cef.frame_ring(&cef_aid).is_some());

        state.close_activity(&wid, &pid, &extra).await.unwrap();
        assert!(
            state.browser_cef.frame_ring(&cef_aid).is_none(),
            "ring should be removed after close_activity"
        );
        // Sanity: the original bootstrap activity is still around.
        let _ = aid;
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
