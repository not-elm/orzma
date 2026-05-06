use crate::error::HttpResult;
use crate::handlers::sessions::session_with_windows_json;
use axum::{
    Json,
    extract::{Path, State},
};
use ozmux_session::activity::ActivityId;
use ozmux_session::cell::{Side, SplitOrientation};
use ozmux_session::pane::PaneId;
use ozmux_session::{SessionId, SessionState, WindowId, WindowService, WindowStore};
use ozmux_terminal::TerminalService;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SplitRequest {
    orientation: SplitOrientation,
    #[serde(default)]
    side: Side,
}

pub async fn split(
    State(svc): State<WindowService>,
    State(terminal): State<TerminalService>,
    State(sessions): State<SessionState>,
    State(windows): State<WindowStore>,
    Path((session_id, window_id, pane_id)): Path<(SessionId, WindowId, PaneId)>,
    Json(req): Json<SplitRequest>,
) -> HttpResult<Json<serde_json::Value>> {
    let new_pane_id = PaneId::new();
    let new_activity_id = ActivityId::new();

    // 1. Mutate state (pure memory operation, deterministic).
    svc.split_pane(
        session_id.clone(),
        window_id.clone(),
        pane_id,
        new_pane_id.clone(),
        req.orientation,
        req.side,
    )
    .await?;

    // 2. Spawn PTY. On failure, roll back by closing the just-created pane.
    if let Err(spawn_err) = terminal
        .spawn(
            new_activity_id,
            new_pane_id.clone(),
            window_id.clone(),
            session_id.clone(),
            ozmux_terminal::SpawnOptions {
                cols: 80,
                rows: 24,
                shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                cwd: None,
            },
        )
        .await
    {
        let _ = svc
            .close_pane(session_id.clone(), window_id.clone(), new_pane_id.clone())
            .await;
        return Err(spawn_err.into());
    }

    let body = session_with_windows_json(&sessions, &windows, &session_id).await?;
    let mut value = body.0;
    value["new_pane_id"] = serde_json::Value::String(new_pane_id.to_string());
    Ok(Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::test_helpers::router_with_state as router_with;

    #[tokio::test]
    async fn split_horizontal_returns_session_with_new_pane() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, wid, pid, _) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/sessions/{}/windows/{}/panes/{}/split",
                        sid, wid, pid
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["new_pane_id"].is_string());
        // Session JSON has the window with 2 panes now.
        let panes = v["windows"][0]["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 2);
    }

    #[tokio::test]
    async fn split_with_unknown_session_returns_404() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/sessions/{}/windows/{}/panes/{}/split",
                        SessionId::new(),
                        WindowId::new(),
                        PaneId::new()
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal"}"#))
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
