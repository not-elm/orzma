use crate::{
    error::{OzmuxError, OzmuxResult},
    session::SessionState,
};
use axum::{
    Router,
    routing::{delete as method_delete, get, post},
};
use tokio::net::TcpListener;

mod health;
mod index;
mod sessions;

pub async fn launch_server() -> OzmuxResult {
    let state = SessionState::default();
    let listener = TcpListener::bind("127.0.0.1:3200")
        .await
        .map_err(|e| OzmuxError::FailedLaunchHttpServer(e.to_string()))?;
    axum::serve(listener, daemon_router(state))
        .await
        .map_err(|e| OzmuxError::FailedLaunchHttpServer(e.to_string()))?;
    Ok(())
}

fn daemon_router(state: SessionState) -> Router {
    Router::new()
        .route("/", get(index::handler))
        .route("/health", get(health::check))
        .route("/sessions", get(sessions::list).post(sessions::create))
        .route(
            "/sessions/{session_id}",
            get(sessions::get_session)
                .patch(sessions::rename)
                .delete(sessions::delete),
        )
        .route(
            "/sessions/{session_id}/panes/{pane_id}",
            method_delete(sessions::pane::close),
        )
        .route(
            "/sessions/{session_id}/panes/{pane_id}/split",
            post(sessions::pane::split::split),
        )
        .with_state(state)
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::{SessionState, daemon_router};
    use axum::Router;
    pub fn daemon_router_for_test(state: SessionState) -> Router {
        daemon_router(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn unknown_session_route_returns_404() {
        let resp = daemon_router(SessionState::default())
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
