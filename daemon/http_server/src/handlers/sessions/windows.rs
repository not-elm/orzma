use crate::error::HttpResult;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use ozmux_session::{
    SessionId, SessionState, Window, WindowId, WindowService, WindowStore,
};
use ozmux_terminal::TerminalService;
use serde::Deserialize;

use super::session_with_windows_json;

#[derive(Deserialize, Default)]
pub struct CreateWindowRequest {
    #[serde(default)]
    name: Option<String>,
}

pub async fn create(
    State(svc): State<WindowService>,
    Path(session_id): Path<SessionId>,
    Json(body): Json<CreateWindowRequest>,
) -> HttpResult<impl IntoResponse> {
    let window: Window = svc.create(session_id, body.name).await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(&window).expect("Window is Serialize")),
    ))
}

pub async fn get_window(
    State(windows): State<WindowStore>,
    State(sessions): State<SessionState>,
    Path((session_id, window_id)): Path<(SessionId, WindowId)>,
) -> HttpResult<Json<serde_json::Value>> {
    // Validate ownership via Session.windows; then read Window from store.
    let session = sessions.session(&session_id).await?;
    if !session.windows().contains(&window_id) {
        return Err(ozmux_session::SessionError::WindowNotFound(window_id).into());
    }
    drop(session);

    let store = windows.lock().await;
    let window = store
        .get(&window_id)
        .ok_or_else(|| ozmux_session::SessionError::WindowNotFound(window_id.clone()))?;
    Ok(Json(serde_json::to_value(window).expect("Window is Serialize")))
}

#[derive(Deserialize)]
pub struct RenameWindowRequest {
    name: String,
}

pub async fn rename(
    State(svc): State<WindowService>,
    Path((session_id, window_id)): Path<(SessionId, WindowId)>,
    Json(req): Json<RenameWindowRequest>,
) -> HttpResult<Json<serde_json::Value>> {
    let window = svc.rename(session_id, window_id, req.name).await?;
    Ok(Json(serde_json::to_value(&window).expect("Window is Serialize")))
}

pub async fn delete(
    State(svc): State<WindowService>,
    State(terminal): State<TerminalService>,
    Path((session_id, window_id)): Path<(SessionId, WindowId)>,
) -> HttpResult<StatusCode> {
    let activity_ids = svc.close(session_id, window_id).await?;
    for aid in activity_ids {
        let _ = terminal.kill(&aid).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn select(
    State(svc): State<WindowService>,
    State(sessions): State<SessionState>,
    State(windows): State<WindowStore>,
    Path((session_id, window_id)): Path<(SessionId, WindowId)>,
) -> HttpResult<Json<serde_json::Value>> {
    svc.select_active(session_id.clone(), window_id).await?;
    session_with_windows_json(&sessions, &windows, &session_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::test_helpers::router_with_state as router_with;

    #[tokio::test]
    async fn create_window_returns_201_with_window_json() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, _, _, _) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions.clone(), windows)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{}/windows", sid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"logs"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["name"].as_str(), Some("logs"));
        assert_eq!(v["session_id"].as_str(), Some(sid.as_ref()));
        assert_eq!(v["panes"].as_array().map(|a| a.len()), Some(1));
    }

    #[tokio::test]
    async fn create_without_name_assigns_window_n() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, _, _, _) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{}/windows", sid))
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Default Window from bootstrap is index 0 ("main"); this new one is the 2nd.
        assert_eq!(v["name"].as_str(), Some("window-2"));
    }

    #[tokio::test]
    async fn get_window_returns_window_json() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, wid, _, _) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .uri(format!("/sessions/{}/windows/{}", sid, wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"].as_str(), Some(wid.as_ref()));
        assert_eq!(v["name"].as_str(), Some("main"));
        assert_eq!(v["session_id"].as_str(), Some(sid.as_ref()));
        assert!(v["panes"].is_array());
    }

    #[tokio::test]
    async fn get_unknown_window_returns_404_window_not_found() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, _, _, _) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .uri(format!("/sessions/{}/windows/{}", sid, WindowId::new()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("WINDOW_NOT_FOUND"));
    }

    #[tokio::test]
    async fn rename_window_returns_renamed_window() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, wid, _, _) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/sessions/{}/windows/{}", sid, wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"build"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["name"].as_str(), Some("build"));
    }

    #[tokio::test]
    async fn delete_only_window_returns_409_cannot_close_last_window() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, wid, _, _) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/sessions/{}/windows/{}", sid, wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("CANNOT_CLOSE_LAST_WINDOW"));
    }

    #[tokio::test]
    async fn select_active_window_updates_session_active_window() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, _wid_default, _, _) = sessions.bootstrap_default(&windows).await;
        // Add a 2nd window directly via the service.
        let svc = WindowService::new(sessions.clone(), windows.clone());
        let new_window = svc
            .create(sid.clone(), Some("logs".into()))
            .await
            .expect("create");

        let resp = router_with(sessions.clone(), windows)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/sessions/{}/windows/{}/select",
                        sid,
                        new_window.id()
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Session.active_window should now be the new window.
        let g = sessions.lock().await;
        assert_eq!(g.get(&sid).unwrap().active_window(), new_window.id());
    }

    #[tokio::test]
    async fn delete_non_last_window_returns_204_and_removes_it() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, _wid_default, _, _) = sessions.bootstrap_default(&windows).await;

        // Create a 2nd window via the service so the session has 2.
        let svc = WindowService::new(sessions.clone(), windows.clone());
        let new_window = svc
            .create(sid.clone(), Some("logs".into()))
            .await
            .expect("create");
        let new_wid = new_window.id().clone();

        let resp = router_with(sessions.clone(), windows.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/sessions/{}/windows/{}", sid, new_wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify the deleted window is gone from WindowStore.
        let store = windows.lock().await;
        assert!(store.get(&new_wid).is_none());
    }
}
