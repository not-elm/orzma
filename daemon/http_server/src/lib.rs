pub mod error;
pub mod handlers;

pub use error::{HttpError, HttpResult};

use axum::{
    Router,
    extract::FromRef,
    routing::get,
};
use ozmux_session::{SessionState, WindowService, WindowStore};
use ozmux_terminal::TerminalService;
use tokio::net::TcpListener;

#[derive(Clone, Default)]
pub struct AppState {
    pub sessions: SessionState,
    pub windows: WindowStore,
    pub terminal: TerminalService,
}

impl FromRef<AppState> for SessionState {
    fn from_ref(input: &AppState) -> Self {
        input.sessions.clone()
    }
}

impl FromRef<AppState> for WindowStore {
    fn from_ref(input: &AppState) -> Self {
        input.windows.clone()
    }
}

impl FromRef<AppState> for WindowService {
    fn from_ref(input: &AppState) -> Self {
        WindowService::new(input.sessions.clone(), input.windows.clone())
    }
}

impl FromRef<AppState> for TerminalService {
    fn from_ref(input: &AppState) -> Self {
        input.terminal.clone()
    }
}

pub async fn serve(state: AppState) -> HttpResult {
    let (sid, wid, pid, aid) = state.sessions.bootstrap_default(&state.windows).await;
    if let Err(e) = state
        .terminal
        .spawn(
            aid,
            pid,
            wid,
            sid,
            ozmux_terminal::SpawnOptions {
                cols: 80,
                rows: 24,
                shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                cwd: None,
            },
        )
        .await
    {
        // Bootstrap PTY spawn failure must not yield a half-initialized server.
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
        .nest("/sessions", sessions_router())
        .nest("/activities", activities_router())
        .with_state(state)
}

fn sessions_router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(handlers::sessions::list).post(handlers::sessions::create),
        )
        .nest("/{session_id}", session_id_router())
}

fn session_id_router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(handlers::sessions::get_session)
                .patch(handlers::sessions::rename)
                .delete(handlers::sessions::delete),
        )
        .nest("/panes/{pane_id}", pane_id_router())
}

fn pane_id_router() -> Router<AppState> {
    // TODO: restore in Task 20 (pane handlers migrated to windows/panes)
    Router::new()
}

fn activities_router() -> Router<AppState> {
    Router::new().route(
        "/{activity_id}/terminal/ws",
        get(handlers::activities::terminal_ws),
    )
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::{AppState, daemon_router};
    use axum::Router;
    use ozmux_session::{SessionState, WindowStore};
    use ozmux_terminal::TerminalService;

    pub fn daemon_router_for_test(state: AppState) -> Router {
        daemon_router(state)
    }

    /// Build a router from a `SessionState` + `WindowStore`, supplying a default
    /// `TerminalService`. Used by handler tests that don't exercise the WS endpoint.
    pub fn router_with_state(sessions: SessionState, windows: WindowStore) -> Router {
        daemon_router(AppState {
            sessions,
            windows,
            terminal: TerminalService::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn unknown_session_route_returns_404() {
        let resp = daemon_router(AppState::default())
            .oneshot(
                Request::builder()
                    .uri("/sessions/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
