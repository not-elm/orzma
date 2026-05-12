use crate::{AppState, error::HttpResult};
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::WindowId;

pub async fn select(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<StatusCode> {
    state.select_active_window(&window_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn select_returns_204_and_updates_active_window() {
        let state = fresh_state();
        let sid = state.create_session(None).await;
        let (_wid_a, _, _) = state.create_window(Some(&sid), None).await.unwrap();
        let (wid_b, _, _) = state.create_window(Some(&sid), None).await.unwrap();
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/select", wid_b))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let sess = state.sessions.lock().await;
        assert_eq!(sess.get(&sid).unwrap().active_window.as_ref(), Some(&wid_b));
    }

    #[tokio::test]
    async fn select_orphan_window_returns_409_window_not_attached() {
        let state = fresh_state();
        let (wid, _, _) = state.create_window(None, None).await.unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/select", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("WINDOW_NOT_ATTACHED"));
    }
}
