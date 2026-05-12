use crate::{AppState, error::HttpResult};
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::SessionId;

pub async fn delete(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> HttpResult<StatusCode> {
    state.delete_session(&session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn delete_returns_204_and_removes_session() {
        let state = fresh_state();
        let sid = state.create_session(None).await;
        let _ = state.create_window(Some(&sid), None).await.unwrap();
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/sessions/{}", sid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let sess = state.sessions.lock().await;
        assert!(sess.get(&sid).is_err());
    }

    #[tokio::test]
    async fn delete_unknown_session_returns_404() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
