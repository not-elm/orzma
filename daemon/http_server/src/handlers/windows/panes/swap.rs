//! `POST /windows/{wid}/panes/{pid}/swap` — swap the named pane with its
//! previous/next neighbor in pane-index order.

use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{PaneId, SwapOffset, WindowId};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SwapRequest {
    offset: SwapOffset,
}

/// Handler for `POST /windows/{wid}/panes/{pid}/swap`.
pub async fn swap(
    State(state): State<AppState>,
    Path((wid, pid)): Path<(WindowId, PaneId)>,
    Json(body): Json<SwapRequest>,
) -> HttpResult<StatusCode> {
    state.swap_pane(&wid, &pid, body.offset).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation, WindowId};
    use tower::ServiceExt;

    async fn split_via_window(
        state: &crate::AppState,
        wid: &WindowId,
        target: &PaneId,
        orient: SplitOrientation,
    ) -> PaneId {
        let new_pane_id = PaneId::new();
        let new_activity_id = ActivityId::new();
        state
            .multiplexer
            .with_window_or_404(wid, |w| {
                w.split_pane(
                    target,
                    new_pane_id.clone(),
                    Activity::terminal(new_activity_id.clone()),
                    Side::After,
                    orient,
                )
            })
            .await
            .unwrap();
        state
            .multiplexer
            .pane_owner_window
            .insert(new_pane_id.clone(), wid.clone());
        new_pane_id
    }

    #[tokio::test]
    async fn swap_happy_path_returns_204_and_broadcasts() {
        let state = fresh_state();
        let (_sid, wid, left, _aid) = bootstrap_default(&state).await;
        let _right = split_via_window(&state, &wid, &left, SplitOrientation::Horizontal).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/swap", wid, left))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"offset":"next"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let view = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["id"].as_str(), Some(wid.as_ref()));
    }

    #[tokio::test]
    async fn swap_single_pane_returns_204_without_broadcast() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/swap", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"offset":"next"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let view = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(view.is_err(), "no broadcast expected for single-pane no-op");
    }

    #[tokio::test]
    async fn swap_unknown_pane_returns_404() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/swap", wid, PaneId::new()))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"offset":"next"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn swap_wrong_window_returns_409() {
        let state = fresh_state();
        let (sid, _wid_a, pid_a, _aid) = bootstrap_default(&state).await;
        let (wid_b, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/swap", wid_b, pid_a))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"offset":"next"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn swap_invalid_offset_returns_422() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/swap", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"offset":"sideways"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn swap_malformed_json_returns_400() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/swap", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"offset":"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
