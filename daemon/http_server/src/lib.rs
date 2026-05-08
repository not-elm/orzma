pub mod error;
pub mod handlers;

pub use error::{HttpError, HttpResult};

use axum::{
    Router,
    extract::FromRef,
    routing::{delete as method_delete, get, patch, post},
};
use ozmux_multiplexer::MultiplexerService;
use ozmux_terminal::TerminalService;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct AppState {
    pub multiplexer: Arc<Mutex<MultiplexerService>>,
    pub terminal: TerminalService,
}

impl FromRef<AppState> for Arc<Mutex<MultiplexerService>> {
    fn from_ref(input: &AppState) -> Self {
        input.multiplexer.clone()
    }
}

impl FromRef<AppState> for TerminalService {
    fn from_ref(input: &AppState) -> Self {
        input.terminal.clone()
    }
}

pub async fn serve(state: AppState) -> HttpResult {
    let (_sid, _wid, pid, aid) = {
        let mut ms = state.multiplexer.lock().await;
        ms.bootstrap_default().expect("bootstrap_default cannot fail on empty MultiplexerService")
    };
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
            post(handlers::sessions::create),
        )
        .route(
            "/sessions/{session_id}",
            patch(handlers::sessions::rename).delete(handlers::sessions::delete),
        )
        .route(
            "/windows",
            post(handlers::windows::create),
        )
        .route(
            "/windows/{window_id}",
            patch(handlers::windows::rename).delete(handlers::windows::delete),
        )
        .route(
            "/windows/{window_id}/select",
            post(handlers::windows::select),
        )
        .route(
            "/panes/{pane_id}/split",
            post(handlers::panes::split),
        )
        .route(
            "/panes/{pane_id}",
            method_delete(handlers::panes::close),
        )
        .route(
            "/activities/{activity_id}/terminal/ws",
            get(handlers::activities::terminal_ws),
        )
        .with_state(state)
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::{AppState, daemon_router};
    use axum::Router;
    use ozmux_multiplexer::MultiplexerService;
    use ozmux_terminal::TerminalService;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    pub fn daemon_router_for_test(state: AppState) -> Router {
        daemon_router(state)
    }

    pub fn router_with(ms: MultiplexerService) -> (Router, AppState) {
        let state = AppState {
            multiplexer: Arc::new(Mutex::new(ms)),
            terminal: TerminalService::default(),
        };
        (daemon_router(state.clone()), state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::MultiplexerService;
    use tower::ServiceExt;

    #[tokio::test]
    async fn unknown_pane_route_returns_404() {
        let (router, _) = test_helpers::router_with(MultiplexerService::default());
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
}
