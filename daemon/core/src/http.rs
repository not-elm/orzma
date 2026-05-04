use crate::{
    error::{OzmuxError, OzmuxResult},
    pty::TerminalService,
    session::SessionState,
};
use axum::{
    Router,
    extract::FromRef,
    routing::{delete as method_delete, get, post},
};
use tokio::net::TcpListener;

mod activities;
mod health;
mod index;
mod sessions;

#[derive(Clone, Default)]
pub struct AppState {
    pub sessions: SessionState,
    pub terminal: TerminalService,
}

impl FromRef<AppState> for SessionState {
    fn from_ref(input: &AppState) -> Self {
        input.sessions.clone()
    }
}

impl FromRef<AppState> for TerminalService {
    fn from_ref(input: &AppState) -> Self {
        input.terminal.clone()
    }
}

pub async fn launch_server() -> OzmuxResult {
    let state = AppState::default();

    // Bootstrap: derive default Activity ID, then drop the lock before await on spawn.
    let activity_id = state.sessions.bootstrap_default().await;
    state
        .terminal
        .spawn(
            activity_id,
            crate::pty::SpawnOptions {
                cols: 80,
                rows: 24,
                shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                cwd: None,
            },
        )
        .await?;

    let listener = TcpListener::bind("127.0.0.1:3200")
        .await
        .map_err(|e| OzmuxError::FailedLaunchHttpServer(e.to_string()))?;
    axum::serve(listener, daemon_router(state))
        .await
        .map_err(|e| OzmuxError::FailedLaunchHttpServer(e.to_string()))?;
    Ok(())
}

fn daemon_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index::handler))
        .route("/health", get(health::check))
        .nest("/sessions", sessions_router())
        .nest("/activities", activities_router())
        .with_state(state)
}

fn sessions_router() -> Router<AppState> {
    Router::new()
        .route("/", get(sessions::list).post(sessions::create))
        .nest("/{session_id}", session_id_router())
}

fn session_id_router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(sessions::get_session)
                .patch(sessions::rename)
                .delete(sessions::delete),
        )
        .nest("/panes/{pane_id}", pane_id_router())
}

fn pane_id_router() -> Router<AppState> {
    Router::new()
        .route("/", method_delete(sessions::pane::close))
        .route("/split", post(sessions::pane::split::split))
}

fn activities_router() -> Router<AppState> {
    Router::new().route(
        "/{activity_id}/terminal/ws",
        get(activities::terminal_ws),
    )
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::{AppState, daemon_router};
    use axum::Router;
    pub fn daemon_router_for_test(state: AppState) -> Router {
        daemon_router(state)
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
