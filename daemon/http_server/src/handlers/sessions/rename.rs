use crate::error::HttpResult;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{MultiplexerService, SessionId};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

pub async fn rename(
    State(multiplexer): State<MultiplexerService>,
    Path(session_id): Path<SessionId>,
    Json(body): Json<RenameRequest>,
) -> HttpResult<StatusCode> {
    multiplexer.rename_session(&session_id, body.name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn rename_returns_204_and_updates_name() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/sessions/{}", sid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let sess = state.multiplexer.sessions.lock().await;
        assert_eq!(sess.get(&sid).unwrap().name, "renamed");
    }

    #[tokio::test]
    async fn rename_unknown_session_returns_404() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/sessions/missing")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("SESSION_NOT_FOUND"));
    }
}
