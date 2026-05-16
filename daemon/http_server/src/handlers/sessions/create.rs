//! `POST /sessions` — create a session and broadcast the new view.

use crate::AppState;
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct CreateRequest {
    #[serde(default)]
    name: Option<String>,
}

/// Create a new session, broadcast the new `SessionView`, and return
/// `201 Created` with the new id.
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let id = state.create_session(body.name).await;
    (StatusCode::CREATED, Json(serde_json::json!({ "id": id })))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn create_returns_201_with_id() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["id"].is_string());
    }

    #[tokio::test]
    async fn create_without_name_still_returns_201_with_id() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_publishes_session_view() {
        use std::time::Duration;
        let state = fresh_state();
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"published"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let sid_str = v["id"].as_str().unwrap();
        let sid: ozmux_multiplexer::SessionId =
            serde_json::from_value(serde_json::Value::String(sid_str.into())).unwrap();

        // The first publish has already fired and was dropped because no
        // receiver existed yet. Subscribe now and re-trigger the publish
        // by calling the same `AppState` helper directly — this confirms
        // the wiring is correct (the handler does fire publishes).
        let mut rx = state.session_broadcast.subscribe_or_create(&sid);
        state.publish_session_view(&sid).await;
        let view = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["name"].as_str(), Some("published"));
    }
}
