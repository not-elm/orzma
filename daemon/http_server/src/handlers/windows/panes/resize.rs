//! `POST /windows/{wid}/panes/{pid}/resize` — direction-only resize.

use crate::{
    AppState,
    error::{HttpError, HttpResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{PaneDirection, PaneId, WindowId};
use serde::Deserialize;

fn default_amount() -> u16 {
    1
}

/// Request body for `POST /windows/{wid}/panes/{pid}/resize`.
#[derive(Deserialize)]
pub struct ResizeRequest {
    direction: PaneDirection,
    #[serde(default = "default_amount")]
    amount: u16,
}

/// Handler for `POST /windows/{wid}/panes/{pid}/resize`. Rejects
/// `amount == 0` with 422 and otherwise delegates to
/// [`AppState::resize_pane`], which decides whether to broadcast.
pub async fn resize(
    State(state): State<AppState>,
    Path((wid, pid)): Path<(WindowId, PaneId)>,
    Json(body): Json<ResizeRequest>,
) -> HttpResult<StatusCode> {
    if body.amount == 0 {
        return Err(HttpError::InvalidAmount);
    }
    state
        .resize_pane(&wid, &pid, body.direction, body.amount)
        .await?;
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
    async fn resize_happy_path_returns_204_and_broadcasts() {
        let state = fresh_state();
        let (_sid, wid, left, _aid) = bootstrap_default(&state).await;
        let _right = split_via_window(&state, &wid, &left, SplitOrientation::Horizontal).await;
        state.set_window_dimensions(&wid, 120, 40).await.unwrap();
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/resize", wid, left))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"right"}"#))
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
    async fn resize_no_op_returns_204_without_broadcast() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        state.set_window_dimensions(&wid, 120, 40).await.unwrap();
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/resize", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"right"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let view = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(view.is_err(), "no broadcast expected for soft no-op");
    }

    #[tokio::test]
    async fn resize_unknown_pane_returns_404() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        state.set_window_dimensions(&wid, 120, 40).await.unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/resize", wid, PaneId::new()))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"right"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resize_wrong_window_returns_409() {
        let state = fresh_state();
        let (sid, _wid_a, pid_a, _aid) = bootstrap_default(&state).await;
        let (wid_b, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        state.set_window_dimensions(&wid_b, 120, 40).await.unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/resize", wid_b, pid_a))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"right"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn resize_window_not_measured_returns_409() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/resize", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"right"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn resize_invalid_direction_returns_422() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        state.set_window_dimensions(&wid, 120, 40).await.unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/resize", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"sideways"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn resize_zero_amount_returns_422_invalid_amount() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        state.set_window_dimensions(&wid, 120, 40).await.unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/resize", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"right","amount":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn resize_malformed_json_returns_400() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        state.set_window_dimensions(&wid, 120, 40).await.unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/resize", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
