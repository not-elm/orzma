use crate::error::HttpResult;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{MultiplexerService, SessionError, session::SessionId};
use ozmux_terminal::TerminalService;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Deserialize, Default)]
pub struct CreateRequest {
    #[serde(default)]
    name: Option<String>,
}

pub async fn create(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    Json(body): Json<CreateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let id = ms.lock().await.new_session(body.name);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": id })),
    )
}

#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

pub async fn rename(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    Path(session_id): Path<SessionId>,
    Json(body): Json<RenameRequest>,
) -> HttpResult<StatusCode> {
    ms.lock().await.rename_session(&session_id, body.name)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    State(terminal): State<TerminalService>,
    Path(session_id): Path<SessionId>,
) -> HttpResult<StatusCode> {
    let activities = ms.lock().await.delete_session(&session_id)?;
    for aid in activities {
        let _ = terminal.kill(&aid).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
) -> Json<serde_json::Value> {
    let ms = ms.lock().await;
    let mut entries: Vec<(&SessionId, &ozmux_multiplexer::session::Session)> =
        ms.sessions().iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.as_ref().cmp(b.as_ref()));
    let sessions: Vec<serde_json::Value> = entries
        .iter()
        .map(|(id, session)| session_view(id, session))
        .collect();
    Json(serde_json::json!({ "sessions": sessions }))
}

fn session_view(
    id: &SessionId,
    session: &ozmux_multiplexer::session::Session,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": session.name,
        "windows": session.windows,
        "active_window": session.active_window,
    })
}

pub async fn get(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    Path(session_id): Path<SessionId>,
) -> HttpResult<Json<serde_json::Value>> {
    let ms = ms.lock().await;
    let session = ms
        .sessions()
        .get(&session_id)
        .ok_or_else(|| SessionError::SessionNotFound(session_id.clone()))?;
    Ok(Json(session_view(&session_id, session)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::router_with;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn create_returns_201_with_id() {
        let (router, _) = router_with(MultiplexerService::default());
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
        let (router, _) = router_with(MultiplexerService::default());
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
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let (router, state) = router_with(ms);
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
        let ms = state.multiplexer.lock().await;
        assert_eq!(ms.sessions().get(&sid).unwrap().name, "renamed");
    }

    #[tokio::test]
    async fn rename_unknown_session_returns_404() {
        let (router, _) = router_with(MultiplexerService::default());
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
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let _wid = ms.new_window_in(Some(&sid), None).unwrap();
        let (router, state) = router_with(ms);
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
        let ms = state.multiplexer.lock().await;
        assert!(ms.sessions().get(&sid).is_none());
    }

    #[tokio::test]
    async fn delete_unknown_session_returns_404() {
        let (router, _) = router_with(MultiplexerService::default());
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
        let (router, _) = router_with(MultiplexerService::default());
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
        let mut ms = MultiplexerService::default();
        let sid_a = ms.new_session(Some("a".into()));
        let sid_b = ms.new_session(Some("b".into()));
        let mut expected = [sid_a.to_string(), sid_b.to_string()];
        expected.sort();

        let (router, _) = router_with(ms);
        let resp = router
            .oneshot(Request::builder().uri("/sessions").body(Body::empty()).unwrap())
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
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(Some("test".into()));
        let wid = ms.new_window_in(Some(&sid), None).unwrap();
        let (router, _) = router_with(ms);
        let resp = router
            .oneshot(Request::builder().uri("/sessions").body(Body::empty()).unwrap())
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
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(Some("named".into()));
        let wid = ms.new_window_in(Some(&sid), None).unwrap();
        let (router, _) = router_with(ms);
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
        let (router, _) = router_with(MultiplexerService::default());
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
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let (router, _) = router_with(ms);
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
