use crate::error::HttpResult;
use axum::{
    Json,
    extract::{Path, State},
};
use ozmux_multiplexer::{MultiplexerService, SessionId};

pub async fn get(
    State(multiplexer): State<MultiplexerService>,
    Path(session_id): Path<SessionId>,
) -> HttpResult<Json<serde_json::Value>> {
    let session_state = multiplexer.sessions.lock().await;
    let session = session_state.get(&session_id)?;
    Ok(Json(super::session_view(&session_id, session)))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn get_returns_session_view() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(Some("named".into())).await;
        let (wid, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/sessions/{}", sid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"].as_str(), Some(sid.as_ref()));
        assert_eq!(v["name"].as_str(), Some("named"));
        assert_eq!(v["windows"][0].as_str(), Some(wid.as_ref()));
        assert_eq!(v["active_window"].as_str(), Some(wid.as_ref()));
    }

    #[tokio::test]
    async fn get_unknown_session_returns_404() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/sessions/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("SESSION_NOT_FOUND"));
    }

    #[tokio::test]
    async fn get_session_with_no_windows_serializes_active_window_null() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/sessions/{}", sid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["active_window"].is_null());
        assert_eq!(v["windows"].as_array().map(|a| a.len()), Some(0));
    }
}
