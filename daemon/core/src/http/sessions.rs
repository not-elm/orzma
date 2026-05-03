use crate::session::{SessionId, SessionState};
use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;

pub fn router() -> Router<SessionState> {
    Router::new().route("/sessions", get(list))
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
