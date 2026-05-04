mod split;

use crate::error::OzmuxResult;
use crate::session::pane::PaneId;
use crate::session::{SessionId, SessionState};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::delete,
};

pub fn router() -> Router<SessionState> {
    Router::new()
        .route("/sessions/{session_id}/panes/{pane_id}", delete(close))
        .merge(split::router())
}

async fn close(
    State(state): State<SessionState>,
    Path((session_id, pane_id)): Path<(SessionId, PaneId)>,
) -> OzmuxResult<Json<serde_json::Value>> {
    let mut session = state.session_mut(&session_id).await?;
    session.close_pane(&pane_id)?;
    Ok(Json(serde_json::to_value(&*session).unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;
    use crate::session::cell::{Side, SplitOrientation};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn router_with(state: SessionState) -> axum::Router {
        crate::http::test_helpers::daemon_router_for_test(state)
    }

    #[tokio::test]
    async fn close_returns_updated_session() {
        let state = SessionState::default();
        let mut session = Session::new(String::new());
        let first_pane = session.panes().any_pane_id().unwrap();
        let new_pane = session
            .split_pane(&first_pane, SplitOrientation::Horizontal, Side::After)
            .expect("split");
        let session_id = session.id().clone();
        {
            let mut guard = state.lock().await;
            guard.insert(session_id.clone(), session);
        }
        let resp = router_with(state.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/sessions/{}/panes/{}", session_id, new_pane))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"].as_str(), Some(session_id.as_ref()));
        assert_eq!(v["panes"].as_array().unwrap().len(), 1);

        let guard = state.lock().await;
        assert!(
            guard
                .get(&session_id)
                .unwrap()
                .panes()
                .get(&new_pane)
                .is_err()
        );
    }

    #[tokio::test]
    async fn close_last_pane_returns_409() {
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
                    .method("DELETE")
                    .uri(format!("/sessions/{}/panes/{}", session_id, pane_id))
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
        let state = SessionState::default();
        let resp = router_with(state)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/sessions/no-such/panes/no-such")
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
    async fn close_with_unknown_pane_returns_404() {
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
                    .method("DELETE")
                    .uri(format!("/sessions/{}/panes/{}", session_id, PaneId::new()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("PANE_NOT_FOUND"));
    }
}
