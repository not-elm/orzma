pub mod split;

use crate::error::HttpResult;
use crate::handlers::sessions::session_with_windows_json;
use axum::{
    Json,
    extract::{Path, State},
};
use ozmux_session::pane::PaneId;
use ozmux_session::{SessionId, SessionState, WindowId, WindowService, WindowStore};
use ozmux_terminal::TerminalService;

pub async fn close(
    State(svc): State<WindowService>,
    State(terminal): State<TerminalService>,
    State(sessions): State<SessionState>,
    State(windows): State<WindowStore>,
    Path((session_id, window_id, pane_id)): Path<(SessionId, WindowId, PaneId)>,
) -> HttpResult<Json<serde_json::Value>> {
    if let Some(aid) = svc
        .close_pane(session_id.clone(), window_id, pane_id)
        .await?
    {
        let _ = terminal.kill(&aid).await;
    }
    session_with_windows_json(&sessions, &windows, &session_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_session::cell::{Side, SplitOrientation};
    use tower::ServiceExt;

    use crate::test_helpers::router_with_state as router_with;

    #[tokio::test]
    async fn close_returns_updated_session() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, wid, pid_first, _aid) = sessions.bootstrap_default(&windows).await;
        // Split to have 2 panes.
        let svc = WindowService::new(sessions.clone(), windows.clone());
        let new_pane_id = PaneId::new();
        svc.split_pane(
            sid.clone(),
            wid.clone(),
            pid_first,
            new_pane_id.clone(),
            SplitOrientation::Horizontal,
            Side::After,
        )
        .await
        .expect("split");

        let resp = router_with(sessions.clone(), windows.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!(
                        "/sessions/{}/windows/{}/panes/{}",
                        sid, wid, new_pane_id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Body has the updated session with 1 pane in the window.
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let panes = v["windows"][0]["panes"].as_array().expect("panes array");
        assert_eq!(panes.len(), 1, "window should have 1 pane after close");

        // In-memory state confirms removal.
        let store = windows.lock().await;
        let window = store.get(&wid).expect("window exists");
        assert!(
            window.panes().get(&new_pane_id).is_err(),
            "new_pane_id should be gone from PaneStore"
        );
    }

    #[tokio::test]
    async fn close_last_pane_in_window_returns_409() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let (sid, wid, pid, _) = sessions.bootstrap_default(&windows).await;
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/sessions/{}/windows/{}/panes/{}", sid, wid, pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("CANNOT_CLOSE_LAST_PANE"));
    }

    #[tokio::test]
    async fn close_with_unknown_session_returns_404() {
        let sessions = SessionState::default();
        let windows = WindowStore::default();
        let resp = router_with(sessions, windows)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!(
                        "/sessions/{}/windows/{}/panes/{}",
                        SessionId::new(),
                        WindowId::new(),
                        PaneId::new()
                    ))
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
