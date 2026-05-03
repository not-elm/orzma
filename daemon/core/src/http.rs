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
    let listener = TcpListener::bind("127.0.0.1:3200")
        .await
        .map_err(|e| OzmuxError::FailedLaunchHttpServer(e.to_string()))?;
    axum::serve(listener, daemon_router())
        .await
        .map_err(|e| OzmuxError::FailedLaunchHttpServer(e.to_string()))?;
    Ok(())
}

fn daemon_router() -> Router {
    let state = SessionState::default();
    Router::new()
        .route("/", get(index::handler))
        .route("/health", get(health::check))
        .with_state(state)
}
