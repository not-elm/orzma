use crate::error::OzmuxResult;
use crate::session::cell::{Side, SplitOrientation};
use crate::session::pane::PaneId;
use crate::session::{SessionId, SessionState};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SplitRequest {
    orientation: SplitOrientation,
    #[serde(default)]
    side: Side,
}

pub async fn split(
    State(state): State<SessionState>,
    Path((session_id, pane_id)): Path<(SessionId, PaneId)>,
    Json(req): Json<SplitRequest>,
) -> OzmuxResult<Json<serde_json::Value>> {
    let mut session = state.session_mut(&session_id).await?;
    let new_pane_id = session.split_pane(&pane_id, req.orientation, req.side)?;
    Ok(Json(serde_json::json!({
        "new_pane_id": new_pane_id,
        "session": &*session,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn router_with(state: SessionState) -> axum::Router {
        let app_state = crate::http::AppState {
            sessions: state,
            terminal: crate::pty::TerminalService::default(),
        };
        crate::http::test_helpers::daemon_router_for_test(app_state)
    }

    #[tokio::test]
    async fn split_horizontal_returns_new_pane_id_and_full_session() {
        let state = SessionState::default();
        let session = Session::new(String::new());
        let pane_id = session.panes().any_pane_id().unwrap();
        let session_id = session.id().clone();
        {
            let mut guard = state.lock().await;
            guard.insert(session_id.clone(), session);
        }
        let resp = router_with(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{}/panes/{}/split", session_id, pane_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let new_id = v["new_pane_id"].as_str().expect("new_pane_id present");
        assert_ne!(new_id, pane_id.as_ref());
        assert_eq!(v["session"]["panes"].as_array().unwrap().len(), 2);
        // wire format: orientation lowercase
        let cells = v["session"]["cells"].as_object().unwrap();
        let has_h = cells.values().any(|c| {
            c["cell"]
                .get("Split")
                .and_then(|s| s.get("orientation"))
                .and_then(|o| o.as_str())
                == Some("horizontal")
        });
        assert!(has_h);
    }

    #[tokio::test]
    async fn split_with_side_before_uses_before() {
        let state = SessionState::default();
        let session = Session::new(String::new());
        let pane_id = session.panes().any_pane_id().unwrap();
        let session_id = session.id().clone();
        {
            let mut guard = state.lock().await;
            guard.insert(session_id.clone(), session);
        }
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{}/panes/{}/split", session_id, pane_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"vertical","side":"before"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn split_with_unknown_session_returns_404() {
        let state = SessionState::default();
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/no/panes/no/split")
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

    #[tokio::test]
    async fn split_with_unknown_pane_returns_404() {
        let state = SessionState::default();
        let session = Session::new(String::new());
        let session_id = session.id().clone();
        {
            let mut guard = state.lock().await;
            guard.insert(session_id.clone(), session);
        }
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/sessions/{}/panes/{}/split",
                        session_id,
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
        assert_eq!(v["error"]["code"].as_str(), Some("PANE_NOT_FOUND"));
    }

    #[tokio::test]
    async fn split_with_invalid_body_returns_400_or_422() {
        let state = SessionState::default();
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/sessions/anything/panes/anything/split")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        // axum's Json extractor returns 422 on missing required fields
        // and 400 on malformed JSON. Either is acceptable.
        let s = resp.status();
        assert!(s == StatusCode::UNPROCESSABLE_ENTITY || s == StatusCode::BAD_REQUEST);
    }
}
