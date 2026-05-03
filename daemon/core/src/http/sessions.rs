use crate::session::{Session, SessionId, SessionState};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};

pub fn router() -> Router<SessionState> {
    Router::new()
        .route("/sessions", get(list).post(create))
        .route("/sessions/{session_id}", get(get_session))
}

#[derive(Deserialize, Default)]
struct CreateRequest {
    #[serde(default)]
    name: String,
}

async fn create(
    State(state): State<SessionState>,
    Json(body): Json<CreateRequest>,
) -> impl IntoResponse {
    let session = Session::new(body.name);
    let id = session.id().clone();
    let mut guard = state.lock().await;
    guard.insert(id.clone(), session);
    let session_ref = guard.get(&id).expect("just inserted");
    (StatusCode::CREATED, Json(serde_json::to_value(session_ref).unwrap()))
}

#[derive(Serialize)]
struct SessionSummary<'a> {
    id: &'a SessionId,
    name: &'a str,
}

async fn list(State(state): State<SessionState>) -> Json<serde_json::Value> {
    let guard = state.lock().await;
    let summaries: Vec<SessionSummary> = guard
        .iter()
        .map(|(id, s)| SessionSummary { id, name: s.name() })
        .collect();
    Json(serde_json::json!({ "sessions": summaries }))
}

async fn get_session(
    State(state): State<SessionState>,
    Path(session_id): Path<SessionId>,
) -> Result<Json<serde_json::Value>, crate::error::OzmuxError> {
    let guard = state.lock().await;
    let session = guard
        .get(&session_id)
        .ok_or_else(|| crate::error::OzmuxError::SessionNotFound(session_id.clone()))?;
    Ok(Json(serde_json::to_value(session).unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn router_with(state: SessionState) -> axum::Router {
        crate::http::test_helpers::daemon_router_for_test(state)
    }

    #[tokio::test]
    async fn list_returns_empty_when_no_sessions() {
        let state = SessionState::default();
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .uri("/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["sessions"].as_array().map(|a| a.len()), Some(0));
    }

    #[tokio::test]
    async fn create_with_name_returns_201_and_full_session() {
        let state = SessionState::default();
        let resp = router_with(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"my session"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["id"].is_string());
        assert_eq!(v["name"].as_str(), Some("my session"));
        assert!(v["root"].is_string());
        assert!(v["cells"].is_object());
        assert_eq!(v["panes"].as_array().map(|a| a.len()), Some(1));

        // The session is actually persisted in state.
        let id = v["id"].as_str().unwrap();
        let guard = state.lock().await;
        assert!(guard.keys().any(|k| k.as_ref() == id));
    }

    #[tokio::test]
    async fn create_without_name_defaults_to_empty_string() {
        let state = SessionState::default();
        let resp = router_with(state)
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
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["name"].as_str(), Some(""));
    }

    #[tokio::test]
    async fn get_returns_full_session() {
        let state = SessionState::default();
        let session = Session::new("xyz".to_string());
        let id = session.id().clone();
        {
            let mut guard = state.lock().await;
            guard.insert(id.clone(), session);
        }
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .uri(format!("/sessions/{}", id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"].as_str(), Some(id.as_ref()));
        assert_eq!(v["name"].as_str(), Some("xyz"));
    }

    #[tokio::test]
    async fn get_returns_404_with_session_not_found_code_for_unknown_id() {
        let state = SessionState::default();
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .uri("/sessions/no-such-id")
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
    async fn list_returns_summaries_for_each_session() {
        let state = SessionState::default();
        let s1 = Session::new("a".to_string());
        let s2 = Session::new("b".to_string());
        {
            let mut guard = state.lock().await;
            guard.insert(s1.id().clone(), s1);
            guard.insert(s2.id().clone(), s2);
        }
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .uri("/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = v["sessions"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let names: std::collections::HashSet<_> =
            arr.iter().map(|s| s["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains("a"));
        assert!(names.contains("b"));
        // Each summary has id + name only.
        for s in arr {
            assert!(s["id"].is_string());
            assert!(s["name"].is_string());
            assert_eq!(s.as_object().unwrap().len(), 2);
        }
    }
}
