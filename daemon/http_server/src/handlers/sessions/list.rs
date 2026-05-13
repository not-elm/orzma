use axum::{Json, extract::State};
use ozmux_multiplexer::{MultiplexerService, Session};

pub async fn list(State(multiplexer): State<MultiplexerService>) -> Json<serde_json::Value> {
    let sess = multiplexer.sessions.lock().await;
    let mut sessions: Vec<&Session> = sess.iter().map(|(_, s)| s).collect();
    sessions.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));
    Json(serde_json::json!({ "sessions": sessions }))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn list_returns_empty_when_no_sessions() {
        let (router, _) = router_with(fresh_state());
        let resp = router
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
    async fn list_returns_sessions_sorted_by_id() {
        let state = fresh_state();
        let sid_a = state.multiplexer.create_session(Some("a".into())).await;
        let sid_b = state.multiplexer.create_session(Some("b".into())).await;
        let mut expected = [sid_a.to_string(), sid_b.to_string()];
        expected.sort();

        let (router, _) = router_with(state);
        let resp = router
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
        let ids: Vec<String> = v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, expected.to_vec());
    }

    #[tokio::test]
    async fn list_includes_full_session_view() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(Some("test".into())).await;
        let (wid, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let (router, _) = router_with(state);
        let resp = router
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
        let s = &v["sessions"][0];
        assert_eq!(s["id"].as_str(), Some(sid.as_ref()));
        assert_eq!(s["name"].as_str(), Some("test"));
        assert_eq!(s["linkedWindows"][0].as_str(), Some(wid.as_ref()));
        assert_eq!(s["active_window"].as_str(), Some(wid.as_ref()));
    }
}
