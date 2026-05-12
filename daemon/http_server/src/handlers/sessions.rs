use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::SessionId;
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct CreateRequest {
    #[serde(default)]
    name: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let id = state.create_session(body.name).await;
    (StatusCode::CREATED, Json(serde_json::json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

pub async fn rename(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    Json(body): Json<RenameRequest>,
) -> HttpResult<StatusCode> {
    state.rename_session(&session_id, body.name).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> HttpResult<StatusCode> {
    state.delete_session(&session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list(State(state): State<AppState>) -> Json<serde_json::Value> {
    let sess = state.sessions.lock().await;
    let mut entries: Vec<(&SessionId, &ozmux_multiplexer::Session)> = sess.iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.as_ref().cmp(b.as_ref()));
    let sessions: Vec<serde_json::Value> = entries
        .iter()
        .map(|(id, session)| session_view(id, session))
        .collect();
    Json(serde_json::json!({ "sessions": sessions }))
}

fn session_view(id: &SessionId, session: &ozmux_multiplexer::Session) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": session.name,
        "windows": session.linked_windows,
        "active_window": session.active_window,
    })
}

pub async fn get(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
) -> HttpResult<Json<serde_json::Value>> {
    let sess = state.sessions.lock().await;
    let session = sess.get(&session_id)?;
    Ok(Json(session_view(&session_id, session)))
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
    async fn rename_returns_204_and_updates_name() {
        let state = fresh_state();
        let sid = state.create_session(None).await;
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
        let sess = state.sessions.lock().await;
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
        let sid_a = state.create_session(Some("a".into())).await;
        let sid_b = state.create_session(Some("b".into())).await;
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
        let sid = state.create_session(Some("test".into())).await;
        let (wid, _, _) = state.create_window(Some(&sid), None).await.unwrap();
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
        assert_eq!(s["windows"][0].as_str(), Some(wid.as_ref()));
        assert_eq!(s["active_window"].as_str(), Some(wid.as_ref()));
    }

    #[tokio::test]
    async fn get_returns_session_view() {
        let state = fresh_state();
        let sid = state.create_session(Some("named".into())).await;
        let (wid, _, _) = state.create_window(Some(&sid), None).await.unwrap();
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
        let sid = state.create_session(None).await;
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
