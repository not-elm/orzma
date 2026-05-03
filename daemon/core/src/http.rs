use crate::error::{OzmuxError, OzmuxResult};
use axum::{Router, routing::get};
use tokio::net::TcpListener;

mod health;
mod index;

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
    Router::new()
        .route("/", get(index::handler))
        .route("/health", get(health::check))
}
