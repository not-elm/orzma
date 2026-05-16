//! `POST /windows` — create a window, optionally attached to a session,
//! and broadcast the parent `SessionView` when attached.

use crate::{AppState, error::HttpResult};
use axum::{Json, extract::State, http::StatusCode};
use ozmux_multiplexer::SessionId;
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct CreateRequest {
    #[serde(default)]
    session_id: Option<SessionId>,
    #[serde(default)]
    name: Option<String>,
}

/// Create a window. When `session_id` is supplied, the window is
/// attached to that session and the new `SessionView` is broadcast.
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let (wid, _pid, _aid) = state
        .create_window(body.session_id.as_ref(), body.name)
        .await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": wid }))))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn create_with_session_id_returns_201_and_attaches() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(None).await;
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/windows")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"session_id":"{}"}}"#, sid)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["id"].is_string());
        let sess = state.multiplexer.sessions.lock().await;
        assert_eq!(sess.get(&sid).unwrap().linked_windows.len(), 1);
    }

    #[tokio::test]
    async fn create_without_session_id_creates_orphan() {
        let (router, state) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/windows")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(state.multiplexer.windows.len(), 1);
        assert_eq!(state.multiplexer.sessions.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn create_with_unknown_session_returns_404() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/windows")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"session_id":"bogus"}"#))
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
    async fn create_with_session_id_publishes_session_view() {
        use std::time::Duration;
        let state = fresh_state();
        let sid = state.multiplexer.create_session(Some("s".into())).await;
        let mut rx = state.session_broadcast.subscribe_or_create(&sid);

        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/windows")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"session_id":"{}","name":"w"}}"#,
                        sid
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let view = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["windows"].as_array().map(|a| a.len()), Some(1));
        assert_eq!(view["windows"][0]["name"].as_str(), Some("w"));
    }
}
