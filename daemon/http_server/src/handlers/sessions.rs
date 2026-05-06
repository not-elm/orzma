use crate::error::HttpResult;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use ozmux_session::{Session, SessionId, SessionState, Window, WindowId, WindowStore};
use serde::{Deserialize, Serialize};

// pub mod pane;     // TODO: restore in Task 20 (migrated under windows/panes)
// pub mod windows;  // TODO: restore in Task 19

/// Build the Session+Windows JSON used by every successful session-returning
/// handler. Inlines all owned windows.
pub(super) async fn session_with_windows_json(
    sessions: &SessionState,
    windows: &WindowStore,
    session_id: &SessionId,
) -> HttpResult<Json<serde_json::Value>> {
    let session = sessions.session(session_id).await?;
    let window_ids: Vec<WindowId> = session.windows().to_vec();
    let session_value = serde_json::to_value(&*session).expect("Session is Serialize");
    drop(session);

    let store = windows.lock().await;
    let window_values: Vec<serde_json::Value> = window_ids
        .iter()
        .map(|wid| {
            store
                .get(wid)
                .map(|w| serde_json::to_value(w).expect("Window is Serialize"))
                .unwrap_or(serde_json::Value::Null)
        })
        .collect();
    drop(store);

    let mut value = session_value;
    value["windows"] = serde_json::Value::Array(window_values);
    Ok(Json(value))
}

#[derive(Deserialize, Default)]
pub struct CreateRequest {
    #[serde(default)]
    name: String,
}

pub async fn create(
    State(sessions): State<SessionState>,
    State(windows): State<WindowStore>,
    Json(body): Json<CreateRequest>,
) -> impl IntoResponse {
    // Build session + default window in canonical lock order.
    let session_id = SessionId::new();
    let window_id = WindowId::new();
    let window = Window::new(window_id.clone(), session_id.clone(), "main".into());
    let session = Session::empty(session_id.clone(), body.name, window_id.clone());

    let mut sess_guard = sessions.lock().await;
    let mut win_guard = windows.lock().await;
    sess_guard.insert(session_id.clone(), session);
    win_guard.insert(window_id, window);
    drop(win_guard);
    drop(sess_guard);

    let body = session_with_windows_json(&sessions, &windows, &session_id)
        .await
        .expect("just inserted");
    (StatusCode::CREATED, body)
}

#[derive(Serialize)]
struct SessionSummary<'a> {
    id: &'a SessionId,
    name: &'a str,
}

pub async fn list(State(state): State<SessionState>) -> Json<serde_json::Value> {
    let guard = state.lock().await;
    let summaries: Vec<SessionSummary> = guard
        .iter()
        .map(|(id, s)| SessionSummary { id, name: s.name() })
        .collect();
    Json(serde_json::json!({ "sessions": summaries }))
}

pub async fn get_session(
    State(sessions): State<SessionState>,
    State(windows): State<WindowStore>,
    Path(session_id): Path<SessionId>,
) -> HttpResult<Json<serde_json::Value>> {
    session_with_windows_json(&sessions, &windows, &session_id).await
}

#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

pub async fn rename(
    State(sessions): State<SessionState>,
    State(windows): State<WindowStore>,
    Path(session_id): Path<SessionId>,
    Json(req): Json<RenameRequest>,
) -> HttpResult<Json<serde_json::Value>> {
    {
        let mut session = sessions.session_mut(&session_id).await?;
        session.rename(req.name);
    }
    session_with_windows_json(&sessions, &windows, &session_id).await
}

pub async fn delete(
    State(sessions): State<SessionState>,
    State(windows): State<WindowStore>,
    State(terminal): State<ozmux_terminal::TerminalService>,
    Path(session_id): Path<SessionId>,
) -> HttpResult<StatusCode> {
    let svc = ozmux_session::WindowService::new(sessions, windows);
    let activity_ids = svc.cascade_delete_session(session_id).await?;
    for aid in activity_ids {
        let _ = terminal.kill(&aid).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_session::WindowStore;
    use tower::ServiceExt;

    use crate::test_helpers::router_with_state as router_with;

    #[tokio::test]
    async fn list_returns_empty_when_no_sessions() {
        let resp = router_with(SessionState::default(), WindowStore::default())
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
    async fn create_with_name_returns_201_and_session_with_one_window() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let resp = router_with(sessions.clone(), windows.clone())
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
        assert_eq!(v["windows"].as_array().map(|a| a.len()), Some(1));
        let w = &v["windows"][0];
        assert!(w["id"].is_string());
        assert_eq!(w["name"].as_str(), Some("main"));
        assert!(w["root"].is_string());
        assert!(w["cells"].is_object());
        assert_eq!(w["panes"].as_array().map(|a| a.len()), Some(1));
        assert!(v["active_window"].is_string());
        assert_eq!(v["active_window"].as_str(), w["id"].as_str());
    }

    #[tokio::test]
    async fn get_returns_session_with_inlined_windows() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, _wid, _pid, _aid) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
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
        assert_eq!(v["windows"].as_array().map(|a| a.len()), Some(1));
    }

    #[tokio::test]
    async fn get_returns_404_with_session_not_found_for_unknown_id() {
        let resp = router_with(SessionState::default(), WindowStore::default())
            .oneshot(
                Request::builder()
                    .uri("/sessions/does-not-exist")
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
    async fn delete_returns_204_and_removes_session_and_its_windows() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, wid, _pid, _aid) = sessions.bootstrap_default(&windows).await;

        let resp = router_with(sessions.clone(), windows.clone())
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

        assert!(sessions.lock().await.get(&sid).is_none());
        assert!(windows.lock().await.get(&wid).is_none());
    }

    #[tokio::test]
    async fn rename_updates_name_and_returns_session() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, _wid, _pid, _aid) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/sessions/{}", sid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"new"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["name"].as_str(), Some("new"));
    }

    #[tokio::test]
    async fn rename_unknown_id_returns_404() {
        let resp = router_with(SessionState::default(), WindowStore::default())
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
    async fn delete_unknown_id_returns_404() {
        let resp = router_with(SessionState::default(), WindowStore::default())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/nope")
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
}
