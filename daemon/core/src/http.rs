use crate::{
    error::{OzmuxError, OzmuxResult},
    session::SessionState,
};
use axum::{Router, routing::get};
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
        .merge(sessions::router())
        .with_state(state)
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
